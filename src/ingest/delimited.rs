mod preview;

use crate::table::{
    CellValue, ColumnDefinition, ColumnId, ColumnSourceIdentity, InMemoryTable, IndexProgress,
    LazyFileTable, LogicalType, OffsetTableStore, RelationMetadata, Row, RowCount, RowIndex,
    RowVisitor, ScanDirection, ScanProgress, ScanRequest, SchemaDelta, SchemaState,
    SourceGeneration, TableDefinition, TableStore, TypeOrigin,
};

use super::adapter::{OpenedSource, OpenedTable, ProbeResult, SourceAdapter};
use super::source::{read_source, InputSource, StreamingInput};
use super::{
    decode_input, parse_decoded_rows, parse_rows, sniff_delimiter, InputFormat, OpenOptions,
    Quoting,
};

#[derive(Debug, Default)]
pub struct DelimitedAdapter;

impl SourceAdapter for DelimitedAdapter {
    fn format(&self) -> InputFormat {
        InputFormat::Delimited
    }

    fn probe(&self, _source: &InputSource, sample: &[u8]) -> ProbeResult {
        let Ok(text) = std::str::from_utf8(sample) else {
            return ProbeResult::Possible;
        };
        if super::sniff_delimiter(text).is_some() {
            ProbeResult::Strong
        } else {
            ProbeResult::Possible
        }
    }

    fn open(&self, source: InputSource, options: &OpenOptions) -> anyhow::Result<OpenedSource> {
        options.validate()?;
        if options.preview {
            return preview::open(source, options);
        }
        if let InputSource::StreamingStdin(input) = &source {
            return open_streaming_delimited(input.clone(), source.display_name(), options);
        }
        if let InputSource::Path(path) = &source {
            if std::fs::metadata(path)?.len() >= options.lazy_threshold_bytes
                && LazyFileTable::supports_options(&options.delimited)
            {
                let mut store = LazyFileTable::open(path, options.delimited.clone())?;
                let generation = store.generation();
                let mut sample_rows = Vec::new();
                for index in 0..2 {
                    if let Some(row) = store.row(RowIndex(index))? {
                        sample_rows.push(row.display_cells());
                    }
                }
                let (definition, header_rows) =
                    delimited_definition(generation, &sample_rows, source.display_name());
                let store: Box<dyn TableStore> = if header_rows == 0 {
                    Box::new(store)
                } else {
                    Box::new(OffsetTableStore::new(Box::new(store), header_rows))
                };
                return Ok(OpenedSource::implicit(OpenedTable {
                    generation,
                    definition,
                    store,
                    object_mode: None,
                    warnings: Vec::new(),
                }));
            }
        }
        let bytes = read_source(&source)?;
        let rows = parse_rows(&bytes, &options.delimited)?;
        let display_name = source.display_name();
        Ok(OpenedSource::implicit(open_delimited_rows(
            rows,
            display_name,
        )))
    }
}

fn open_streaming_delimited(
    input: StreamingInput,
    display_name: String,
    options: &OpenOptions,
) -> anyhow::Result<OpenedSource> {
    input.wait_for_delimited_sample()?;
    let generation = SourceGeneration::new();
    let mut store = StreamingDelimitedTable {
        generation,
        input,
        options: options.delimited.clone(),
        display_name,
        rows: Vec::new(),
        columns: Vec::new(),
        header_rows: None,
        parsed_bytes: 0,
        last_bytes: usize::MAX,
        complete: false,
        options_resolved: false,
    };
    store.refresh(false)?;
    let definition = TableDefinition {
        generation,
        columns: store.columns.clone(),
        schema_state: if store.complete {
            SchemaState::Complete
        } else {
            SchemaState::Provisional
        },
        relation: RelationMetadata::implicit(
            store.display_name.clone(),
            store.header_rows == Some(1),
        ),
    };
    Ok(OpenedSource::implicit(OpenedTable {
        generation,
        definition,
        store: Box::new(store),
        object_mode: None,
        warnings: Vec::new(),
    }))
}

struct StreamingDelimitedTable {
    generation: SourceGeneration,
    input: StreamingInput,
    options: crate::ingest::ParseOptions,
    display_name: String,
    rows: Vec<Row>,
    columns: Vec<ColumnDefinition>,
    header_rows: Option<usize>,
    parsed_bytes: usize,
    last_bytes: usize,
    complete: bool,
    options_resolved: bool,
}

