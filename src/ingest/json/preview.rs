use super::*;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};

type Reader = Box<dyn BufRead + Send>;

enum Shape {
    Array,
    Entries,
    Documents,
    Record,
}

struct SequentialJson {
    reader: Reader,
    shape: Shape,
    live_input: bool,
    first: bool,
    pending: VecDeque<FlatRow>,
    seen: HashSet<String>,
    schema: JsonSchema,
    rows: Vec<Row>,
    present: Vec<Vec<usize>>,
    pointer: Option<JsonPointer>,
    eof: bool,
}

fn peek(reader: &mut Reader) -> anyhow::Result<Option<u8>> {
    loop {
        let bytes = reader.fill_buf()?;
        let Some(byte) = bytes.first().copied() else {
            return Ok(None);
        };
        if !byte.is_ascii_whitespace() {
            return Ok(Some(byte));
        }
        reader.consume(1);
    }
}

fn expect(reader: &mut Reader, expected: u8) -> anyhow::Result<()> {
    anyhow::ensure!(
        peek(reader)? == Some(expected),
        "expected '{}' in JSON input",
        expected as char
    );
    reader.consume(1);
    Ok(())
}

fn value<T: serde::de::DeserializeOwned>(reader: &mut Reader) -> anyhow::Result<T> {
    Ok(T::deserialize(&mut serde_json::Deserializer::from_reader(
        reader,
    ))?)
}

fn locate(reader: &mut Reader, segments: &[String]) -> anyhow::Result<()> {
    if segments.is_empty() {
        return Ok(());
    }
    match peek(reader)? {
        Some(b'{') => {
            expect(reader, b'{')?;
            let mut first = true;
            loop {
                anyhow::ensure!(
                    peek(reader)? != Some(b'}'),
                    "JSON starting path was not found"
                );
                if !first {
                    expect(reader, b',')?;
                }
                first = false;
                let key: String = value(reader)?;
                expect(reader, b':')?;
                if key == segments[0] {
                    return locate(reader, &segments[1..]);
                }
                let _: serde::de::IgnoredAny = value(reader)?;
            }
        }
        Some(b'[') => {
            let target: usize = segments[0]
                .parse()
                .map_err(|_| anyhow::anyhow!("JSON Pointer array segment is not an index"))?;
            expect(reader, b'[')?;
            for index in 0..=target {
                anyhow::ensure!(
                    peek(reader)? != Some(b']'),
                    "JSON starting path was not found"
                );
                if index > 0 {
                    expect(reader, b',')?;
                }
                if index == target {
                    return locate(reader, &segments[1..]);
                }
                let _: serde::de::IgnoredAny = value(reader)?;
            }
            unreachable!()
        }
        _ => anyhow::bail!("JSON starting path was not found"),
    }
}

impl SequentialJson {
    fn next_entry(&mut self) -> anyhow::Result<Option<RawObjectEntry>> {
        if peek(&mut self.reader)? == Some(b'}') {
            self.reader.consume(1);
            self.eof = true;
            return Ok(None);
        }
        if !self.first {
            expect(&mut self.reader, b',')?;
        }
        self.first = false;
        let key: String = value(&mut self.reader)?;
        anyhow::ensure!(
            self.seen.insert(key.clone()),
            "duplicate object member key '{key}'"
        );
        expect(&mut self.reader, b':')?;
        Ok(Some(RawObjectEntry {
            key,
            value: value(&mut self.reader)?,
        }))
    }

    fn next_flat(&mut self) -> anyhow::Result<Option<FlatRow>> {
        if let Some(row) = self.pending.pop_front() {
            return Ok(Some(row));
        }
        if self.eof {
            return Ok(None);
        }
        match self.shape {
            Shape::Entries => self
                .next_entry()?
                .map(|entry| {
                    flatten_keyed_entry(
                        &entry.key,
                        &serde_json::from_str(entry.value.get())?,
                        entry.encoded_len(),
                    )
                })
                .transpose(),
            Shape::Record => {
                self.eof = true;
                Ok(None)
            }
            Shape::Array => {
                if peek(&mut self.reader)? == Some(b']') {
                    self.reader.consume(1);
                    self.eof = true;
                    return Ok(None);
                }
                if !self.first {
                    expect(&mut self.reader, b',')?;
                }
                self.first = false;
                let row: Value = value(&mut self.reader)?;
                Ok(Some(flatten_row(&row)?))
            }
            Shape::Documents => {
                if peek(&mut self.reader)?.is_none() {
                    self.eof = true;
                    return Ok(None);
                }
                let row: Value = value(&mut self.reader)?;
                let selected = resolve_pointer(&row, self.pointer.as_ref())?;
                anyhow::ensure!(
                    matches!(selected, Value::Object(_) | Value::Array(_)),
                    "JSON starting path does not identify an object or array"
                );
                Ok(Some(flatten_row(selected)?))
            }
        }
    }
}

