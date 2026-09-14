use super::*;
use crate::ingest::SchemaScan;
use crate::table::RowId;
use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read};
use std::path::PathBuf;

/// Decode on demand so a short stdin preview need not wait for EOF.
struct DecodedReader {
    input: Box<dyn BufRead + Send>,
    decoder: Option<encoding_rs::Decoder>,
    cp720: bool,
    pending: std::io::Cursor<Vec<u8>>,
    eof: bool,
    fallback_path: Option<PathBuf>,
    raw_consumed: u64,
    decoder_pending_start: Option<u64>,
}

impl DecodedReader {
    fn restart_with_fallback(&mut self, offset: u64) -> io::Result<bool> {
        let Some(path) = self.fallback_path.take() else {
            return Ok(false);
        };
        const ENCODING_FALLBACK_PROBE_BYTES: u64 = 8 * 1024 * 1024;
        let mut bytes = Vec::new();
        std::fs::File::open(&path)?
            .take(ENCODING_FALLBACK_PROBE_BYTES)
            .read_to_end(&mut bytes)?;
        let decoded = crate::ingest::decode_input(&bytes, None)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        if decoded.encoding == "utf-8" {
            return Ok(false);
        }
        let cp720 = decoded.encoding == "cp720";
        let decoder = if cp720 {
            None
        } else {
            Some(
                crate::ingest::encoding_for_label(&decoded.encoding)
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("unknown encoding '{}'", decoded.encoding),
                        )
                    })?
                    .new_decoder(),
            )
        };
        let mut reader = BufReader::new(std::fs::File::open(path)?);
        let mut remaining = offset;
        while remaining > 0 {
            let available = reader.fill_buf()?;
            if available.is_empty() {
                return Ok(false);
            }
            let consumed = remaining.min(available.len() as u64) as usize;
            reader.consume(consumed);
            remaining -= consumed as u64;
        }
        self.input = Box::new(reader);
        self.decoder = decoder;
        self.cp720 = cp720;
        self.pending = std::io::Cursor::new(Vec::new());
        self.eof = false;
        self.raw_consumed = offset;
        self.decoder_pending_start = None;
        Ok(true)
    }
}

impl Read for DecodedReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            let count = self.pending.read(out)?;
            if count > 0 || self.eof {
                return Ok(count);
            }
            let available = self.input.fill_buf()?;
            let mut text = String::with_capacity(8);
            if self.cp720 {
                if let Some(byte) = available.first() {
                    text = crate::ingest::decode_cp720(&[*byte]);
                    self.input.consume(1);
                } else {
                    self.eof = true;
                }
            } else {
                let eof = available.is_empty();
                let input = &available[..available.len().min(1)];
                let (_, consumed, errors) = self
                    .decoder
                    .as_mut()
                    .expect("decoder")
                    .decode_to_string(input, &mut text, eof);
                self.input.consume(consumed);
                let previous_consumed = self.raw_consumed;
                self.raw_consumed += consumed as u64;
                let restart_offset = self.decoder_pending_start.unwrap_or(previous_consumed);
                if errors {
                    if self.restart_with_fallback(restart_offset)? {
                        continue;
                    }
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "input cannot be decoded with the selected encoding",
                    ));
                }
                if text.is_empty() && consumed > 0 {
                    self.decoder_pending_start.get_or_insert(previous_consumed);
                } else if !text.is_empty() {
                    self.decoder_pending_start = None;
                }
                self.eof = eof;
            }
            self.pending = std::io::Cursor::new(text.into_bytes());
        }
    }
}

enum Records {
    Csv {
        reader: csv::Reader<Box<dyn Read + Send>>,
        pending: VecDeque<Vec<String>>,
    },
    Space {
        reader: BufReader<Box<dyn Read + Send>>,
        first: bool,
        pending: VecDeque<Vec<String>>,
    },
}
impl Records {
    fn next(&mut self) -> anyhow::Result<Option<Vec<String>>> {
        match self {
            Self::Csv { reader, pending } => {
                if let Some(row) = pending.pop_front() {
                    return Ok(Some(row));
                }
                let mut record = csv::StringRecord::new();
                Ok(reader
                    .read_record(&mut record)?
                    .then(|| record.iter().map(ToOwned::to_owned).collect()))
            }
            Self::Space {
                reader,
                first,
                pending,
            } => {
                if let Some(row) = pending.pop_front() {
                    return Ok(Some(row));
                }
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line)? == 0 {
                        return Ok(None);
                    }
                    let line = if *first {
                        line.strip_prefix('#')
                            .or_else(|| line.strip_prefix('%'))
                            .unwrap_or(&line)
                    } else {
                        &line
                    };
                    if !line.trim().is_empty() {
                        *first = false;
                        return Ok(Some(crate::ingest::split_shell_like(line)));
                    }
                }
            }
        }
    }

    fn prepend(&mut self, rows: impl IntoIterator<Item = Vec<String>>) {
        match self {
            Self::Csv { pending, .. } | Self::Space { pending, .. } => {
                pending.extend(rows);
            }
        }
    }
}