impl StreamingDelimitedTable {
    fn refresh(&mut self, wait_for_completion: bool) -> anyhow::Result<SchemaDelta> {
        let snapshot = self.input.snapshot(wait_for_completion)?;
        if snapshot.bytes.len() == self.last_bytes && snapshot.complete == self.complete {
            return Ok(SchemaDelta::default());
        }
        let incrementally_safe = streaming_encoding_is_incremental(&self.options, &snapshot.bytes);
        if !incrementally_safe && !snapshot.complete {
            self.last_bytes = snapshot.bytes.len();
            return Ok(SchemaDelta::default());
        }
        let parse_start = if incrementally_safe {
            self.parsed_bytes
        } else {
            0
        };
        let parse_len = if snapshot.complete {
            snapshot.bytes.len()
        } else {
            parse_start
                + complete_delimited_prefix(
                    &snapshot.bytes[parse_start..],
                    self.options.quote_char,
                    self.options.quoting != Some(Quoting::None),
                    false,
                )
        };
        let parsed = if parse_len == parse_start {
            Vec::new()
        } else {
            match self.parse_chunk(&snapshot.bytes[parse_start..parse_len]) {
                Ok(rows) => rows,
                Err(_) if !snapshot.complete => {
                    self.last_bytes = snapshot.bytes.len();
                    return Ok(SchemaDelta::default());
                }
                Err(error) => return Err(error.into()),
            }
        };

        if !streaming_encoding_is_incremental(&self.options, &snapshot.bytes) && !snapshot.complete
        {
            self.last_bytes = snapshot.bytes.len();
            return Ok(SchemaDelta::default());
        }
        if self.header_rows.is_none() && parsed.len() < 2 && !snapshot.complete {
            self.last_bytes = snapshot.bytes.len();
            return Ok(SchemaDelta::default());
        }

        let previous_columns = self.columns.len();
        let header_rows = if let Some(header_rows) = self.header_rows {
            let column_count = parsed
                .iter()
                .map(Vec::len)
                .max()
                .unwrap_or_default()
                .max(self.columns.len());
            self.extend_columns(column_count);
            header_rows
        } else {
            let (candidate, detected_header_rows) =
                delimited_definition(self.generation, &parsed, self.display_name.clone());
            self.columns = candidate.columns;
            self.header_rows = Some(detected_header_rows);
            detected_header_rows
        };
        if self.columns.len() > previous_columns {
            for row in &mut self.rows {
                row.cells
                    .resize(self.columns.len(), CellValue::Text(String::new()));
            }
        }
        let skip = usize::from(parse_start == 0) * header_rows;
        for mut cells in parsed.into_iter().skip(skip) {
            cells.resize(self.columns.len(), String::new());
            self.rows
                .push(Row::from_text(self.generation, self.rows.len(), cells));
        }
        self.parsed_bytes = parse_len;
        self.last_bytes = snapshot.bytes.len();
        let became_complete = snapshot.complete && !self.complete;
        self.complete = snapshot.complete;
        Ok(SchemaDelta {
            added_columns: self.columns[previous_columns..].to_vec(),
            widened_types: Vec::new(),
            completed: became_complete,
        })
    }

    fn parse_chunk(
        &mut self,
        bytes: &[u8],
    ) -> Result<Vec<Vec<String>>, crate::ingest::IngestError> {
        if self.options_resolved {
            return parse_rows(bytes, &self.options);
        }
        let decoded = decode_input(bytes, self.options.encoding.as_deref())?;
        self.options.encoding = Some(decoded.encoding);
        self.options.delimiter = Some(
            self.options
                .delimiter
                .unwrap_or_else(|| sniff_delimiter(&decoded.text).unwrap_or(b',')),
        );
        self.options_resolved = true;
        parse_decoded_rows(&decoded.text, &self.options)
    }

    fn extend_columns(&mut self, column_count: usize) {
        self.columns.extend(
            (self.columns.len()..column_count)
                .map(|ordinal| delimited_column(self.generation, ordinal, None)),
        );
    }
}

fn streaming_encoding_is_incremental(options: &crate::ingest::ParseOptions, bytes: &[u8]) -> bool {
    let utf16_option = options.encoding.as_deref().is_some_and(|encoding| {
        encoding
            .trim()
            .to_ascii_lowercase()
            .replace('_', "-")
            .starts_with("utf-16")
    });
    !utf16_option && !bytes.starts_with(&[0xFF, 0xFE]) && !bytes.starts_with(&[0xFE, 0xFF])
}