pub(super) fn open(
    source: InputSource,
    format: InputFormat,
    options: &OpenOptions,
) -> anyhow::Result<OpenedSource> {
    let reader: Reader = match &source {
        InputSource::Path(path) => Box::new(BufReader::new(File::open(path)?)),
        InputSource::Stdin => Box::new(BufReader::new(std::io::stdin())),
        _ => anyhow::bail!("preview requires a local file or stdin"),
    };
    open_reader(source, format, options, reader)
}

fn open_reader(
    source: InputSource,
    format: InputFormat,
    options: &OpenOptions,
    mut reader: Reader,
) -> anyhow::Result<OpenedSource> {
    // Match the file adapters' UTF-8 BOM handling.
    if reader.fill_buf()?.starts_with(&[0xef, 0xbb, 0xbf]) {
        reader.consume(3);
    }
    if format == InputFormat::Json {
        locate(
            &mut reader,
            options
                .json_path
                .as_ref()
                .map(JsonPointer::segments)
                .unwrap_or_default(),
        )?;
    }
    let generation = SourceGeneration::new();
    let kind = if format == InputFormat::Ndjson {
        None
    } else {
        peek(&mut reader)?
    };
    let shape = match kind {
        None if format == InputFormat::Ndjson => Shape::Documents,
        Some(b'[') => {
            expect(&mut reader, b'[')?;
            Shape::Array
        }
        Some(b'{') => {
            expect(&mut reader, b'{')?;
            Shape::Entries
        }
        _ => anyhow::bail!("JSON starting path does not identify a tabular object or array"),
    };
    let mut store = SequentialJson {
        reader,
        shape,
        live_input: matches!(&source, InputSource::Stdin),
        first: true,
        pending: VecDeque::new(),
        seen: HashSet::new(),
        schema: JsonSchema::new(generation),
        rows: Vec::new(),
        present: Vec::new(),
        pointer: options.json_path.clone(),
        eof: false,
    };
    let mut object_mode = None;
    let mut warnings = Vec::new();
    if matches!(store.shape, Shape::Entries) {
        let mut sample = Vec::new();
        if options.object_mode == ObjectMode::Auto {
            let mut bytes = 0;
            let max_entries = if options.preview && options.schema_scan != SchemaScan::Full {
                3
            } else {
                OBJECT_DETECTION_MAX_ENTRIES
            };
            while sample.len() < max_entries && bytes < OBJECT_DETECTION_MAX_BYTES {
                let Some(entry) = store.next_entry()? else {
                    break;
                };
                bytes += entry.encoded_len();
                sample.push(entry);
            }
        }
        let resolution = resolve_selected_shape(
            SelectedValueShape::Object,
            options.object_mode,
            options.object_mode_origin,
            options.object_mode == ObjectMode::Auto && detect_keyed_object(&sample)?,
        )?;
        object_mode = resolution.object_mode;
        if resolution.table_shape == Some(SelectedTableShape::ObjectRecord) {
            while !store.eof {
                if let Some(entry) = store.next_entry()? {
                    sample.push(entry);
                }
            }
            store.pending.push_back(flatten_object_record(&sample)?);
            store.shape = Shape::Record;
        } else {
            store.pending.extend(project_object_entries(&sample)?);
        }
    } else if matches!(store.shape, Shape::Array) {
        let resolution = resolve_selected_shape(
            SelectedValueShape::Array,
            options.object_mode,
            options.object_mode_origin,
            false,
        )?;
        warnings.extend(resolution.warning);
    }
    let scan_target = if options.schema_scan == SchemaScan::Full
        && options.limit.is_none()
        && options.source_filters.is_empty()
    {
        usize::MAX
    } else {
        0
    };
    store.ensure_indexed_through(RowIndex(scan_target))?;
    store.schema.assign_initial_labels();
    let definition = TableDefinition {
        generation,
        columns: store.schema.columns.clone(),
        schema_state: if store.eof && store.pending.is_empty() {
            SchemaState::Complete
        } else {
            SchemaState::Provisional
        },
        relation: RelationMetadata::implicit(source.display_name(), true),
    };
    Ok(OpenedSource::implicit(OpenedTable {
        generation,
        definition,
        store: Box::new(store),
        object_mode,
        warnings,
    }))
}