struct SequentialDelimited {
    records: Records,
    definition: TableDefinition,
    initial_schema_column_count: usize,
    rows: Vec<Row>,
    eof: bool,
}

pub(super) fn open(source: InputSource, options: &OpenOptions) -> anyhow::Result<OpenedSource> {
    let input: Box<dyn BufRead + Send> = match &source {
        InputSource::Path(path) => Box::new(BufReader::new(std::fs::File::open(path)?)),
        InputSource::Stdin => Box::new(BufReader::new(std::io::stdin())),
        _ => anyhow::bail!("preview requires a local file or stdin"),
    };
    open_reader(source, options, input)
}

fn open_reader(
    source: InputSource,
    options: &OpenOptions,
    mut input: Box<dyn BufRead + Send>,
) -> anyhow::Result<OpenedSource> {
    const ENCODING_PROBE_BYTES: u64 = 1024 * 1024;
    const PREVIEW_ENCODING_PROBE_BYTES: u64 = 8192;
    let mut encoding_probe = Vec::new();
    if options.delimited.encoding.is_none() && matches!(&source, InputSource::Path(_)) {
        let probe_bytes = if options.preview {
            PREVIEW_ENCODING_PROBE_BYTES
        } else {
            ENCODING_PROBE_BYTES
        };
        (&mut *input)
            .take(probe_bytes)
            .read_to_end(&mut encoding_probe)?;
    }
    let sample = if encoding_probe.is_empty() {
        input.fill_buf()?
    } else {
        encoding_probe.as_slice()
    };
    let label = if let Some(label) = &options.delimited.encoding {
        crate::ingest::normalize_encoding_label(label)
    } else {
        // An incomplete final UTF-8 character in read-ahead does not imply a legacy encoding.
        match std::str::from_utf8(sample) {
            Ok(_) => "utf-8".to_owned(),
            Err(error) if error.error_len().is_none() => "utf-8".to_owned(),
            _ => decode_input(sample, None)?.encoding,
        }
    };
    if !encoding_probe.is_empty() {
        input = Box::new(BufReader::new(
            std::io::Cursor::new(encoding_probe).chain(input),
        ));
    }
    let cp720 = label == "cp720";
    let decoder = if cp720 {
        None
    } else {
        Some(
            crate::ingest::encoding_for_label(&label)
                .ok_or_else(|| anyhow::anyhow!("unknown encoding '{label}'"))?
                .new_decoder(),
        )
    };
    let fallback_path = (label == "utf-8" && options.delimited.encoding.is_none())
        .then(|| match &source {
            InputSource::Path(path) => path.clone(),
            _ => PathBuf::new(),
        })
        .filter(|path| !path.as_os_str().is_empty());
    let decoded: Box<dyn Read + Send> = if label == "utf-8" && fallback_path.is_none() {
        Box::new(input)
    } else {
        Box::new(DecodedReader {
            input,
            decoder,
            cp720,
            pending: std::io::Cursor::new(Vec::new()),
            eof: false,
            fallback_path,
            raw_consumed: 0,
            decoder_pending_start: None,
        })
    };
    let mut decoded = BufReader::new(decoded);
    // Sniff the first physical line and, for leading blank/comment lines, a small bounded
    // follow-up sample. Replay every sampled line so preview parsing sees the same input.
    let mut sample = String::new();
    decoded.read_line(&mut sample)?;
    let first_line_needs_followup = sample
        .lines()
        .next()
        .is_none_or(|line| line.trim().is_empty() || line.trim_start().starts_with(['#', '%']));
    let path_needs_followup =
        matches!(&source, InputSource::Path(_)) && sniff_delimiter(&sample).is_none();
    if options.delimited.delimiter.is_none() && (first_line_needs_followup || path_needs_followup) {
        let mut line = String::new();
        for _ in 0..3 {
            if sample.len() >= 8192 {
                break;
            }
            line.clear();
            if decoded.read_line(&mut line)? == 0 {
                break;
            }
            sample.push_str(&line);
        }
    }
    let delimiter = options
        .delimited
        .delimiter
        .unwrap_or_else(|| sniff_delimiter(&sample).unwrap_or(b','));
    let replay: Box<dyn Read + Send> =
        Box::new(std::io::Cursor::new(sample.into_bytes()).chain(decoded));
    let mut records = if delimiter == b' ' {
        Records::Space {
            reader: BufReader::new(replay),
            first: true,
            pending: VecDeque::new(),
        }
    } else {
        Records::Csv {
            reader: csv::ReaderBuilder::new()
                .has_headers(false)
                .flexible(true)
                .delimiter(delimiter)
                .quote(options.delimited.quote_char)
                .quoting(options.delimited.quoting != Some(Quoting::None))
                .from_reader(replay),
            pending: VecDeque::new(),
        }
    };
    let mut sample = Vec::new();
    for _ in 0..2 {
        match records.next() {
            Ok(Some(row)) => sample.push(row),
            Ok(None) => break,
            Err(_) if options.limit.is_some_and(|limit| limit.get() == 1) && sample.len() == 1 => {
                break;
            }
            Err(error) => return Err(error),
        }
    }
    let generation = SourceGeneration::new();
    let (_, header_rows) = delimited_definition(generation, &sample, source.display_name());
    let header_column_count = if header_rows == 1 {
        sample.first().map_or(0, Vec::len)
    } else {
        0
    };
    let definition_sample = sample
        .iter()
        .take(header_rows.saturating_add(1))
        .cloned()
        .collect::<Vec<_>>();
    let (mut definition, _) =
        delimited_definition(generation, &definition_sample, source.display_name());
    let initial_schema_column_count = if header_rows == 1 {
        header_column_count
    } else {
        0
    };
    definition.schema_state = SchemaState::Provisional;
    let seed_end = header_rows.saturating_add(1).min(sample.len());
    records.prepend(sample.iter().skip(seed_end).cloned());
    let rows = sample
        .into_iter()
        .skip(header_rows)
        .take(1)
        .enumerate()
        .map(|(index, row)| {
            Row::new(
                crate::table::RowId {
                    generation,
                    ordinal: index as u64,
                },
                row.into_iter().map(CellValue::Text).collect(),
            )
        })
        .collect();
    let mut store = SequentialDelimited {
        records,
        definition: definition.clone(),
        initial_schema_column_count,
        rows,
        eof: false,
    };
    if options.schema_scan == SchemaScan::Full
        && options.limit.is_none()
        && options.source_filters.is_empty()
    {
        store.ensure_indexed_through(RowIndex(usize::MAX))?;
    }
    let definition = store.definition.clone();
    Ok(OpenedSource::implicit(OpenedTable {
        generation,
        definition,
        store: Box::new(store),
        object_mode: None,
        warnings: Vec::new(),
    }))
}