fn complete_delimited_prefix(bytes: &[u8], quote: u8, quoting: bool, complete: bool) -> usize {
    let mut in_quotes = false;
    let mut last_record_end = 0;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if quoting && byte == quote {
            if in_quotes && bytes.get(index + 1) == Some(&quote) {
                index += 2;
                continue;
            }
            in_quotes = !in_quotes;
        } else if !in_quotes
            && (byte == b'\n'
                || (byte == b'\r' && bytes.get(index + 1).is_some_and(|next| *next != b'\n')))
        {
            last_record_end = index + 1;
        }
        index += 1;
    }
    if complete {
        bytes.len()
    } else {
        last_record_end
    }
}

impl TableStore for StreamingDelimitedTable {
    fn generation(&self) -> SourceGeneration {
        self.generation
    }

    fn row_count(&self) -> RowCount {
        if self.complete {
            RowCount::Exact(self.rows.len())
        } else if self.rows.is_empty() {
            RowCount::Unknown
        } else {
            RowCount::AtLeast(self.rows.len())
        }
    }

    fn column_count(&self) -> usize {
        self.columns.len()
    }

    fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
        Ok(self.rows.get(index.0).cloned())
    }

    fn ensure_indexed_through(&mut self, index: RowIndex) -> anyhow::Result<IndexProgress> {
        let delta = self.refresh(index.0 == usize::MAX)?;
        Ok(IndexProgress {
            row_count: self.row_count(),
            schema_delta: delta,
            bytes_scanned: self.last_bytes as u64,
        })
    }

    fn scan_rows(
        &mut self,
        request: ScanRequest,
        visitor: &mut dyn RowVisitor,
    ) -> anyhow::Result<ScanProgress> {
        self.refresh(false)?;
        let mut current = request.start.0;
        let mut visited = 0;
        while visited < request.max_rows {
            let Some(row) = self.rows.get(current) else {
                break;
            };
            visited += 1;
            if visitor.visit(RowIndex(current), row).is_break() {
                return Ok(ScanProgress {
                    visited,
                    next: None,
                    reached_end: false,
                });
            }
            match request.direction {
                ScanDirection::Forward => current += 1,
                ScanDirection::Reverse if current > 0 => current -= 1,
                ScanDirection::Reverse => {
                    return Ok(ScanProgress {
                        visited,
                        next: None,
                        reached_end: true,
                    });
                }
            }
        }
        let reached_end = current >= self.rows.len();
        Ok(ScanProgress {
            visited,
            next: (!reached_end).then_some(RowIndex(current)),
            reached_end,
        })
    }

    fn materialize(&mut self) -> anyhow::Result<InMemoryTable> {
        InMemoryTable::from_rows(self.generation, self.rows.clone())
    }
}

pub fn open_delimited_rows(rows: Vec<Vec<String>>, display_name: String) -> OpenedTable {
    let generation = SourceGeneration::new();
    let (definition, header_rows) = delimited_definition(generation, &rows, display_name);
    let data_rows = rows.into_iter().skip(header_rows).collect();
    OpenedTable {
        generation,
        definition,
        store: Box::new(InMemoryTable::from_text_rows(generation, data_rows)),
        object_mode: None,
        warnings: Vec::new(),
    }
}

fn delimited_definition(
    generation: SourceGeneration,
    rows: &[Vec<String>],
    display_name: String,
) -> (TableDefinition, usize) {
    let has_header = rows.len() > 1
        && rows
            .first()
            .is_some_and(|row| !row.iter().any(|cell| cell.parse::<f64>().is_ok()));
    let (header, data_rows) = if has_header {
        (rows.first(), &rows[1..])
    } else {
        (None, rows)
    };
    let column_count = header
        .as_ref()
        .map(|header| header.len())
        .unwrap_or_default()
        .max(data_rows.iter().map(Vec::len).max().unwrap_or_default());
    let columns = (0..column_count)
        .map(|ordinal| {
            let name = header
                .as_ref()
                .and_then(|header| header.get(ordinal))
                .cloned();
            delimited_column(generation, ordinal, name)
        })
        .collect();
    let relation = RelationMetadata::implicit(display_name, has_header);
    (
        TableDefinition {
            generation,
            columns,
            schema_state: SchemaState::Complete,
            relation,
        },
        usize::from(has_header),
    )
}