impl TableStore for SequentialJson {
    fn present_columns(&self, row: crate::table::RowId) -> Option<Vec<usize>> {
        self.present.get(row.ordinal as usize).cloned()
    }

    fn generation(&self) -> SourceGeneration {
        self.schema.generation
    }
    fn column_count(&self) -> usize {
        self.schema.columns.len()
    }
    fn initial_schema_column_count(&self) -> usize {
        0
    }
    fn is_live_input(&self) -> bool {
        self.live_input
    }
    fn row_count(&self) -> RowCount {
        if self.eof && self.pending.is_empty() {
            RowCount::Exact(self.rows.len())
        } else {
            RowCount::AtLeast(self.rows.len())
        }
    }
    fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
        self.ensure_indexed_through(index)?;
        Ok(self.rows.get(index.0).cloned().map(|mut row| {
            row.cells.resize(self.schema.columns.len(), CellValue::Null);
            row
        }))
    }
    fn ensure_indexed_through(&mut self, index: RowIndex) -> anyhow::Result<IndexProgress> {
        let mut delta = SchemaDelta::default();
        let mut bytes_scanned = 0;
        while self.rows.len() <= index.0 {
            let Some(flat) = self.next_flat()? else {
                break;
            };
            let observed = self.schema.observe(&flat);
            delta.added_columns.extend(observed.added_columns);
            delta.widened_types.extend(observed.widened_types);
            bytes_scanned += flat.source_bytes;
            let mut cells = vec![CellValue::Null; self.schema.columns.len()];
            self.present.push(
                flat.cells
                    .iter()
                    .map(|(path, _, _)| self.schema.indices[path])
                    .collect(),
            );
            for (path, cell, _) in flat.cells {
                cells[self.schema.indices[&path]] = cell;
            }
            self.rows.push(Row::new(
                crate::table::RowId {
                    generation: self.generation(),
                    ordinal: self.rows.len() as u64,
                },
                cells,
            ));
        }
        delta.completed = self.eof && self.pending.is_empty();
        Ok(IndexProgress {
            row_count: self.row_count(),
            schema_delta: delta,
            bytes_scanned,
        })
    }
    fn scan_rows(
        &mut self,
        request: ScanRequest,
        visitor: &mut dyn RowVisitor,
    ) -> anyhow::Result<ScanProgress> {
        let mut next = Some(request.start);
        let mut visited = 0;
        while visited < request.max_rows {
            let Some(index) = next else {
                break;
            };
            let Some(row) = self.row(index)? else {
                next = None;
                break;
            };
            visited += 1;
            next = match request.direction {
                ScanDirection::Forward => index.0.checked_add(1).map(RowIndex),
                ScanDirection::Reverse => index.0.checked_sub(1).map(RowIndex),
            };
            if visitor.visit(index, &row).is_break() {
                break;
            }
        }
        Ok(ScanProgress {
            visited,
            next,
            reached_end: next.is_none(),
        })
    }
    fn materialize(&mut self) -> anyhow::Result<InMemoryTable> {
        self.ensure_indexed_through(RowIndex(usize::MAX))?;
        InMemoryTable::from_rows(self.generation(), self.rows.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_reads_are_bounded_for_json_and_ndjson() {
        for format in [InputFormat::Json, InputFormat::Ndjson] {
            for threshold in [1, u64::MAX] {
                let prefix = if format == InputFormat::Json {
                    "[{\"a\":1},{\"a\":2},{\"a\":3},"
                } else {
                    "{\"a\":1}\n{\"a\":2}\n{\"a\":3}\n"
                };
                let input = std::io::Cursor::new(prefix.as_bytes().to_vec())
                    .chain(std::io::repeat(b'x').take(200 * 1024 * 1024));
                let (input, count) = crate::ingest::CountingReader::new(input);
                let mut table = open_reader(
                    InputSource::Stdin,
                    format,
                    &OpenOptions {
                        preview: true,
                        lazy_threshold_bytes: threshold,
                        ..OpenOptions::default()
                    },
                    Box::new(BufReader::new(input)),
                )
                .unwrap()
                .into_implicit_table()
                .unwrap();
                table.store.ensure_indexed_through(RowIndex(2)).unwrap();
                assert_eq!(table.store.row_count(), RowCount::AtLeast(3));
                assert!(count.load(std::sync::atomic::Ordering::Relaxed) <= 8192);
            }
        }
    }
}