impl TableStore for SequentialDelimited {
    fn present_columns(&self, row: RowId) -> Option<Vec<usize>> {
        self.rows
            .get(row.ordinal as usize)
            .map(|row| (0..row.cells.len().max(self.initial_schema_column_count)).collect())
    }

    fn generation(&self) -> SourceGeneration {
        self.definition.generation
    }
    fn column_count(&self) -> usize {
        self.definition.columns.len()
    }
    fn initial_schema_column_count(&self) -> usize {
        self.initial_schema_column_count
    }
    fn row_count(&self) -> RowCount {
        if self.eof {
            RowCount::Exact(self.rows.len())
        } else {
            RowCount::AtLeast(self.rows.len())
        }
    }
    fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
        self.ensure_indexed_through(index)?;
        Ok(self.rows.get(index.0).cloned())
    }
    fn ensure_indexed_through(&mut self, index: RowIndex) -> anyhow::Result<IndexProgress> {
        let mut delta = SchemaDelta::default();
        let mut bytes_scanned = 0;
        while !self.eof && self.rows.len() <= index.0 {
            let Some(cells) = self.records.next()? else {
                self.eof = true;
                break;
            };
            for ordinal in self.definition.columns.len()..cells.len() {
                let column = delimited_column(self.generation(), ordinal, None);
                self.definition.columns.push(column.clone());
                delta.added_columns.push(column);
            }
            bytes_scanned += cells.iter().map(|cell| cell.len() as u64).sum::<u64>();
            self.rows.push(Row::new(
                crate::table::RowId {
                    generation: self.generation(),
                    ordinal: self.rows.len() as u64,
                },
                cells.into_iter().map(CellValue::Text).collect(),
            ));
        }
        delta.completed = self.eof;
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
    fn preview_reads_are_bounded_regardless_of_lazy_threshold() {
        for threshold in [1, u64::MAX] {
            let input = std::io::Cursor::new(b"A,B\n1,x\n2,y\n3,z\n".to_vec())
                .chain(std::io::repeat(b'x').take(200 * 1024 * 1024));
            let (input, count) = crate::ingest::CountingReader::new(input);
            let mut table = open_reader(
                InputSource::Stdin,
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