fn delimited_column(
    generation: SourceGeneration,
    ordinal: usize,
    name: Option<String>,
) -> ColumnDefinition {
    let display_name = name
        .as_deref()
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("Column {}", ordinal + 1));
    ColumnDefinition {
        id: ColumnId {
            generation,
            ordinal: ordinal as u32,
        },
        source_identity: ColumnSourceIdentity::Delimited {
            ordinal,
            name: name.clone(),
        },
        display_name,
        source_declared_type: None,
        source_type: LogicalType::Text,
        type_origin: TypeOrigin::Declared,
    }
}

#[cfg(test)]
mod tests {
    use crate::table::{CellValue, RowCount, RowIndex};

    use super::*;

    #[test]
    fn moves_header_classification_into_table_definition() {
        let mut opened = open_delimited_rows(
            vec![
                vec!["name".to_owned(), "name".to_owned(), String::new()],
                vec!["a".to_owned(), "b".to_owned(), String::new()],
            ],
            "data.csv".to_owned(),
        );
        assert!(opened.definition.relation.header_visible);
        assert_eq!(opened.definition.columns[0].display_name, "name");
        assert_eq!(opened.definition.columns[1].display_name, "name");
        assert_ne!(
            opened.definition.columns[0].id,
            opened.definition.columns[1].id
        );
        assert_eq!(opened.definition.columns[2].display_name, "Column 3");
        assert_eq!(opened.store.row_count(), RowCount::Exact(1));
        assert_eq!(
            opened
                .store
                .row(RowIndex(0))
                .expect("read")
                .expect("row")
                .cells,
            vec![
                CellValue::Text("a".to_owned()),
                CellValue::Text("b".to_owned()),
                CellValue::Text(String::new())
            ]
        );
    }

    #[test]
    fn headerless_columns_have_stable_generated_definitions() {
        let opened = open_delimited_rows(
            vec![
                vec!["1".to_owned(), "2".to_owned()],
                vec!["3".to_owned(), "4".to_owned()],
            ],
            "numbers.csv".to_owned(),
        );
        assert!(!opened.definition.relation.header_visible);
        assert_eq!(opened.definition.columns[0].display_name, "Column 1");
        assert!(matches!(
            opened.definition.columns[0].source_identity,
            ColumnSourceIdentity::Delimited {
                ordinal: 0,
                name: None
            }
        ));
    }

    #[test]
    fn large_seekable_input_uses_incremental_store_and_skips_header() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data.csv");
        std::fs::write(&path, "name,value\na,1\nb,2\n").expect("write");
        let options = OpenOptions {
            lazy_threshold_bytes: 0,
            ..OpenOptions::default()
        };
        let source = DelimitedAdapter
            .open(InputSource::Path(path), &options)
            .expect("open");
        let mut table = source.into_implicit_table().expect("table");
        assert!(table.definition.relation.header_visible);
        assert_eq!(table.definition.columns[0].display_name, "name");
        assert_eq!(table.store.row_count(), RowCount::AtLeast(1));
        assert_eq!(
            table
                .store
                .row(RowIndex(0))
                .expect("read")
                .expect("row")
                .display_cells(),
            ["a", "1"]
        );
    }

    #[test]
    fn unsafe_byte_encoding_falls_back_to_materialized_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data.tsv");
        let mut bytes = vec![0xff, 0xfe];
        for unit in "a\tb\n1\t2\n".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        std::fs::write(&path, bytes).expect("write");
        let options = OpenOptions {
            lazy_threshold_bytes: 0,
            delimited: crate::ingest::ParseOptions {
                encoding: Some("utf-16".to_owned()),
                ..crate::ingest::ParseOptions::default()
            },
            ..OpenOptions::default()
        };
        let source = DelimitedAdapter
            .open(InputSource::Path(path), &options)
            .expect("open");
        let table = source.into_implicit_table().expect("table");
        assert_eq!(table.store.row_count(), RowCount::Exact(1));
    }

    #[test]
    fn streaming_stdin_exposes_rows_before_eof_and_completes_on_demand() {
        let input = StreamingInput::pending_for_test();
        input.append_for_test(b"name,value\na,1\n");
        let source = DelimitedAdapter
            .open(
                InputSource::StreamingStdin(input.clone()),
                &OpenOptions::default(),
            )
            .expect("open streaming input");
        let mut table = source.into_implicit_table().expect("table");
        assert_eq!(table.store.row_count(), RowCount::AtLeast(1));
        assert_eq!(
            table
                .store
                .row(RowIndex(0))
                .unwrap()
                .unwrap()
                .display_cells(),
            ["a", "1"]
        );

        input.append_for_test(b"b,2\n");
        let progress = table
            .store
            .ensure_indexed_through(RowIndex(1))
            .expect("refresh available rows");
        assert_eq!(progress.row_count, RowCount::AtLeast(2));
        input.finish_for_test();
        let progress = table
            .store
            .ensure_indexed_through(RowIndex(usize::MAX))
            .expect("finish stream");
        assert_eq!(progress.row_count, RowCount::Exact(2));
        assert!(progress.schema_delta.completed);
    }

    #[test]
    fn streaming_view_appends_available_rows_without_navigation() {
        let input = StreamingInput::pending_for_test();
        input.append_for_test(b"name,value\na,1\n");
        let opened = DelimitedAdapter
            .open(
                InputSource::StreamingStdin(input.clone()),
                &OpenOptions::default(),
            )
            .expect("open streaming input")
            .into_implicit_table()
            .expect("table");
        let mut view =
            crate::view::TableView::from_opened_table(opened, crate::view::Viewport::new(1, 10))
                .expect("view");
        assert_eq!(view.row_count(), 1);

        input.append_for_test(b"b,2\n");
        view.resize_viewport(1, 10);
        assert_eq!(view.row_count(), 2);
        assert_eq!(view.visible_raw_rows_vec()[1], ["b", "2"]);
        input.finish_for_test();
    }

    #[test]
    fn streaming_parser_appends_complete_multiline_records_without_duplicates() {
        let input = StreamingInput::pending_for_test();
        input.append_for_test(b"name,note\n\"a\",\"line 1\nline 2\"\n\"b\",\"pending");
        let source = DelimitedAdapter
            .open(
                InputSource::StreamingStdin(input.clone()),
                &OpenOptions::default(),
            )
            .expect("open streaming input");
        let mut table = source.into_implicit_table().expect("table");
        assert_eq!(table.store.row_count(), RowCount::AtLeast(1));
        assert_eq!(
            table
                .store
                .row(RowIndex(0))
                .unwrap()
                .unwrap()
                .display_cells(),
            ["a", "line 1\nline 2"]
        );

        input.append_for_test(b"\"\n");
        input.finish_for_test();
        let progress = table
            .store
            .ensure_indexed_through(RowIndex(usize::MAX))
            .expect("finish stream");
        assert_eq!(progress.row_count, RowCount::Exact(2));
        assert_eq!(
            table
                .store
                .row(RowIndex(1))
                .unwrap()
                .unwrap()
                .display_cells(),
            ["b", "pending"]
        );
    }

    #[test]
    fn streaming_parser_extends_schema_and_pads_prior_rows() {
        let input = StreamingInput::pending_for_test();
        input.append_for_test(b"a,b\n1,2\n");
        let source = DelimitedAdapter
            .open(
                InputSource::StreamingStdin(input.clone()),
                &OpenOptions::default(),
            )
            .expect("open streaming input");
        let mut table = source.into_implicit_table().expect("table");

        input.append_for_test(b"3,4,5\n");
        let progress = table
            .store
            .ensure_indexed_through(RowIndex(1))
            .expect("refresh rows");
        assert_eq!(progress.schema_delta.added_columns.len(), 1);
        assert_eq!(
            table
                .store
                .row(RowIndex(0))
                .unwrap()
                .unwrap()
                .display_cells(),
            ["1", "2", ""]
        );
        assert_eq!(
            table
                .store
                .row(RowIndex(1))
                .unwrap()
                .unwrap()
                .display_cells(),
            ["3", "4", "5"]
        );
        input.finish_for_test();
    }

    #[test]
    fn complete_record_prefix_ignores_newlines_inside_quotes() {
        let bytes = b"name,note\n\"a\",\"line 1\nline 2\"\n\"b\",\"pending";

        assert_eq!(
            complete_delimited_prefix(bytes, b'"', true, false),
            b"name,note\n\"a\",\"line 1\nline 2\"\n".len()
        );
        assert_eq!(
            complete_delimited_prefix(bytes, b'"', true, true),
            bytes.len()
        );
    }
}
