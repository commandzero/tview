mod preview;

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::table::{
    CellValue, ColumnDefinition, ColumnId, ColumnSourceIdentity, InMemoryTable, IndexProgress,
    RelationMetadata, Row, RowCount, RowId, RowIndex, RowVisitor, ScanDirection, ScanProgress,
    ScanRequest, SchemaDelta, SchemaState, SourceGeneration, TableDefinition, TableStore,
    TypeOrigin, TypeWidening,
};

use super::adapter::{OpenedSource, OpenedTable, ProbeResult, SourceAdapter};
use super::source::{read_source, InputSource, StreamingInput};
use super::streaming_json::{RawObjectEntry, SelectedJsonValue};
use super::{
    resolve_selected_shape, InputFormat, JsonPointer, ObjectMode, ObjectModeResolution,
    OpenOptions, SchemaScan, SelectedTableShape, SelectedValueShape, SourceOptionError,
};

#[derive(Debug, Clone, Copy)]
pub struct JsonAdapter {
    format: InputFormat,
}

impl JsonAdapter {
    pub fn json() -> Self {
        Self {
            format: InputFormat::Json,
        }
    }

    pub fn ndjson() -> Self {
        Self {
            format: InputFormat::Ndjson,
        }
    }
}

impl SourceAdapter for JsonAdapter {
    fn format(&self) -> InputFormat {
        self.format
    }

    fn probe(&self, _source: &InputSource, sample: &[u8]) -> ProbeResult {
        let Ok(text) = std::str::from_utf8(sample) else {
            return ProbeResult::NoMatch;
        };
        let trimmed = text.trim_start_matches('\u{feff}').trim_start();
        match self.format {
            InputFormat::Json if trimmed.starts_with(['{', '[']) => ProbeResult::Strong,
            InputFormat::Ndjson
                if trimmed
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .count()
                    > 1 =>
            {
                ProbeResult::Possible
            }
            _ => ProbeResult::NoMatch,
        }
    }

    fn open(&self, source: InputSource, options: &OpenOptions) -> anyhow::Result<OpenedSource> {
        options.validate()?;
        if self.format == InputFormat::Ndjson && options.object_mode != ObjectMode::Auto {
            return Err(SourceOptionError::ObjectModeRequiresObject {
                mode: options.object_mode,
            }
            .into());
        }
        if options.preview {
            return preview::open(source, self.format, options);
        }
        if let InputSource::StreamingStdin(input) = &source {
            return open_streaming_json(input.clone(), source.display_name(), self.format, options);
        }
        if let InputSource::Path(path) = &source {
            if std::fs::metadata(path)?.len() >= options.lazy_threshold_bytes {
                match self.format {
                    InputFormat::Json => {
                        return open_lazy_json(path, source.display_name(), options);
                    }
                    InputFormat::Ndjson => {
                        return open_lazy_ndjson(path, source.display_name(), options);
                    }
                    _ => {}
                }
            }
        }
        let bytes = read_source(&source)?;
        match self.format {
            InputFormat::Json => {
                let parsed =
                    parse_json_rows_with_options(&bytes, options.json_path.as_ref(), options)?;
                open_json_rows_with_metadata(
                    parsed.rows,
                    source.display_name(),
                    options,
                    parsed.object_mode,
                    parsed.warnings,
                )
            }
            InputFormat::Ndjson => open_json_rows(
                parse_ndjson_rows(&bytes, options.json_path.as_ref())?,
                source.display_name(),
                options,
            ),
            _ => anyhow::bail!("JSON adapter requires json or ndjson format"),
        }
    }
}

const JSON_SOURCE_SAMPLE_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct JsonSourceFingerprint {
    len: u64,
    sample_hash: u64,
}

fn json_source_fingerprint(path: &Path) -> anyhow::Result<JsonSourceFingerprint> {
    let metadata = std::fs::metadata(path)?;
    let len = metadata.len();
    let mut file = File::open(path)?;
    let mut hasher = DefaultHasher::new();
    len.hash(&mut hasher);
    let mut sample = vec![0_u8; JSON_SOURCE_SAMPLE_BYTES as usize];
    let first_len = file.read(&mut sample)?;
    sample[..first_len].hash(&mut hasher);
    if len > JSON_SOURCE_SAMPLE_BYTES {
        file.seek(SeekFrom::Start(
            len.saturating_sub(JSON_SOURCE_SAMPLE_BYTES),
        ))?;
        let last_len = file.read(&mut sample)?;
        sample[..last_len].hash(&mut hasher);
    }
    Ok(JsonSourceFingerprint {
        len,
        sample_hash: hasher.finish(),
    })
}

#[derive(Debug, Clone)]
struct FlatRow {
    cells: Vec<(String, CellValue, ColumnSourceIdentity)>,
    source_bytes: u64,
}

struct ParsedJsonRows {
    rows: Vec<FlatRow>,
    object_mode: Option<ObjectModeResolution>,
    warnings: Vec<String>,
}

#[cfg(test)]
fn parse_json_rows(bytes: &[u8], pointer: Option<&JsonPointer>) -> anyhow::Result<Vec<FlatRow>> {
    Ok(parse_json_rows_with_options(bytes, pointer, &OpenOptions::default())?.rows)
}

fn parse_json_rows_with_options(
    bytes: &[u8],
    pointer: Option<&JsonPointer>,
    options: &OpenOptions,
) -> anyhow::Result<ParsedJsonRows> {
    match super::streaming_json::select_json_table(bytes, pointer)? {
        SelectedJsonValue::ArrayRows(rows) => {
            let resolution = resolve_selected_shape(
                SelectedValueShape::Array,
                options.object_mode,
                options.object_mode_origin,
                false,
            )?;
            Ok(ParsedJsonRows {
                rows: rows
                    .iter()
                    .map(|raw| serde_json::from_str::<Value>(raw.get()).map_err(Into::into))
                    .map(|value| value.and_then(|value| flatten_row(&value)))
                    .collect::<anyhow::Result<Vec<_>>>()?,
                object_mode: None,
                warnings: resolution.warning.into_iter().collect(),
            })
        }
        SelectedJsonValue::Object(entries) => {
            ensure_unique_object_entries(&entries)?;
            let resolution = resolve_selected_shape(
                SelectedValueShape::Object,
                options.object_mode,
                options.object_mode_origin,
                options.object_mode == ObjectMode::Auto && detect_keyed_object(&entries)?,
            )?;
            let rows = match resolution.table_shape.expect("object table shape") {
                SelectedTableShape::ObjectRecord => vec![flatten_object_record(&entries)?],
                SelectedTableShape::ObjectEntries => project_object_entries(&entries)?,
                SelectedTableShape::ArrayRows => unreachable!("object resolved as array"),
            };
            Ok(ParsedJsonRows {
                rows,
                object_mode: resolution.object_mode,
                warnings: resolution.warning.into_iter().collect(),
            })
        }
        SelectedJsonValue::Scalar => {
            let resolution = resolve_selected_shape(
                SelectedValueShape::Scalar,
                options.object_mode,
                options.object_mode_origin,
                false,
            )?;
            let message = "JSON starting path does not identify a tabular object or array";
            if let Some(warning) = resolution.warning {
                anyhow::bail!("{warning}; {message}");
            }
            anyhow::bail!(message);
        }
    }
}

const OBJECT_DETECTION_MAX_ENTRIES: usize = 64;
const OBJECT_DETECTION_MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum JsonValueKind {
    Null,
    Boolean,
    Number,
    String,
    Array,
    Object,
}

fn json_value_kind(value: &Value) -> JsonValueKind {
    match value {
        Value::Null => JsonValueKind::Null,
        Value::Bool(_) => JsonValueKind::Boolean,
        Value::Number(_) => JsonValueKind::Number,
        Value::String(_) => JsonValueKind::String,
        Value::Array(_) => JsonValueKind::Array,
        Value::Object(_) => JsonValueKind::Object,
    }
}

fn detection_sample(entries: &[RawObjectEntry]) -> anyhow::Result<Vec<Value>> {
    let mut sampled = Vec::new();
    let mut bytes = 0_u64;
    for entry in entries.iter().take(OBJECT_DETECTION_MAX_ENTRIES) {
        sampled.push(serde_json::from_str(entry.value.get())?);
        bytes = bytes.saturating_add(entry.encoded_len());
        if bytes >= OBJECT_DETECTION_MAX_BYTES {
            break;
        }
    }
    Ok(sampled)
}

fn values_are_keyed_object(sampled: &[Value]) -> bool {
    if sampled.len() < 3 || !sampled.iter().all(Value::is_object) {
        return false;
    }
    let mut counts = HashMap::<(String, JsonValueKind), usize>::new();
    for value in sampled {
        let Value::Object(object) = value else {
            return false;
        };
        for (key, value) in object {
            *counts
                .entry((key.clone(), json_value_kind(value)))
                .or_default() += 1;
        }
    }
    let threshold = sampled.len().saturating_mul(3).div_ceil(4);
    counts.values().any(|count| *count >= threshold)
}

fn detect_keyed_object(entries: &[RawObjectEntry]) -> anyhow::Result<bool> {
    Ok(values_are_keyed_object(&detection_sample(entries)?))
}

fn flatten_object_record(entries: &[RawObjectEntry]) -> anyhow::Result<FlatRow> {
    let mut object = serde_json::Map::new();
    for entry in entries {
        object.insert(entry.key.clone(), serde_json::from_str(entry.value.get())?);
    }
    flatten_row(&Value::Object(object))
}

fn ensure_unique_object_entries(entries: &[RawObjectEntry]) -> anyhow::Result<()> {
    let mut seen = HashSet::with_capacity(entries.len());
    for entry in entries {
        if !seen.insert(entry.key.as_str()) {
            anyhow::bail!("duplicate object member key '{}'", entry.key);
        }
    }
    Ok(())
}

fn project_object_entries(entries: &[RawObjectEntry]) -> anyhow::Result<Vec<FlatRow>> {
    entries
        .iter()
        .map(|entry| {
            let value = serde_json::from_str(entry.value.get())?;
            flatten_keyed_entry(&entry.key, &value, entry.encoded_len())
        })
        .collect()
}

fn flatten_keyed_entry(key: &str, value: &Value, source_bytes: u64) -> anyhow::Result<FlatRow> {
    let mut cells = vec![(
        "@key".to_owned(),
        CellValue::Text(key.to_owned()),
        ColumnSourceIdentity::ObjectKey,
    )];
    match value {
        Value::Object(object) => flatten_object(object, "", &mut cells)?,
        value => {
            let path = "/value".to_owned();
            cells.push((
                path.clone(),
                cell_from_value(value)?,
                ColumnSourceIdentity::StructuredPath(path.parse()?),
            ));
        }
    }
    Ok(FlatRow {
        cells,
        source_bytes,
    })
}

fn parse_partial_json_rows(
    bytes: &[u8],
    pointer: Option<&JsonPointer>,
) -> anyhow::Result<Vec<FlatRow>> {
    super::streaming_json::select_partial_json_rows(bytes, pointer)?
        .iter()
        .map(|raw| serde_json::from_str::<Value>(raw.get()).map_err(Into::into))
        .map(|value| value.and_then(|value| flatten_row(&value)))
        .collect()
}

fn parse_ndjson_rows(bytes: &[u8], pointer: Option<&JsonPointer>) -> anyhow::Result<Vec<FlatRow>> {
    let stream = serde_json::Deserializer::from_slice(bytes).into_iter::<Value>();
    let mut rows = Vec::new();
    for document in stream {
        let document = document?;
        let selected = resolve_pointer(&document, pointer)?;
        if !matches!(selected, Value::Object(_) | Value::Array(_)) {
            anyhow::bail!("JSON starting path does not identify an object or array");
        }
        rows.push(flatten_row(selected)?);
    }
    Ok(rows)
}

fn resolve_pointer<'a>(
    root: &'a Value,
    pointer: Option<&JsonPointer>,
) -> anyhow::Result<&'a Value> {
    let Some(pointer) = pointer else {
        return Ok(root);
    };
    let mut selected = root;
    for segment in pointer.segments() {
        selected = match selected {
            Value::Object(object) => object.get(segment),
            Value::Array(array) => segment
                .parse::<usize>()
                .ok()
                .and_then(|index| array.get(index)),
            _ => None,
        }
        .ok_or_else(|| {
            anyhow::anyhow!("JSON starting path '{}' was not found", pointer.as_str())
        })?;
    }
    Ok(selected)
}

fn flatten_row(value: &Value) -> anyhow::Result<FlatRow> {
    let source_bytes = serde_json::to_vec(value)?.len() as u64;
    let mut cells = Vec::new();
    match value {
        Value::Object(object) => flatten_object(object, "", &mut cells)?,
        Value::Array(array) => {
            for (index, value) in array.iter().enumerate() {
                let path = format!("/{index}");
                cells.push((
                    path,
                    cell_from_value(value)?,
                    ColumnSourceIdentity::Positional(index),
                ));
            }
        }
        scalar => cells.push((
            "/0".to_owned(),
            cell_from_value(scalar)?,
            ColumnSourceIdentity::Positional(0),
        )),
    }
    Ok(FlatRow {
        cells,
        source_bytes,
    })
}

fn flatten_object(
    object: &serde_json::Map<String, Value>,
    prefix: &str,
    cells: &mut Vec<(String, CellValue, ColumnSourceIdentity)>,
) -> anyhow::Result<()> {
    for (key, value) in object {
        let path = format!("{prefix}/{}", JsonPointer::encode_segment(key));
        if let Value::Object(nested) = value {
            if nested.is_empty() {
                let pointer: JsonPointer = path.parse()?;
                cells.push((
                    path,
                    CellValue::Json("{}".to_owned()),
                    ColumnSourceIdentity::StructuredPath(pointer),
                ));
            } else {
                flatten_object(nested, &path, cells)?;
            }
        } else {
            let pointer: JsonPointer = path.parse()?;
            cells.push((
                path,
                cell_from_value(value)?,
                ColumnSourceIdentity::StructuredPath(pointer),
            ));
        }
    }
    Ok(())
}

fn cell_from_value(value: &Value) -> anyhow::Result<CellValue> {
    Ok(match value {
        Value::Null => CellValue::Null,
        Value::Bool(value) => CellValue::Boolean(*value),
        Value::Number(value) => value
            .as_i64()
            .map(CellValue::Integer)
            .or_else(|| value.as_f64().map(CellValue::Float))
            .ok_or_else(|| anyhow::anyhow!("JSON number cannot be represented"))?,
        Value::String(value) => CellValue::Text(value.clone()),
        Value::Array(_) | Value::Object(_) => CellValue::Json(serde_json::to_string(value)?),
    })
}

fn open_json_rows(
    rows: Vec<FlatRow>,
    display_name: String,
    options: &OpenOptions,
) -> anyhow::Result<OpenedSource> {
    open_json_rows_with_metadata(rows, display_name, options, None, Vec::new())
}

fn open_json_rows_with_metadata(
    rows: Vec<FlatRow>,
    display_name: String,
    options: &OpenOptions,
    object_mode: Option<ObjectModeResolution>,
    warnings: Vec<String>,
) -> anyhow::Result<OpenedSource> {
    let generation = SourceGeneration::new();
    let mut schema = JsonSchema::new(generation);
    let mut bytes_scanned = 0_u64;
    let initial_rows = match options.schema_scan {
        SchemaScan::Full => rows.len(),
        SchemaScan::Default => {
            let mut count = 0;
            for row in &rows {
                schema.observe(row);
                bytes_scanned = bytes_scanned.saturating_add(row.source_bytes);
                count += 1;
                if bytes_scanned >= options.schema_scan_bytes {
                    break;
                }
            }
            count
        }
    };
    if options.schema_scan == SchemaScan::Full {
        for row in &rows {
            schema.observe(row);
            bytes_scanned = bytes_scanned.saturating_add(row.source_bytes);
        }
    }
    schema.assign_initial_labels();
    let complete = initial_rows >= rows.len();
    let definition = TableDefinition {
        generation,
        columns: schema.columns.clone(),
        schema_state: if complete {
            SchemaState::Complete
        } else {
            SchemaState::Provisional
        },
        relation: RelationMetadata::implicit(display_name, true),
    };
    let store = JsonTableStore {
        generation,
        rows,
        schema,
        indexed_rows: initial_rows,
        bytes_scanned,
        complete,
    };
    Ok(OpenedSource::implicit(OpenedTable {
        generation,
        definition,
        store: Box::new(store),
        object_mode,
        warnings,
    }))
}

fn open_streaming_json(
    input: StreamingInput,
    display_name: String,
    format: InputFormat,
    options: &OpenOptions,
) -> anyhow::Result<OpenedSource> {
    let generation = SourceGeneration::new();
    let mut store = StreamingJsonTable {
        generation,
        input,
        format,
        pointer: options.json_path.clone(),
        options: options.clone(),
        rows: Vec::new(),
        schema: JsonSchema::new(generation),
        indexed_rows: 0,
        bytes_scanned: 0,
        last_bytes: usize::MAX,
        input_complete: false,
        schema_complete: false,
        object_mode: None,
        warnings: Vec::new(),
    };
    store.refresh_rows(options.schema_scan == SchemaScan::Full)?;
    let initial_target = match options.schema_scan {
        SchemaScan::Full => store.rows.len(),
        SchemaScan::Default => {
            let mut count = 0;
            let mut bytes = 0_u64;
            for row in &store.rows {
                count += 1;
                bytes = bytes.saturating_add(row.source_bytes);
                if bytes >= options.schema_scan_bytes {
                    break;
                }
            }
            count
        }
    };
    while store.indexed_rows < initial_target {
        let row = &store.rows[store.indexed_rows];
        store.schema.observe(row);
        store.bytes_scanned = store.bytes_scanned.saturating_add(row.source_bytes);
        store.indexed_rows += 1;
    }
    store.schema.assign_initial_labels();
    store.schema_complete = store.input_complete && store.indexed_rows == store.rows.len();
    let definition = TableDefinition {
        generation,
        columns: store.schema.columns.clone(),
        schema_state: if store.schema_complete {
            SchemaState::Complete
        } else {
            SchemaState::Provisional
        },
        relation: RelationMetadata::implicit(display_name, true),
    };
    Ok(OpenedSource::implicit(OpenedTable {
        generation,
        definition,
        object_mode: store.object_mode,
        warnings: store.warnings.clone(),
        store: Box::new(store),
    }))
}

struct StreamingJsonTable {
    generation: SourceGeneration,
    input: StreamingInput,
    format: InputFormat,
    pointer: Option<JsonPointer>,
    options: OpenOptions,
    rows: Vec<FlatRow>,
    schema: JsonSchema,
    indexed_rows: usize,
    bytes_scanned: u64,
    last_bytes: usize,
    input_complete: bool,
    schema_complete: bool,
    object_mode: Option<ObjectModeResolution>,
    warnings: Vec<String>,
}

impl StreamingJsonTable {
    fn refresh_rows(&mut self, wait_for_completion: bool) -> anyhow::Result<()> {
        let snapshot = self.input.snapshot(wait_for_completion)?;
        if snapshot.bytes.len() == self.last_bytes && snapshot.complete == self.input_complete {
            return Ok(());
        }
        let parsed = parse_available_json_rows(
            &snapshot.bytes,
            snapshot.complete,
            self.format,
            self.pointer.as_ref(),
            &self.options,
        )?;
        if parsed.rows.len() >= self.rows.len() {
            self.rows = parsed.rows;
        }
        if parsed.object_mode.is_some() {
            self.object_mode = parsed.object_mode;
        }
        for warning in parsed.warnings {
            if !self.warnings.contains(&warning) {
                self.warnings.push(warning);
            }
        }
        self.last_bytes = snapshot.bytes.len();
        self.input_complete = snapshot.complete;
        Ok(())
    }

    fn typed_row(&self, index: usize) -> Option<Row> {
        let raw = self.rows.get(index)?;
        Some(typed_row_from_flat(
            self.generation,
            index,
            raw,
            &self.schema,
        ))
    }
}

impl TableStore for StreamingJsonTable {
    fn generation(&self) -> SourceGeneration {
        self.generation
    }

    fn row_count(&self) -> RowCount {
        if self.input_complete {
            RowCount::Exact(self.rows.len())
        } else if self.rows.is_empty() {
            RowCount::Unknown
        } else {
            RowCount::AtLeast(self.rows.len())
        }
    }

    fn column_count(&self) -> usize {
        self.schema.columns.len()
    }

    fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
        Ok(self.typed_row(index.0))
    }

    fn ensure_indexed_through(&mut self, index: RowIndex) -> anyhow::Result<IndexProgress> {
        self.refresh_rows(index.0 == usize::MAX)?;
        let target = if self.input_complete {
            index.0.saturating_add(1).min(self.rows.len())
        } else {
            self.rows.len()
        };
        let mut delta = SchemaDelta::default();
        while self.indexed_rows < target {
            let row = &self.rows[self.indexed_rows];
            let observed = self.schema.observe(row);
            delta.added_columns.extend(observed.added_columns);
            delta.widened_types.extend(observed.widened_types);
            self.bytes_scanned = self.bytes_scanned.saturating_add(row.source_bytes);
            self.indexed_rows += 1;
        }
        if self.input_complete && self.indexed_rows == self.rows.len() && !self.schema_complete {
            self.schema_complete = true;
            delta.completed = true;
        }
        Ok(IndexProgress {
            row_count: self.row_count(),
            schema_delta: delta,
            bytes_scanned: self.bytes_scanned,
        })
    }

    fn scan_rows(
        &mut self,
        request: ScanRequest,
        visitor: &mut dyn RowVisitor,
    ) -> anyhow::Result<ScanProgress> {
        self.refresh_rows(false)?;
        let mut current = request.start.0;
        let mut visited = 0;
        while visited < request.max_rows {
            let Some(row) = self.typed_row(current) else {
                break;
            };
            visited += 1;
            if visitor.visit(RowIndex(current), &row).is_break() {
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
        InMemoryTable::from_rows(
            self.generation,
            (0..self.rows.len())
                .filter_map(|index| self.typed_row(index))
                .collect(),
        )
    }
}

fn parse_available_json_rows(
    bytes: &[u8],
    complete: bool,
    format: InputFormat,
    pointer: Option<&JsonPointer>,
    options: &OpenOptions,
) -> anyhow::Result<ParsedJsonRows> {
    if complete {
        return match format {
            InputFormat::Json => parse_json_rows_with_options(bytes, pointer, options),
            InputFormat::Ndjson => Ok(ParsedJsonRows {
                rows: parse_ndjson_rows(bytes, pointer)?,
                object_mode: None,
                warnings: Vec::new(),
            }),
            _ => anyhow::bail!("JSON adapter requires json or ndjson format"),
        };
    }
    match format {
        InputFormat::Ndjson => {
            let Some(end) = bytes.iter().rposition(|byte| *byte == b'\n') else {
                return Ok(ParsedJsonRows {
                    rows: Vec::new(),
                    object_mode: None,
                    warnings: Vec::new(),
                });
            };
            Ok(ParsedJsonRows {
                rows: parse_ndjson_rows(&bytes[..=end], pointer)?,
                object_mode: None,
                warnings: Vec::new(),
            })
        }
        InputFormat::Json => match parse_json_rows_with_options(bytes, pointer, options) {
            Ok(parsed) => Ok(parsed),
            Err(error)
                if error
                    .downcast_ref::<serde_json::Error>()
                    .is_some_and(serde_json::Error::is_eof) =>
            {
                Ok(ParsedJsonRows {
                    rows: parse_partial_json_rows(bytes, pointer)?,
                    object_mode: None,
                    warnings: Vec::new(),
                })
            }
            Err(error) => Err(error),
        },
        _ => anyhow::bail!("JSON adapter requires json or ndjson format"),
    }
}

#[derive(Debug, Clone)]
struct JsonSchema {
    generation: SourceGeneration,
    columns: Vec<ColumnDefinition>,
    indices: HashMap<String, usize>,
    labels_assigned: bool,
}

impl JsonSchema {
    fn new(generation: SourceGeneration) -> Self {
        Self {
            generation,
            columns: Vec::new(),
            indices: HashMap::new(),
            labels_assigned: false,
        }
    }

    fn observe(&mut self, row: &FlatRow) -> SchemaDelta {
        let mut delta = SchemaDelta::default();
        for (path, value, identity) in &row.cells {
            if let Some(index) = self.indices.get(path).copied() {
                let widened = self.columns[index].source_type.widen(value.logical_type());
                if widened != self.columns[index].source_type {
                    self.columns[index].source_type = widened;
                    delta.widened_types.push(TypeWidening {
                        column: self.columns[index].id,
                        source_type: widened,
                    });
                }
                continue;
            }
            let ordinal = self.columns.len();
            let mut column = ColumnDefinition {
                id: ColumnId {
                    generation: self.generation,
                    ordinal: ordinal as u32,
                },
                source_identity: identity.clone(),
                display_name: path.clone(),
                source_declared_type: None,
                source_type: value.logical_type(),
                type_origin: TypeOrigin::Inferred,
            };
            if self.labels_assigned {
                column.display_name = shortest_nonconflicting_label(
                    path,
                    self.columns
                        .iter()
                        .map(|column| column.display_name.as_str()),
                );
            }
            self.indices.insert(path.clone(), ordinal);
            self.columns.push(column.clone());
            delta.added_columns.push(column);
        }
        delta
    }

    fn assign_initial_labels(&mut self) {
        let paths = self
            .columns
            .iter()
            .filter_map(|column| match &column.source_identity {
                ColumnSourceIdentity::StructuredPath(path) => Some(path.as_str().to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut child_uses_name = false;
        for column in &mut self.columns {
            column.display_name = match &column.source_identity {
                ColumnSourceIdentity::Positional(index) => format!("Column {}", index + 1),
                ColumnSourceIdentity::ObjectKey => continue,
                ColumnSourceIdentity::StructuredPath(path) => {
                    let label =
                        shortest_unique_label(path.as_str(), paths.iter().map(String::as_str));
                    child_uses_name |= label == "name";
                    label
                }
                ColumnSourceIdentity::Delimited { .. } => String::new(),
                ColumnSourceIdentity::RelationColumn { name, .. } => name.clone(),
            };
        }
        if let Some(key) = self
            .columns
            .iter_mut()
            .find(|column| column.source_identity == ColumnSourceIdentity::ObjectKey)
        {
            key.display_name = if child_uses_name { "_key" } else { "name" }.to_owned();
        }
        self.labels_assigned = true;
    }
}

fn source_path(identity: &ColumnSourceIdentity) -> &str {
    identity.canonical_key().unwrap_or("")
}

fn shortest_unique_label<'a>(path: &str, all: impl Iterator<Item = &'a str>) -> String {
    let paths = all.collect::<Vec<_>>();
    let segments = label_segments(path);
    for depth in 1..=segments.len().max(1) {
        let candidate = join_label_suffix(&segments, depth);
        let collisions = paths
            .iter()
            .filter(|other| join_label_suffix(&label_segments(other), depth) == candidate)
            .count();
        if collisions <= 1 {
            return candidate;
        }
    }
    path.to_owned()
}

fn shortest_nonconflicting_label<'a>(path: &str, used: impl Iterator<Item = &'a str>) -> String {
    let used = used.collect::<Vec<_>>();
    let segments = label_segments(path);
    for depth in 1..=segments.len().max(1) {
        let candidate = join_label_suffix(&segments, depth);
        if !used.iter().any(|label| *label == candidate) {
            return candidate;
        }
    }
    path.to_owned()
}

fn label_segments(path: &str) -> Vec<String> {
    path.parse::<JsonPointer>()
        .map(|pointer| {
            pointer
                .segments()
                .iter()
                .map(|segment| friendly_segment(segment))
                .collect()
        })
        .unwrap_or_else(|_| vec![path.to_owned()])
}

fn friendly_segment(segment: &str) -> String {
    if !segment.is_empty()
        && segment
            .chars()
            .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-'))
    {
        segment.to_owned()
    } else {
        format!(
            "[{}]",
            serde_json::to_string(segment).unwrap_or_else(|_| "\"?\"".to_owned())
        )
    }
}

fn join_label_suffix(segments: &[String], depth: usize) -> String {
    let start = segments.len().saturating_sub(depth);
    let mut label = String::new();
    for segment in &segments[start..] {
        if !label.is_empty() && !segment.starts_with('[') {
            label.push('.');
        }
        label.push_str(segment);
    }
    if label.is_empty() {
        "value".to_owned()
    } else {
        label
    }
}

fn open_lazy_json(
    path: &Path,
    display_name: String,
    options: &OpenOptions,
) -> anyhow::Result<OpenedSource> {
    let mut file = File::open(path)?;
    let selected_offset = locate_json_pointer(&mut file, options.json_path.as_ref())?;
    file.seek(SeekFrom::Start(selected_offset))?;
    let selected_kind = read_non_whitespace(&mut file)?
        .ok_or_else(|| anyhow::anyhow!("JSON starting path was not found"))?;
    if selected_kind == b'{' {
        return open_lazy_json_object(path, display_name, options, selected_offset);
    }
    let selected_shape = if selected_kind == b'[' {
        SelectedValueShape::Array
    } else {
        SelectedValueShape::Scalar
    };
    let resolution = resolve_selected_shape(
        selected_shape,
        options.object_mode,
        options.object_mode_origin,
        false,
    )?;
    if selected_shape == SelectedValueShape::Scalar {
        let message = "JSON starting path does not identify a tabular object or array";
        if let Some(warning) = resolution.warning {
            anyhow::bail!("{warning}; {message}");
        }
        anyhow::bail!(message);
    }
    let warnings = resolution.warning.into_iter().collect();

    let generation = SourceGeneration::new();
    let mut store = LazyJsonArrayTable {
        generation,
        path: path.to_path_buf(),
        offsets: Vec::new(),
        schema: JsonSchema::new(generation),
        scan_offset: file.stream_position()?,
        bytes_scanned: 0,
        eof: false,
        fingerprint: json_source_fingerprint(path)?,
    };
    match options.schema_scan {
        SchemaScan::Full => {
            store.index_until(None, Some(u64::MAX))?;
        }
        SchemaScan::Default => {
            store.index_until(None, Some(options.schema_scan_bytes))?;
        }
    }
    store.schema.assign_initial_labels();
    let definition = TableDefinition {
        generation,
        columns: store.schema.columns.clone(),
        schema_state: if store.eof {
            SchemaState::Complete
        } else {
            SchemaState::Provisional
        },
        relation: RelationMetadata::implicit(display_name, true),
    };
    Ok(OpenedSource::implicit(OpenedTable {
        generation,
        definition,
        store: Box::new(store),
        object_mode: None,
        warnings,
    }))
}

fn open_lazy_json_object(
    path: &Path,
    display_name: String,
    options: &OpenOptions,
    selected_offset: u64,
) -> anyhow::Result<OpenedSource> {
    let generation = SourceGeneration::new();
    let mut store = LazyJsonObjectTable {
        generation,
        path: path.to_path_buf(),
        entries: Vec::new(),
        seen_keys: HashSet::new(),
        schema: JsonSchema::new(generation),
        scan_offset: selected_offset.saturating_add(1),
        bytes_scanned: 0,
        eof: false,
        fingerprint: json_source_fingerprint(path)?,
    };

    let auto_entries = if options.object_mode == ObjectMode::Auto {
        let sample = store.index_detection_sample()?;
        values_are_keyed_object(&sample)
    } else {
        false
    };
    let resolution = resolve_selected_shape(
        SelectedValueShape::Object,
        options.object_mode,
        options.object_mode_origin,
        auto_entries,
    )?;
    if resolution.table_shape == Some(SelectedTableShape::ObjectRecord) {
        store.index_until(None, Some(u64::MAX))?;
        let row = store.flattened_object_record()?;
        return open_json_rows_with_metadata(
            vec![row],
            display_name,
            options,
            resolution.object_mode,
            Vec::new(),
        );
    };

    match options.schema_scan {
        SchemaScan::Full => {
            store.index_until(None, Some(u64::MAX))?;
        }
        SchemaScan::Default => {
            store.index_until(None, Some(options.schema_scan_bytes))?;
        }
    }
    store.schema.assign_initial_labels();
    let definition = TableDefinition {
        generation,
        columns: store.schema.columns.clone(),
        schema_state: if store.eof {
            SchemaState::Complete
        } else {
            SchemaState::Provisional
        },
        relation: RelationMetadata::implicit(display_name, true),
    };
    Ok(OpenedSource::implicit(OpenedTable {
        generation,
        definition,
        store: Box::new(store),
        object_mode: resolution.object_mode,
        warnings: Vec::new(),
    }))
}

fn locate_json_pointer(file: &mut File, pointer: Option<&JsonPointer>) -> anyhow::Result<u64> {
    file.seek(SeekFrom::Start(0))?;
    locate_json_segments(file, pointer.map(JsonPointer::segments).unwrap_or_default())
}

fn locate_json_segments(file: &mut File, segments: &[String]) -> anyhow::Result<u64> {
    let value_offset = skip_whitespace(file)?
        .ok_or_else(|| anyhow::anyhow!("JSON starting path was not found"))?;
    if segments.is_empty() {
        return Ok(value_offset);
    }
    file.seek(SeekFrom::Start(value_offset))?;
    match read_byte(file)? {
        Some(b'{') => {
            loop {
                let Some(offset) = skip_whitespace(file)? else {
                    anyhow::bail!("unterminated JSON object");
                };
                file.seek(SeekFrom::Start(offset))?;
                if read_byte(file)? == Some(b'}') {
                    break;
                }
                file.seek(SeekFrom::Start(offset))?;
                let key = read_json_string(file)?;
                expect_json_byte(file, b':')?;
                if key == segments[0] {
                    return locate_json_segments(file, &segments[1..]);
                }
                skip_json_value(file)?;
                match read_non_whitespace(file)? {
                    Some(b',') => continue,
                    Some(b'}') => break,
                    _ => anyhow::bail!("invalid JSON object separator"),
                }
            }
            anyhow::bail!("JSON starting path was not found")
        }
        Some(b'[') => {
            let target = segments[0]
                .parse::<usize>()
                .map_err(|_| anyhow::anyhow!("JSON Pointer array segment is not an index"))?;
            for index in 0..=target {
                let Some(offset) = skip_whitespace(file)? else {
                    anyhow::bail!("JSON starting path was not found");
                };
                file.seek(SeekFrom::Start(offset))?;
                if read_byte(file)? == Some(b']') {
                    anyhow::bail!("JSON starting path was not found");
                }
                file.seek(SeekFrom::Start(offset))?;
                if index == target {
                    return locate_json_segments(file, &segments[1..]);
                }
                skip_json_value(file)?;
                if read_non_whitespace(file)? != Some(b',') {
                    anyhow::bail!("JSON starting path was not found");
                }
            }
            unreachable!()
        }
        _ => anyhow::bail!("JSON starting path was not found"),
    }
}

fn read_byte(file: &mut File) -> anyhow::Result<Option<u8>> {
    let mut byte = [0_u8; 1];
    Ok((file.read(&mut byte)? == 1).then_some(byte[0]))
}

fn skip_whitespace(file: &mut File) -> anyhow::Result<Option<u64>> {
    loop {
        let offset = file.stream_position()?;
        match read_byte(file)? {
            Some(byte) if byte.is_ascii_whitespace() => continue,
            Some(_) => {
                file.seek(SeekFrom::Start(offset))?;
                return Ok(Some(offset));
            }
            None => return Ok(None),
        }
    }
}

fn read_non_whitespace(file: &mut File) -> anyhow::Result<Option<u8>> {
    let Some(offset) = skip_whitespace(file)? else {
        return Ok(None);
    };
    file.seek(SeekFrom::Start(offset))?;
    read_byte(file)
}

fn expect_json_byte(file: &mut File, expected: u8) -> anyhow::Result<()> {
    match read_non_whitespace(file)? {
        Some(actual) if actual == expected => Ok(()),
        _ => anyhow::bail!("invalid JSON syntax"),
    }
}

fn read_json_string(file: &mut File) -> anyhow::Result<String> {
    let start = skip_whitespace(file)?.ok_or_else(|| anyhow::anyhow!("expected JSON string"))?;
    file.seek(SeekFrom::Start(start))?;
    if read_byte(file)? != Some(b'\"') {
        anyhow::bail!("expected JSON object key");
    }
    let mut escaped = false;
    loop {
        let byte = read_byte(file)?.ok_or_else(|| anyhow::anyhow!("unterminated JSON string"))?;
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'\"' {
            break;
        }
    }
    let end = file.stream_position()?;
    let mut encoded = vec![0_u8; end.saturating_sub(start) as usize];
    file.seek(SeekFrom::Start(start))?;
    file.read_exact(&mut encoded)?;
    file.seek(SeekFrom::Start(end))?;
    Ok(serde_json::from_slice(&encoded)?)
}

fn skip_json_value(file: &mut File) -> anyhow::Result<(u64, u64)> {
    let start = skip_whitespace(file)?.ok_or_else(|| anyhow::anyhow!("expected JSON value"))?;
    file.seek(SeekFrom::Start(start))?;
    let first = read_byte(file)?.ok_or_else(|| anyhow::anyhow!("expected JSON value"))?;
    match first {
        b'\"' => {
            let mut escaped = false;
            loop {
                let byte =
                    read_byte(file)?.ok_or_else(|| anyhow::anyhow!("unterminated JSON string"))?;
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'\"' {
                    break;
                }
            }
        }
        b'{' | b'[' => {
            let mut stack = vec![first];
            while !stack.is_empty() {
                let byte = read_byte(file)?
                    .ok_or_else(|| anyhow::anyhow!("unterminated JSON container"))?;
                match byte {
                    b'\"' => {
                        let mut escaped = false;
                        loop {
                            let byte = read_byte(file)?
                                .ok_or_else(|| anyhow::anyhow!("unterminated JSON string"))?;
                            if escaped {
                                escaped = false;
                            } else if byte == b'\\' {
                                escaped = true;
                            } else if byte == b'\"' {
                                break;
                            }
                        }
                    }
                    b'{' | b'[' => stack.push(byte),
                    b'}' if stack.last() == Some(&b'{') => {
                        stack.pop();
                    }
                    b']' if stack.last() == Some(&b'[') => {
                        stack.pop();
                    }
                    _ => {}
                }
            }
        }
        _ => loop {
            let offset = file.stream_position()?;
            match read_byte(file)? {
                Some(byte) if byte.is_ascii_whitespace() || matches!(byte, b',' | b']' | b'}') => {
                    file.seek(SeekFrom::Start(offset))?;
                    break;
                }
                Some(_) => {}
                None => break,
            }
        },
    }
    Ok((start, file.stream_position()?))
}

#[derive(Debug, Clone)]
struct KeyedObjectOffset {
    key: String,
    value_start: u64,
    value_end: u64,
    source_bytes: u64,
}

#[derive(Debug, Clone)]
struct LazyJsonObjectTable {
    generation: SourceGeneration,
    path: PathBuf,
    entries: Vec<KeyedObjectOffset>,
    seen_keys: HashSet<String>,
    schema: JsonSchema,
    scan_offset: u64,
    bytes_scanned: u64,
    eof: bool,
    fingerprint: JsonSourceFingerprint,
}

impl LazyJsonObjectTable {
    fn ensure_source_unchanged(&self) -> anyhow::Result<()> {
        if json_source_fingerprint(&self.path)? != self.fingerprint {
            anyhow::bail!("source changed during incremental access; reload is required");
        }
        Ok(())
    }

    fn read_value(&self, entry: &KeyedObjectOffset) -> anyhow::Result<Value> {
        self.ensure_source_unchanged()?;
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(entry.value_start))?;
        let mut bytes = vec![0_u8; entry.value_end.saturating_sub(entry.value_start) as usize];
        file.read_exact(&mut bytes)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn read_flat_row(&self, index: usize) -> anyhow::Result<Option<FlatRow>> {
        let Some(entry) = self.entries.get(index) else {
            return Ok(None);
        };
        let value = self.read_value(entry)?;
        Ok(Some(flatten_keyed_entry(
            &entry.key,
            &value,
            entry.source_bytes,
        )?))
    }

    fn typed_row(&self, index: usize) -> anyhow::Result<Option<Row>> {
        Ok(self
            .read_flat_row(index)?
            .map(|flat| typed_row_from_flat(self.generation, index, &flat, &self.schema)))
    }

    fn flattened_object_record(&self) -> anyhow::Result<FlatRow> {
        self.ensure_source_unchanged()?;
        let mut object = serde_json::Map::new();
        for entry in &self.entries {
            object.insert(entry.key.clone(), self.read_value(entry)?);
        }
        flatten_row(&Value::Object(object))
    }

    fn index_next(&mut self) -> anyhow::Result<(Option<Value>, SchemaDelta)> {
        if self.eof {
            return Ok((None, SchemaDelta::default()));
        }
        self.ensure_source_unchanged()?;
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(self.scan_offset))?;
        let entry_start = skip_whitespace(&mut file)?
            .ok_or_else(|| anyhow::anyhow!("unterminated selected JSON object"))?;
        file.seek(SeekFrom::Start(entry_start))?;
        if read_byte(&mut file)? == Some(b'}') {
            self.ensure_source_unchanged()?;
            self.scan_offset = file.stream_position()?;
            self.eof = true;
            return Ok((
                None,
                SchemaDelta {
                    completed: true,
                    ..SchemaDelta::default()
                },
            ));
        }

        file.seek(SeekFrom::Start(entry_start))?;
        let key = read_json_string(&mut file)?;
        if self.seen_keys.contains(&key) {
            anyhow::bail!("duplicate object member key '{key}'");
        }
        expect_json_byte(&mut file, b':')?;
        let (value_start, value_end) = skip_json_value(&mut file)?;
        file.seek(SeekFrom::Start(value_start))?;
        let mut deserializer = serde_json::Deserializer::from_reader(&mut file);
        let value = Value::deserialize(&mut deserializer)?;
        let source_bytes = value_end.saturating_sub(entry_start);
        let flat = flatten_keyed_entry(&key, &value, source_bytes)?;

        file.seek(SeekFrom::Start(value_end))?;
        let separator = read_non_whitespace(&mut file)?;
        let next_scan_offset = file.stream_position()?;
        let next_eof = match separator {
            Some(b',') => false,
            Some(b'}') => true,
            _ => anyhow::bail!("invalid selected JSON object separator"),
        };
        self.ensure_source_unchanged()?;

        let mut next_schema = self.schema.clone();
        let mut delta = next_schema.observe(&flat);
        if next_eof {
            delta.completed = true;
        }
        self.schema = next_schema;
        self.seen_keys.insert(key.clone());
        self.entries.push(KeyedObjectOffset {
            key,
            value_start,
            value_end,
            source_bytes,
        });
        self.scan_offset = next_scan_offset;
        self.bytes_scanned = self.bytes_scanned.saturating_add(source_bytes);
        self.eof = next_eof;
        Ok((Some(value), delta))
    }

    fn index_detection_sample(&mut self) -> anyhow::Result<Vec<Value>> {
        let mut values = Vec::new();
        while values.len() < OBJECT_DETECTION_MAX_ENTRIES
            && self.bytes_scanned < OBJECT_DETECTION_MAX_BYTES
            && !self.eof
        {
            if let (Some(value), _) = self.index_next()? {
                values.push(value);
            }
        }
        Ok(values)
    }

    fn index_until(
        &mut self,
        target: Option<usize>,
        byte_limit: Option<u64>,
    ) -> anyhow::Result<SchemaDelta> {
        let mut delta = SchemaDelta::default();
        while !self.eof
            && target.is_none_or(|target| self.entries.len() <= target)
            && byte_limit.is_none_or(|limit| self.bytes_scanned < limit)
        {
            let (_, observed) = self.index_next()?;
            delta.added_columns.extend(observed.added_columns);
            delta.widened_types.extend(observed.widened_types);
            delta.completed |= observed.completed;
        }
        Ok(delta)
    }
}

impl TableStore for LazyJsonObjectTable {
    fn generation(&self) -> SourceGeneration {
        self.generation
    }

    fn row_count(&self) -> RowCount {
        if self.eof {
            RowCount::Exact(self.entries.len())
        } else if self.entries.is_empty() {
            RowCount::Unknown
        } else {
            RowCount::AtLeast(self.entries.len())
        }
    }

    fn column_count(&self) -> usize {
        self.schema.columns.len()
    }

    fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
        self.index_until(Some(index.0), None)?;
        self.typed_row(index.0)
    }

    fn ensure_indexed_through(&mut self, index: RowIndex) -> anyhow::Result<IndexProgress> {
        let delta = self.index_until(Some(index.0), None)?;
        Ok(IndexProgress {
            row_count: self.row_count(),
            schema_delta: delta,
            bytes_scanned: self.bytes_scanned,
        })
    }

    fn scan_rows(
        &mut self,
        request: ScanRequest,
        visitor: &mut dyn RowVisitor,
    ) -> anyhow::Result<ScanProgress> {
        let mut current = request.start.0;
        let mut visited = 0;
        while visited < request.max_rows {
            let Some(row) = self.row(RowIndex(current))? else {
                break;
            };
            visited += 1;
            if visitor.visit(RowIndex(current), &row).is_break() {
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
        let reached_end = self.eof && current >= self.entries.len();
        Ok(ScanProgress {
            visited,
            next: (!reached_end).then_some(RowIndex(current)),
            reached_end,
        })
    }

    fn materialize(&mut self) -> anyhow::Result<InMemoryTable> {
        self.index_until(None, Some(u64::MAX))?;
        InMemoryTable::from_rows(
            self.generation,
            (0..self.entries.len())
                .map(|index| {
                    self.typed_row(index)?
                        .ok_or_else(|| anyhow::anyhow!("indexed keyed-object row is unavailable"))
                })
                .collect::<anyhow::Result<Vec<_>>>()?,
        )
    }
}

#[derive(Debug, Clone)]
struct LazyJsonArrayTable {
    generation: SourceGeneration,
    path: PathBuf,
    offsets: Vec<u64>,
    schema: JsonSchema,
    scan_offset: u64,
    bytes_scanned: u64,
    eof: bool,
    fingerprint: JsonSourceFingerprint,
}

impl LazyJsonArrayTable {
    fn ensure_source_unchanged(&self) -> anyhow::Result<()> {
        if json_source_fingerprint(&self.path)? != self.fingerprint {
            anyhow::bail!("source changed during incremental access; reload is required");
        }
        Ok(())
    }

    fn read_flat_row(&self, index: usize) -> anyhow::Result<Option<FlatRow>> {
        let Some(offset) = self.offsets.get(index).copied() else {
            return Ok(None);
        };
        self.ensure_source_unchanged()?;
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut deserializer = serde_json::Deserializer::from_reader(file);
        let value = Value::deserialize(&mut deserializer)?;
        Ok(Some(flatten_row(&value)?))
    }

    fn typed_row(&self, index: usize) -> anyhow::Result<Option<Row>> {
        Ok(self
            .read_flat_row(index)?
            .map(|flat| typed_row_from_flat(self.generation, index, &flat, &self.schema)))
    }

    fn index_until(
        &mut self,
        target: Option<usize>,
        byte_limit: Option<u64>,
    ) -> anyhow::Result<SchemaDelta> {
        if self.eof
            || target.is_some_and(|target| self.offsets.len() > target)
            || byte_limit.is_some_and(|limit| self.bytes_scanned >= limit)
        {
            return Ok(SchemaDelta::default());
        }
        self.ensure_source_unchanged()?;
        let mut file = File::open(&self.path)?;
        let mut delta = SchemaDelta::default();
        loop {
            if target.is_some_and(|target| self.offsets.len() > target)
                || byte_limit.is_some_and(|limit| self.bytes_scanned >= limit)
            {
                break;
            }
            file.seek(SeekFrom::Start(self.scan_offset))?;
            let Some(start) = skip_whitespace(&mut file)? else {
                anyhow::bail!("unterminated selected JSON array");
            };
            file.seek(SeekFrom::Start(start))?;
            if read_byte(&mut file)? == Some(b']') {
                self.scan_offset = file.stream_position()?;
                self.eof = true;
                delta.completed = true;
                break;
            }
            file.seek(SeekFrom::Start(start))?;
            let (_, end) = skip_json_value(&mut file)?;
            file.seek(SeekFrom::Start(start))?;
            let mut deserializer = serde_json::Deserializer::from_reader(&mut file);
            let value = Value::deserialize(&mut deserializer)?;
            let flat = flatten_row(&value)?;
            let observed = self.schema.observe(&flat);
            delta.added_columns.extend(observed.added_columns);
            delta.widened_types.extend(observed.widened_types);
            self.offsets.push(start);
            self.bytes_scanned = self.bytes_scanned.saturating_add(end.saturating_sub(start));

            file.seek(SeekFrom::Start(end))?;
            match read_non_whitespace(&mut file)? {
                Some(b',') => self.scan_offset = file.stream_position()?,
                Some(b']') => {
                    self.scan_offset = file.stream_position()?;
                    self.eof = true;
                    delta.completed = true;
                }
                _ => anyhow::bail!("invalid selected JSON array separator"),
            }
            self.ensure_source_unchanged()?;
            if self.eof {
                break;
            }
        }
        Ok(delta)
    }
}

impl TableStore for LazyJsonArrayTable {
    fn generation(&self) -> SourceGeneration {
        self.generation
    }

    fn row_count(&self) -> RowCount {
        if self.eof {
            RowCount::Exact(self.offsets.len())
        } else if self.offsets.is_empty() {
            RowCount::Unknown
        } else {
            RowCount::AtLeast(self.offsets.len())
        }
    }

    fn column_count(&self) -> usize {
        self.schema.columns.len()
    }

    fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
        self.index_until(Some(index.0), None)?;
        self.typed_row(index.0)
    }

    fn ensure_indexed_through(&mut self, index: RowIndex) -> anyhow::Result<IndexProgress> {
        let delta = self.index_until(Some(index.0), None)?;
        Ok(IndexProgress {
            row_count: self.row_count(),
            schema_delta: delta,
            bytes_scanned: self.bytes_scanned,
        })
    }

    fn scan_rows(
        &mut self,
        request: ScanRequest,
        visitor: &mut dyn RowVisitor,
    ) -> anyhow::Result<ScanProgress> {
        let mut current = request.start.0;
        let mut visited = 0;
        while visited < request.max_rows {
            let Some(row) = self.row(RowIndex(current))? else {
                break;
            };
            visited += 1;
            if visitor.visit(RowIndex(current), &row).is_break() {
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
        let reached_end = self.eof && current >= self.offsets.len();
        Ok(ScanProgress {
            visited,
            next: (!reached_end).then_some(RowIndex(current)),
            reached_end,
        })
    }

    fn materialize(&mut self) -> anyhow::Result<InMemoryTable> {
        self.index_until(None, Some(u64::MAX))?;
        InMemoryTable::from_rows(
            self.generation,
            (0..self.offsets.len())
                .map(|index| {
                    self.typed_row(index)?
                        .ok_or_else(|| anyhow::anyhow!("indexed JSON row is unavailable"))
                })
                .collect::<anyhow::Result<Vec<_>>>()?,
        )
    }
}

fn open_lazy_ndjson(
    path: &Path,
    display_name: String,
    options: &OpenOptions,
) -> anyhow::Result<OpenedSource> {
    let generation = SourceGeneration::new();
    let mut store = LazyNdjsonTable {
        generation,
        path: path.to_path_buf(),
        pointer: options.json_path.clone(),
        offsets: Vec::new(),
        schema: JsonSchema::new(generation),
        scan_offset: 0,
        eof: false,
        fingerprint: json_source_fingerprint(path)?,
    };
    match options.schema_scan {
        SchemaScan::Full => {
            store.index_until(None, Some(u64::MAX))?;
        }
        SchemaScan::Default => {
            store.index_until(None, Some(options.schema_scan_bytes))?;
        }
    }
    store.schema.assign_initial_labels();
    let definition = TableDefinition {
        generation,
        columns: store.schema.columns.clone(),
        schema_state: if store.eof {
            SchemaState::Complete
        } else {
            SchemaState::Provisional
        },
        relation: RelationMetadata::implicit(display_name, true),
    };
    Ok(OpenedSource::implicit(OpenedTable {
        generation,
        definition,
        store: Box::new(store),
        object_mode: None,
        warnings: Vec::new(),
    }))
}

#[derive(Debug, Clone)]
struct LazyNdjsonTable {
    generation: SourceGeneration,
    path: PathBuf,
    pointer: Option<JsonPointer>,
    offsets: Vec<u64>,
    schema: JsonSchema,
    scan_offset: u64,
    eof: bool,
    fingerprint: JsonSourceFingerprint,
}

impl LazyNdjsonTable {
    fn ensure_source_unchanged(&self) -> anyhow::Result<()> {
        if json_source_fingerprint(&self.path)? != self.fingerprint {
            anyhow::bail!("source changed during incremental access; reload is required");
        }
        Ok(())
    }

    fn index_until(
        &mut self,
        target: Option<usize>,
        byte_limit: Option<u64>,
    ) -> anyhow::Result<SchemaDelta> {
        if self.eof
            || target.is_some_and(|target| self.offsets.len() > target)
            || byte_limit.is_some_and(|limit| self.scan_offset >= limit)
        {
            return Ok(SchemaDelta::default());
        }
        self.ensure_source_unchanged()?;
        let base_offset = self.scan_offset;
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(base_offset))?;
        let mut stream = serde_json::Deserializer::from_reader(file).into_iter::<Value>();
        let mut delta = SchemaDelta::default();

        loop {
            if target.is_some_and(|target| self.offsets.len() > target)
                || byte_limit.is_some_and(|limit| self.scan_offset >= limit)
            {
                break;
            }
            let document_offset = base_offset.saturating_add(stream.byte_offset() as u64);
            let Some(document) = stream.next() else {
                self.scan_offset = base_offset.saturating_add(stream.byte_offset() as u64);
                self.eof = true;
                if !matches!(self.schema.columns.as_slice(), []) {
                    delta.completed = true;
                }
                break;
            };
            let document = document?;
            let selected = resolve_pointer(&document, self.pointer.as_ref())?;
            if !matches!(selected, Value::Object(_) | Value::Array(_)) {
                anyhow::bail!("JSON starting path does not identify an object or array");
            }
            let flat = flatten_row(selected)?;
            let observed = self.schema.observe(&flat);
            delta.added_columns.extend(observed.added_columns);
            delta.widened_types.extend(observed.widened_types);
            self.offsets.push(document_offset);
            self.scan_offset = base_offset.saturating_add(stream.byte_offset() as u64);
            self.ensure_source_unchanged()?;
        }
        Ok(delta)
    }

    fn read_flat_row(&self, index: usize) -> anyhow::Result<Option<FlatRow>> {
        let Some(offset) = self.offsets.get(index).copied() else {
            return Ok(None);
        };
        self.ensure_source_unchanged()?;
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut deserializer = serde_json::Deserializer::from_reader(file);
        let document = Value::deserialize(&mut deserializer)?;
        let selected = resolve_pointer(&document, self.pointer.as_ref())?;
        Ok(Some(flatten_row(selected)?))
    }

    fn typed_row(&self, index: usize) -> anyhow::Result<Option<Row>> {
        Ok(self
            .read_flat_row(index)?
            .map(|flat| typed_row_from_flat(self.generation, index, &flat, &self.schema)))
    }
}

impl TableStore for LazyNdjsonTable {
    fn generation(&self) -> SourceGeneration {
        self.generation
    }

    fn row_count(&self) -> RowCount {
        if self.eof {
            RowCount::Exact(self.offsets.len())
        } else if self.offsets.is_empty() {
            RowCount::Unknown
        } else {
            RowCount::AtLeast(self.offsets.len())
        }
    }

    fn column_count(&self) -> usize {
        self.schema.columns.len()
    }

    fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
        self.index_until(Some(index.0), None)?;
        self.typed_row(index.0)
    }

    fn ensure_indexed_through(&mut self, index: RowIndex) -> anyhow::Result<IndexProgress> {
        let delta = self.index_until(Some(index.0), None)?;
        Ok(IndexProgress {
            row_count: self.row_count(),
            schema_delta: delta,
            bytes_scanned: self.scan_offset,
        })
    }

    fn scan_rows(
        &mut self,
        request: ScanRequest,
        visitor: &mut dyn RowVisitor,
    ) -> anyhow::Result<ScanProgress> {
        let mut current = request.start.0;
        let mut visited = 0;
        while visited < request.max_rows {
            let Some(row) = self.row(RowIndex(current))? else {
                break;
            };
            visited += 1;
            if visitor.visit(RowIndex(current), &row).is_break() {
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
        let reached_end = self.eof && current >= self.offsets.len();
        Ok(ScanProgress {
            visited,
            next: (!reached_end).then_some(RowIndex(current)),
            reached_end,
        })
    }

    fn materialize(&mut self) -> anyhow::Result<InMemoryTable> {
        self.index_until(None, Some(u64::MAX))?;
        InMemoryTable::from_rows(
            self.generation,
            (0..self.offsets.len())
                .map(|index| {
                    self.typed_row(index)?
                        .ok_or_else(|| anyhow::anyhow!("indexed NDJSON row is unavailable"))
                })
                .collect::<anyhow::Result<Vec<_>>>()?,
        )
    }
}

fn typed_row_from_flat(
    generation: SourceGeneration,
    index: usize,
    raw: &FlatRow,
    schema: &JsonSchema,
) -> Row {
    let values = raw
        .cells
        .iter()
        .map(|(path, value, _)| (path.as_str(), value))
        .collect::<HashMap<_, _>>();
    let cells = schema
        .columns
        .iter()
        .map(|column| {
            let path = source_path(&column.source_identity);
            if path.is_empty() {
                match &column.source_identity {
                    ColumnSourceIdentity::Positional(index) => raw
                        .cells
                        .iter()
                        .find(|(_, _, identity)| {
                            identity == &ColumnSourceIdentity::Positional(*index)
                        })
                        .map(|(_, value, _)| value.clone())
                        .unwrap_or(CellValue::Null),
                    _ => CellValue::Null,
                }
            } else {
                values
                    .get(path)
                    .map(|value| (*value).clone())
                    .unwrap_or(CellValue::Null)
            }
        })
        .collect();
    Row::new(
        RowId {
            generation,
            ordinal: index as u64,
        },
        cells,
    )
}

#[derive(Debug, Clone)]
struct JsonTableStore {
    generation: SourceGeneration,
    rows: Vec<FlatRow>,
    schema: JsonSchema,
    indexed_rows: usize,
    bytes_scanned: u64,
    complete: bool,
}

impl JsonTableStore {
    fn typed_row(&self, index: usize) -> Option<Row> {
        let raw = self.rows.get(index)?;
        Some(typed_row_from_flat(
            self.generation,
            index,
            raw,
            &self.schema,
        ))
    }
}

impl TableStore for JsonTableStore {
    fn generation(&self) -> SourceGeneration {
        self.generation
    }

    fn row_count(&self) -> RowCount {
        RowCount::Exact(self.rows.len())
    }

    fn column_count(&self) -> usize {
        self.schema.columns.len()
    }

    fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
        self.ensure_indexed_through(index)?;
        Ok(self.typed_row(index.0))
    }

    fn ensure_indexed_through(&mut self, index: RowIndex) -> anyhow::Result<IndexProgress> {
        let target = index.0.saturating_add(1).min(self.rows.len());
        let mut delta = SchemaDelta::default();
        while self.indexed_rows < target {
            let row = &self.rows[self.indexed_rows];
            let observed = self.schema.observe(row);
            delta.added_columns.extend(observed.added_columns);
            delta.widened_types.extend(observed.widened_types);
            self.bytes_scanned = self.bytes_scanned.saturating_add(row.source_bytes);
            self.indexed_rows += 1;
        }
        if self.indexed_rows == self.rows.len() && !self.complete {
            self.complete = true;
            delta.completed = true;
        }
        Ok(IndexProgress {
            row_count: self.row_count(),
            schema_delta: delta,
            bytes_scanned: self.bytes_scanned,
        })
    }

    fn scan_rows(
        &mut self,
        request: ScanRequest,
        visitor: &mut dyn RowVisitor,
    ) -> anyhow::Result<ScanProgress> {
        let mut current = request.start.0;
        let mut visited = 0;
        while visited < request.max_rows {
            let Some(row) = self.row(RowIndex(current))? else {
                break;
            };
            visited += 1;
            if visitor.visit(RowIndex(current), &row).is_break() {
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
        self.ensure_indexed_through(RowIndex(usize::MAX))?;
        InMemoryTable::from_rows(
            self.generation,
            (0..self.rows.len())
                .filter_map(|index| self.typed_row(index))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::{ObjectModeOrigin, ResolvedObjectMode};
    use crate::table::LogicalType;

    fn object_options(mode: ObjectMode, scan_bytes: u64) -> OpenOptions {
        OpenOptions {
            format: InputFormat::Json,
            object_mode: mode,
            object_mode_origin: if mode == ObjectMode::Auto {
                ObjectModeOrigin::Default
            } else {
                ObjectModeOrigin::Cli
            },
            schema_scan_bytes: scan_bytes,
            ..OpenOptions::default()
        }
    }

    fn open_object(
        input: &str,
        path: Option<&str>,
        mode: ObjectMode,
        scan_bytes: u64,
    ) -> anyhow::Result<OpenedTable> {
        let pointer = path.map(|path| path.parse::<JsonPointer>()).transpose()?;
        let options = object_options(mode, scan_bytes);
        let parsed = parse_json_rows_with_options(input.as_bytes(), pointer.as_ref(), &options)?;
        open_json_rows_with_metadata(
            parsed.rows,
            "data".to_owned(),
            &options,
            parsed.object_mode,
            parsed.warnings,
        )?
        .into_implicit_table()
    }

    fn canonical_keys(table: &OpenedTable) -> Vec<&str> {
        table
            .definition
            .columns
            .iter()
            .filter_map(|column| column.source_identity.canonical_key())
            .collect()
    }

    fn open(input: &str, format: InputFormat, path: Option<&str>, scan_bytes: u64) -> OpenedTable {
        let pointer = path.map(|path| path.parse::<JsonPointer>().unwrap());
        let rows = match format {
            InputFormat::Json => parse_json_rows(input.as_bytes(), pointer.as_ref()).unwrap(),
            InputFormat::Ndjson => parse_ndjson_rows(input.as_bytes(), pointer.as_ref()).unwrap(),
            _ => unreachable!(),
        };
        open_json_rows(
            rows,
            "data".to_owned(),
            &OpenOptions {
                schema_scan_bytes: scan_bytes,
                ..OpenOptions::default()
            },
        )
        .unwrap()
        .into_implicit_table()
        .unwrap()
    }

    #[test]
    fn selects_pointer_flattens_objects_and_preserves_native_values() {
        let mut table = open(
            r#"{"took":1,"hits":{"hits":[{"_source":{"id":1,"ok":true,"empty":"","none":null,"tags":["a"]}}]}}"#,
            InputFormat::Json,
            Some("/hits/hits"),
            u64::MAX,
        );
        let identities = table
            .definition
            .columns
            .iter()
            .map(|column| source_path(&column.source_identity))
            .collect::<Vec<_>>();
        assert_eq!(
            identities,
            [
                "/_source/id",
                "/_source/ok",
                "/_source/empty",
                "/_source/none",
                "/_source/tags"
            ]
        );
        let row = table.store.row(RowIndex(0)).unwrap().unwrap();
        assert_eq!(row.cells[0], CellValue::Integer(1));
        assert_eq!(row.cells[1], CellValue::Boolean(true));
        assert_eq!(row.cells[2], CellValue::Text(String::new()));
        assert_eq!(row.cells[3], CellValue::Null);
        assert_eq!(row.cells[4], CellValue::Json("[\"a\"]".to_owned()));
    }

    #[test]
    fn ndjson_resolves_pointer_per_document_and_array_rows_are_positional() {
        let mut table = open(
            "{\"row\":[1,2]}\n{\"row\":[3,4]}\n",
            InputFormat::Ndjson,
            Some("/row"),
            u64::MAX,
        );
        assert!(matches!(
            table.definition.columns[0].source_identity,
            ColumnSourceIdentity::Positional(0)
        ));
        assert_eq!(
            table.store.row(RowIndex(1)).unwrap().unwrap().cells[1],
            CellValue::Integer(4)
        );
    }

    #[test]
    fn compact_labels_use_shortest_unique_suffix_and_escape_path_like_keys() {
        let table = open(
            r#"[{"customer":{"email":"a"},"billing":{"email":"b"},"a.b":1,"a/b":2}]"#,
            InputFormat::Json,
            None,
            u64::MAX,
        );
        let labels = table
            .definition
            .columns
            .iter()
            .map(|column| column.display_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(labels[0], "customer.email");
        assert_eq!(labels[1], "billing.email");
        assert!(labels[2].starts_with('['));
        assert!(labels[3].starts_with('['));
    }

    #[test]
    fn streaming_json_array_discovers_rows_and_late_columns_before_export() {
        let input = StreamingInput::pending_for_test();
        input.append_for_test(b"[\n{\"a\":1},\n");
        let source = JsonAdapter::json()
            .open(
                InputSource::StreamingStdin(input.clone()),
                &OpenOptions::default(),
            )
            .expect("open streaming JSON");
        let mut table = source.into_implicit_table().expect("table");
        assert_eq!(table.store.row_count(), RowCount::AtLeast(1));
        assert_eq!(
            table.store.row(RowIndex(0)).unwrap().unwrap().cells[0],
            CellValue::Integer(1)
        );

        input.append_for_test(b"{\"a\":2,\"late\":3}]\n");
        input.finish_for_test();
        let progress = table
            .store
            .ensure_indexed_through(RowIndex(usize::MAX))
            .expect("complete stream");
        assert_eq!(progress.row_count, RowCount::Exact(2));
        assert_eq!(progress.schema_delta.added_columns.len(), 1);
        assert_eq!(progress.schema_delta.added_columns[0].display_name, "late");
        assert!(progress.schema_delta.completed);
    }

    #[test]
    fn streaming_nested_json_array_discovers_rows_before_enclosing_document_finishes() {
        let input = StreamingInput::pending_for_test();
        input.append_for_test(b"{\"hits\":{\"hits\":[\n{\"a\":1},\n");
        let options = OpenOptions {
            format: InputFormat::Json,
            json_path: Some("/hits/hits".parse().unwrap()),
            ..OpenOptions::default()
        };
        let source = JsonAdapter::json()
            .open(InputSource::StreamingStdin(input.clone()), &options)
            .expect("open streaming JSON");
        let mut table = source.into_implicit_table().expect("table");
        assert_eq!(table.store.row_count(), RowCount::AtLeast(1));
        assert_eq!(
            table.store.row(RowIndex(0)).unwrap().unwrap().cells[0],
            CellValue::Integer(1)
        );

        input.append_for_test(b"{\"a\":2,\"late\":3}]},\"tail\":true}\n");
        input.finish_for_test();
        let progress = table
            .store
            .ensure_indexed_through(RowIndex(usize::MAX))
            .expect("complete stream");
        assert_eq!(progress.row_count, RowCount::Exact(2));
        assert_eq!(progress.schema_delta.added_columns.len(), 1);
        assert_eq!(progress.schema_delta.added_columns[0].display_name, "late");
        assert!(progress.schema_delta.completed);
    }

    #[test]
    fn bounded_schema_is_provisional_and_late_columns_append_without_renaming() {
        let mut table = open(
            r#"[{"a":1},{"a":2,"late":true}]"#,
            InputFormat::Json,
            None,
            1,
        );
        assert_eq!(table.definition.schema_state, SchemaState::Provisional);
        assert_eq!(table.definition.columns.len(), 1);
        let original_label = table.definition.columns[0].display_name.clone();
        let progress = table.store.ensure_indexed_through(RowIndex(1)).unwrap();
        assert_eq!(progress.schema_delta.added_columns.len(), 1);
        table.definition.apply_delta(progress.schema_delta).unwrap();
        assert_eq!(table.definition.columns[0].display_name, original_label);
        assert_eq!(table.definition.columns[1].display_name, "late");
        assert_eq!(table.definition.schema_state, SchemaState::Complete);
        let first = table.store.row(RowIndex(0)).unwrap().unwrap();
        assert_eq!(first.cells[1], CellValue::Null);
    }

    #[test]
    fn generated_default_100_mib_boundary_is_bounded_and_full_scan_reaches_eof() {
        assert_eq!(crate::ingest::DEFAULT_SCHEMA_SCAN_BYTES, 100 * 1024 * 1024);
        let mut first = flatten_row(&serde_json::json!({"a": 1})).expect("first");
        first.source_bytes = crate::ingest::DEFAULT_SCHEMA_SCAN_BYTES + 1;
        let late = flatten_row(&serde_json::json!({"a": 2, "late": true})).expect("late");

        let mut bounded = open_json_rows(
            vec![first.clone(), late.clone()],
            "generated.json".to_owned(),
            &OpenOptions::default(),
        )
        .expect("bounded open")
        .into_implicit_table()
        .expect("table");
        assert_eq!(bounded.definition.schema_state, SchemaState::Provisional);
        assert_eq!(bounded.definition.columns.len(), 1);
        let stable_label = bounded.definition.columns[0].display_name.clone();
        let progress = bounded
            .store
            .ensure_indexed_through(RowIndex(1))
            .expect("late row");
        bounded
            .definition
            .apply_delta(progress.schema_delta)
            .expect("delta");
        assert_eq!(bounded.definition.columns.len(), 2);
        assert_eq!(bounded.definition.columns[0].display_name, stable_label);

        let full = open_json_rows(
            vec![first, late],
            "generated.json".to_owned(),
            &OpenOptions {
                schema_scan: SchemaScan::Full,
                ..OpenOptions::default()
            },
        )
        .expect("full open")
        .into_implicit_table()
        .expect("table");
        assert_eq!(full.definition.schema_state, SchemaState::Complete);
        assert_eq!(full.definition.columns.len(), 2);
        assert_eq!(full.store.row_count(), RowCount::Exact(2));
    }

    #[test]
    fn full_scan_widens_integer_to_float_and_marks_complete() {
        let rows = parse_json_rows(br#"[{"n":1},{"n":2.5}]"#, None).unwrap();
        let table = open_json_rows(
            rows,
            "data".to_owned(),
            &OpenOptions {
                schema_scan: SchemaScan::Full,
                schema_scan_bytes: 1,
                ..OpenOptions::default()
            },
        )
        .unwrap()
        .into_implicit_table()
        .unwrap();
        assert_eq!(table.definition.schema_state, SchemaState::Complete);
        assert_eq!(table.definition.columns[0].source_type, LogicalType::Float);
    }

    #[test]
    fn missing_and_scalar_pointer_selections_are_errors() {
        let root: Value = serde_json::from_str(r#"{"a":1}"#).unwrap();
        assert!(resolve_pointer(&root, Some(&"/missing".parse().unwrap())).is_err());
        assert!(parse_json_rows(br#"{"a":1}"#, Some(&"/a".parse().unwrap())).is_err());
    }

    #[test]
    fn json_fixture_matrix_covers_supported_row_shapes_and_malformed_input() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sample/json");
        for name in [
            "top-level-object.json",
            "array-of-objects.json",
            "array-of-arrays.json",
            "nested-values.json",
            "mixed-types.json",
            "path-like-keys.json",
        ] {
            let bytes = std::fs::read(root.join(name)).expect("fixture");
            assert!(!parse_json_rows(&bytes, None).expect(name).is_empty());
        }
        let malformed = std::fs::read(root.join("malformed.json")).expect("malformed");
        assert!(parse_json_rows(&malformed, None).is_err());
        let ndjson = std::fs::read(root.join("records.ndjson")).expect("ndjson");
        assert_eq!(parse_ndjson_rows(&ndjson, None).expect("records").len(), 2);
    }

    #[test]
    fn elasticsearch_fixture_exposes_only_row_relative_hit_columns() {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("sample/json/elasticsearch-response.json"),
        )
        .expect("fixture");
        let rows = parse_json_rows(&bytes, Some(&"/hits/hits".parse().unwrap())).expect("hits");
        let table = open_json_rows(rows, "response.json".to_owned(), &OpenOptions::default())
            .unwrap()
            .into_implicit_table()
            .unwrap();
        let identities = table
            .definition
            .columns
            .iter()
            .map(|column| source_path(&column.source_identity))
            .collect::<Vec<_>>();
        assert!(identities.contains(&"/_source/user/id"));
        assert!(identities.contains(&"/_source/user/email"));
        assert!(!identities.iter().any(|path| path.contains("took")));
        assert!(!identities.iter().any(|path| path.contains("total")));
    }

    #[test]
    fn large_ndjson_indexes_document_offsets_and_decodes_random_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("records.ndjson");
        std::fs::write(
            &path,
            " \n{\"id\":1,\"text\":\"line\\nvalue\"}\n  {\"id\":2}\n{\"id\":3}\n",
        )
        .expect("write");
        let options = OpenOptions {
            format: InputFormat::Ndjson,
            lazy_threshold_bytes: 1,
            schema_scan_bytes: 1,
            ..OpenOptions::default()
        };
        let mut table = JsonAdapter::ndjson()
            .open(InputSource::Path(path), &options)
            .expect("open")
            .into_implicit_table()
            .expect("table");

        assert_eq!(table.definition.schema_state, SchemaState::Provisional);
        assert!(matches!(table.store.row_count(), RowCount::AtLeast(1)));
        assert_eq!(
            table.store.row(RowIndex(1)).unwrap().unwrap().cells[0],
            CellValue::Integer(2)
        );
        table
            .store
            .ensure_indexed_through(RowIndex(usize::MAX))
            .expect("finish indexing");
        assert_eq!(table.store.row_count(), RowCount::Exact(3));
        assert_eq!(
            table.store.row(RowIndex(0)).unwrap().unwrap().cells[1],
            CellValue::Text("line\nvalue".to_owned())
        );
    }

    #[test]
    fn large_ndjson_full_schema_scan_reaches_eof_without_retaining_flat_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("records.ndjson");
        std::fs::write(&path, "{\"a\":1}\n{\"late\":true}\n").expect("write");
        let options = OpenOptions {
            format: InputFormat::Ndjson,
            schema_scan: SchemaScan::Full,
            lazy_threshold_bytes: 1,
            schema_scan_bytes: 1,
            ..OpenOptions::default()
        };
        let mut table = JsonAdapter::ndjson()
            .open(InputSource::Path(path), &options)
            .expect("open")
            .into_implicit_table()
            .expect("table");

        assert_eq!(table.definition.schema_state, SchemaState::Complete);
        assert_eq!(table.store.row_count(), RowCount::Exact(2));
        assert_eq!(table.definition.columns.len(), 2);
        assert_eq!(
            table.store.row(RowIndex(0)).unwrap().unwrap().cells[1],
            CellValue::Null
        );
    }

    #[test]
    fn large_ndjson_detects_source_replacement_before_random_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("records.ndjson");
        std::fs::write(&path, "{\"id\":1}\n{\"id\":2}\n").expect("write");
        let options = OpenOptions {
            format: InputFormat::Ndjson,
            lazy_threshold_bytes: 1,
            schema_scan_bytes: 1,
            ..OpenOptions::default()
        };
        let mut table = JsonAdapter::ndjson()
            .open(InputSource::Path(path.clone()), &options)
            .expect("open")
            .into_implicit_table()
            .expect("table");
        let prior = table.store.row_count();

        std::fs::write(&path, "{\"changed\":true}\n").expect("replace");
        let error = table.store.row(RowIndex(1)).expect_err("changed source");
        assert!(error.to_string().contains("reload is required"));
        assert_eq!(table.store.row_count(), prior);
    }

    #[test]
    fn large_selected_json_array_indexes_nested_values_and_ignores_trailing_metadata() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("response.json");
        let padding = " ".repeat(8_193);
        std::fs::write(
            &path,
            format!(
                r#"{{"meta":{{"padding":"{padding}"}},"hits":{{"hits":[
                    {{"id":1,"text":"escaped \" quote","nested":{{"items":[1,{{"x":2}}]}}}},
                    {{"id":2,"text":"second"}},
                    {{"id":3,"late":true}}
                ]}},"tail":{{"must_be_ignored":[1,2,3]}}}}"#
            ),
        )
        .expect("write");
        let options = OpenOptions {
            format: InputFormat::Json,
            json_path: Some("/hits/hits".parse().unwrap()),
            lazy_threshold_bytes: 1,
            schema_scan_bytes: 1,
            ..OpenOptions::default()
        };
        let mut table = JsonAdapter::json()
            .open(InputSource::Path(path), &options)
            .expect("open")
            .into_implicit_table()
            .expect("table");

        assert_eq!(table.definition.schema_state, SchemaState::Provisional);
        assert!(matches!(table.store.row_count(), RowCount::AtLeast(1)));
        let progress = table
            .store
            .ensure_indexed_through(RowIndex(usize::MAX))
            .expect("finish");
        table.definition.apply_delta(progress.schema_delta).unwrap();
        assert_eq!(
            table.store.row(RowIndex(2)).unwrap().unwrap().cells[0],
            CellValue::Integer(3)
        );
        assert_eq!(table.store.row_count(), RowCount::Exact(3));
        assert!(table
            .definition
            .columns
            .iter()
            .any(|column| source_path(&column.source_identity) == "/late"));
        assert!(!table
            .definition
            .columns
            .iter()
            .any(|column| source_path(&column.source_identity).contains("tail")));
    }

    #[test]
    fn large_json_array_full_scan_reaches_eof_without_materialized_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rows.json");
        std::fs::write(&path, "[ {\"a\":1},\n{\"b\":2}, {\"a\":3.5} ]").expect("write");
        let options = OpenOptions {
            format: InputFormat::Json,
            schema_scan: SchemaScan::Full,
            lazy_threshold_bytes: 1,
            schema_scan_bytes: 1,
            ..OpenOptions::default()
        };
        let mut table = JsonAdapter::json()
            .open(InputSource::Path(path), &options)
            .expect("open")
            .into_implicit_table()
            .expect("table");

        assert_eq!(table.definition.schema_state, SchemaState::Complete);
        assert_eq!(table.store.row_count(), RowCount::Exact(3));
        assert_eq!(table.definition.columns[0].source_type, LogicalType::Float);
        assert_eq!(
            table.store.row(RowIndex(1)).unwrap().unwrap().cells[0],
            CellValue::Null
        );
    }

    #[test]
    fn large_json_array_detects_mid_generation_replacement() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rows.json");
        std::fs::write(&path, "[{\"id\":1},{\"id\":2}]").expect("write");
        let options = OpenOptions {
            format: InputFormat::Json,
            lazy_threshold_bytes: 1,
            schema_scan_bytes: 1,
            ..OpenOptions::default()
        };
        let mut table = JsonAdapter::json()
            .open(InputSource::Path(path.clone()), &options)
            .expect("open")
            .into_implicit_table()
            .expect("table");
        let prior_count = table.store.row_count();

        std::fs::write(&path, "[{\"replacement\":true}]").expect("replace");
        let error = table
            .store
            .ensure_indexed_through(RowIndex(1))
            .expect_err("source change");
        assert!(error.to_string().contains("reload is required"));
        assert_eq!(table.store.row_count(), prior_count);
    }

    #[test]
    fn auto_detector_recognizes_representative_keyed_maps() {
        for input in [
            r#"{"repo-a":{"type":"fs","settings":{}},"repo-b":{"type":"s3","settings":{}},"repo-c":{"type":"gcs","settings":{}}}"#,
            r#"{"pipe-a":{"processors":[]},"pipe-b":{"processors":[]},"pipe-c":{"processors":[]}}"#,
            r#"{"index-a":{"settings":{}},"index-b":{"settings":{}},"index-c":{"settings":{}}}"#,
            r#"{"index-a":{"aliases":{}},"index-b":{"aliases":{}},"index-c":{"aliases":{}}}"#,
        ] {
            let table = open_object(input, None, ObjectMode::Auto, u64::MAX).expect("keyed map");
            assert_eq!(
                table.object_mode.expect("resolution").resolved,
                ResolvedObjectMode::Entries
            );
            assert_eq!(table.store.row_count(), RowCount::Exact(3));
            assert_eq!(canonical_keys(&table)[0], "@key");
        }
    }

    #[test]
    fn representative_keyed_object_fixtures_auto_detect_as_entries() {
        for name in [
            "keyed-repositories.json",
            "keyed-pipelines.json",
            "keyed-index-settings.json",
            "keyed-aliases.json",
            "keyed-mappings.json",
            "keyed-nodes.json",
        ] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("sample/json")
                .join(name);
            let options = OpenOptions {
                format: InputFormat::Json,
                ..OpenOptions::default()
            };
            let table = JsonAdapter::json()
                .open(InputSource::Path(path), &options)
                .unwrap_or_else(|error| panic!("open {name}: {error}"))
                .into_implicit_table()
                .expect("table");
            assert_eq!(
                table.object_mode.expect("resolution").resolved,
                ResolvedObjectMode::Entries,
                "fixture {name}"
            );
            assert_eq!(canonical_keys(&table)[0], "@key", "fixture {name}");
        }
    }

    #[test]
    fn auto_detector_keeps_records_for_scalar_small_and_heterogeneous_objects() {
        for input in [
            r#"{"name":"cluster","settings":{"a":1},"enabled":true}"#,
            r#"{"persistent":{"enabled":true},"transient":{"enabled":false}}"#,
            r#"{"one":{"a":1},"two":{"b":2},"three":{"c":3}}"#,
        ] {
            let table = open_object(input, None, ObjectMode::Auto, u64::MAX).expect("record");
            assert_eq!(
                table.object_mode.expect("resolution").resolved,
                ResolvedObjectMode::Record
            );
            assert_eq!(table.store.row_count(), RowCount::Exact(1));
        }
    }

    #[test]
    fn explicit_null_counts_as_present_but_absence_does_not() {
        assert!(values_are_keyed_object(&[
            serde_json::json!({"shared": null}),
            serde_json::json!({"shared": null}),
            serde_json::json!({"shared": null}),
            serde_json::json!({"other": 1}),
        ]));
        assert!(!values_are_keyed_object(&[
            serde_json::json!({"shared": null}),
            serde_json::json!({"shared": null}),
            serde_json::json!({"other": 1}),
            serde_json::json!({"another": 2}),
        ]));
    }

    #[test]
    fn detector_finishes_crossing_entry_and_caps_sample_at_64() {
        let huge = "x".repeat(OBJECT_DETECTION_MAX_BYTES as usize);
        let input = serde_json::json!({
            "first": {"shared": huge},
            "second": {"shared": 2},
            "third": {"shared": 3}
        })
        .to_string();
        let table = open_object(&input, None, ObjectMode::Auto, u64::MAX).expect("huge record");
        assert_eq!(
            table.object_mode.expect("resolution").resolved,
            ResolvedObjectMode::Record
        );

        let mut entries = (0..OBJECT_DETECTION_MAX_ENTRIES)
            .map(|index| format!(r#""k{index}":{{"shared":{index}}}"#))
            .collect::<Vec<_>>();
        entries.push(r#""late":0"#.to_owned());
        let input = format!("{{{}}}", entries.join(","));
        let table = open_object(&input, None, ObjectMode::Auto, u64::MAX).expect("entry cap");
        assert_eq!(
            table.object_mode.expect("resolution").resolved,
            ResolvedObjectMode::Entries
        );
        assert!(canonical_keys(&table).contains(&"/value"));
    }

    #[test]
    fn forced_modes_pointer_order_and_non_object_validation_are_consistent() {
        let input = r#"{"outer":{"a":{"v":1},"b":{"v":2},"c":{"v":3}}}"#;
        let entries =
            open_object(input, Some("/outer"), ObjectMode::Entries, u64::MAX).expect("entries");
        assert_eq!(entries.store.row_count(), RowCount::Exact(3));
        assert_eq!(canonical_keys(&entries), ["@key", "/v"]);

        let record =
            open_object(input, Some("/outer"), ObjectMode::Record, u64::MAX).expect("record");
        assert_eq!(record.store.row_count(), RowCount::Exact(1));
        assert!(canonical_keys(&record).contains(&"/a/v"));

        let options = object_options(ObjectMode::Entries, u64::MAX);
        assert!(parse_json_rows_with_options(br#"[{"a":1}]"#, None, &options).is_err());

        let mut saved_options = options;
        saved_options.object_mode_origin = ObjectModeOrigin::SavedView;
        let array = parse_json_rows_with_options(br#"[{"a":1}]"#, None, &saved_options)
            .expect("saved array falls back");
        assert_eq!(array.object_mode, None);
        assert!(array.warnings[0].contains("selected array"));

        let scalar = br#"{"value":1}"#;
        let pointer: JsonPointer = "/value".parse().unwrap();
        let cli_error = parse_json_rows_with_options(
            scalar,
            Some(&pointer),
            &object_options(ObjectMode::Entries, u64::MAX),
        )
        .err()
        .expect("CLI scalar mode");
        assert!(cli_error
            .to_string()
            .contains("object mode 'entries' requires an input with a selected object/map"));

        let mut saved_scalar = object_options(ObjectMode::Record, u64::MAX);
        saved_scalar.object_mode_origin = ObjectModeOrigin::SavedView;
        let saved_error = parse_json_rows_with_options(scalar, Some(&pointer), &saved_scalar)
            .err()
            .expect("saved scalar mode");
        let message = saved_error.to_string();
        assert!(message.contains("saved object_mode 'record'"));
        assert!(message.contains("was ignored"));
        assert!(message.contains("does not identify a tabular object or array"));
    }

    #[test]
    fn explicit_saved_mode_is_stable_when_auto_would_choose_another_shape() {
        let input = r#"{"first":{"value":1},"second":{"value":2}}"#;
        let automatic = open_object(input, None, ObjectMode::Auto, u64::MAX).expect("auto");
        assert_eq!(
            automatic.object_mode.unwrap().resolved,
            ResolvedObjectMode::Record
        );

        let pointer = None;
        let options = OpenOptions {
            format: InputFormat::Json,
            object_mode: ObjectMode::Entries,
            object_mode_origin: ObjectModeOrigin::SavedView,
            ..OpenOptions::default()
        };
        for _ in 0..2 {
            let parsed = parse_json_rows_with_options(input.as_bytes(), pointer, &options)
                .expect("saved entries");
            assert_eq!(
                parsed.object_mode.unwrap().resolved,
                ResolvedObjectMode::Entries
            );
            assert_eq!(parsed.rows.len(), 2);
        }
    }

    #[test]
    fn keyed_projection_preserves_order_types_paths_and_optional_fields() {
        let mut table = open_object(
            r#"{"a/b":{"id":1,"nested":{"flag":true},"tags":["x"]},"two":{"id":2,"nested":{"flag":false}},"three":{"nested":{"flag":null}}}"#,
            None,
            ObjectMode::Entries,
            u64::MAX,
        )
        .expect("entries");
        assert_eq!(
            canonical_keys(&table),
            ["@key", "/id", "/nested/flag", "/tags"]
        );
        let first = table.store.row(RowIndex(0)).unwrap().unwrap();
        assert_eq!(first.cells[0], CellValue::Text("a/b".to_owned()));
        assert_eq!(first.cells[1], CellValue::Integer(1));
        assert_eq!(first.cells[2], CellValue::Boolean(true));
        assert_eq!(first.cells[3], CellValue::Json("[\"x\"]".to_owned()));
        let third = table.store.row(RowIndex(2)).unwrap().unwrap();
        assert_eq!(third.cells[1], CellValue::Null);
        assert_eq!(third.cells[2], CellValue::Null);
    }

    #[test]
    fn forced_scalar_null_and_array_entries_use_typed_value_column() {
        let mut table = open_object(
            r#"{"number":1,"none":null,"items":[1,2]}"#,
            None,
            ObjectMode::Entries,
            u64::MAX,
        )
        .expect("scalar entries");
        assert_eq!(canonical_keys(&table), ["@key", "/value"]);
        assert_eq!(
            table.store.row(RowIndex(0)).unwrap().unwrap().cells[1],
            CellValue::Integer(1)
        );
        assert_eq!(
            table.store.row(RowIndex(1)).unwrap().unwrap().cells[1],
            CellValue::Null
        );
        assert_eq!(
            table.store.row(RowIndex(2)).unwrap().unwrap().cells[1],
            CellValue::Json("[1,2]".to_owned())
        );
    }

    #[test]
    fn key_label_handles_initial_and_late_name_collisions_without_renaming() {
        let initial = open_object(
            r#"{"a":{"name":"A"},"b":{"name":"B"}}"#,
            None,
            ObjectMode::Entries,
            u64::MAX,
        )
        .expect("initial name");
        assert_eq!(initial.definition.columns[0].display_name, "_key");
        assert_eq!(initial.definition.columns[1].display_name, "name");

        let mut late = open_object(
            r#"{"a":{"id":1},"b":{"name":"B"}}"#,
            None,
            ObjectMode::Entries,
            1,
        )
        .expect("late name");
        assert_eq!(late.definition.columns[0].display_name, "name");
        let delta = late
            .store
            .ensure_indexed_through(RowIndex(1))
            .unwrap()
            .schema_delta;
        late.definition.apply_delta(delta).unwrap();
        assert_eq!(late.definition.columns[0].display_name, "name");
        assert_ne!(late.definition.columns[2].display_name, "name");
    }

    #[test]
    fn materialized_objects_reject_duplicate_keys_before_interpretation() {
        for mode in [ObjectMode::Auto, ObjectMode::Record, ObjectMode::Entries] {
            let error =
                match open_object(r#"{"same":{"v":1},"same":{"v":2}}"#, None, mode, u64::MAX) {
                    Ok(_) => panic!("duplicate key accepted in {mode}"),
                    Err(error) => error,
                };
            assert!(error
                .to_string()
                .contains("duplicate object member key 'same'"));
        }
    }

    fn large_keyed_json(count: usize, duplicate_first_at_end: bool) -> String {
        let mut entries = (0..count)
            .map(|index| {
                if index == 70 {
                    format!(r#""k{index}":{{"shared":{index},"late":true}}"#)
                } else {
                    format!(r#""k{index}":{{"shared":{index}}}"#)
                }
            })
            .collect::<Vec<_>>();
        if duplicate_first_at_end {
            entries.push(r#""k0":{"shared":999}"#.to_owned());
        }
        format!("{{{}}}", entries.join(","))
    }

    #[test]
    fn large_keyed_object_is_bounded_navigable_and_materializable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("repositories.json");
        std::fs::write(&path, large_keyed_json(80, false)).expect("write");
        let options = OpenOptions {
            format: InputFormat::Json,
            lazy_threshold_bytes: 1,
            schema_scan_bytes: 1,
            ..OpenOptions::default()
        };
        let mut table = JsonAdapter::json()
            .open(InputSource::Path(path), &options)
            .expect("open")
            .into_implicit_table()
            .expect("table");
        assert_eq!(
            table.object_mode.expect("resolution").resolved,
            ResolvedObjectMode::Entries
        );
        assert!(matches!(table.store.row_count(), RowCount::AtLeast(64)));
        assert_eq!(table.definition.schema_state, SchemaState::Provisional);
        let progress = table
            .store
            .ensure_indexed_through(RowIndex(70))
            .expect("navigate");
        table.definition.apply_delta(progress.schema_delta).unwrap();
        assert_eq!(
            table.store.row(RowIndex(70)).unwrap().unwrap().cells[0],
            CellValue::Text("k70".to_owned())
        );
        assert!(canonical_keys(&table).contains(&"/late"));
        let materialized = table.store.materialize().expect("materialize");
        assert_eq!(TableStore::row_count(&materialized), RowCount::Exact(80));
        assert_eq!(table.store.row_count(), RowCount::Exact(80));
    }

    #[test]
    fn full_schema_scan_for_keyed_object_reaches_end_and_widens_types() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("full.json");
        std::fs::write(
            &path,
            r#"{"a":{"shared":1},"b":{"shared":2.5},"c":{"shared":3,"late":true}}"#,
        )
        .expect("write");
        let options = OpenOptions {
            format: InputFormat::Json,
            object_mode: ObjectMode::Entries,
            object_mode_origin: ObjectModeOrigin::Cli,
            schema_scan: SchemaScan::Full,
            lazy_threshold_bytes: 1,
            schema_scan_bytes: 1,
            ..OpenOptions::default()
        };
        let mut table = JsonAdapter::json()
            .open(InputSource::Path(path), &options)
            .expect("open")
            .into_implicit_table()
            .expect("table");

        assert_eq!(table.definition.schema_state, SchemaState::Complete);
        assert_eq!(table.store.row_count(), RowCount::Exact(3));
        let shared = table
            .definition
            .columns
            .iter()
            .position(|column| column.source_identity.canonical_key() == Some("/shared"))
            .expect("shared");
        assert_eq!(
            table.definition.columns[shared].source_type,
            LogicalType::Float
        );
        assert!(canonical_keys(&table).contains(&"/late"));
        assert_eq!(
            table.store.row(RowIndex(0)).unwrap().unwrap().cells[2],
            CellValue::Null
        );
    }

    #[test]
    fn lazy_auto_and_record_reject_the_same_duplicate_record_as_materialized() {
        for mode in [ObjectMode::Auto, ObjectMode::Record] {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("duplicate-record.json");
            std::fs::write(&path, r#"{"same":{"value":1},"same":{"value":2}}"#).expect("write");
            let options = OpenOptions {
                format: InputFormat::Json,
                object_mode: mode,
                object_mode_origin: if mode == ObjectMode::Auto {
                    ObjectModeOrigin::Default
                } else {
                    ObjectModeOrigin::Cli
                },
                lazy_threshold_bytes: 1,
                schema_scan_bytes: 1,
                ..OpenOptions::default()
            };
            let error = JsonAdapter::json()
                .open(InputSource::Path(path), &options)
                .err()
                .expect("duplicate rejected");
            assert!(error
                .to_string()
                .contains("duplicate object member key 'same'"));
        }
    }

    #[test]
    fn lazy_scalar_reports_cli_incompatibility_and_ignored_saved_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("scalar.json");
        std::fs::write(&path, "1").expect("write");

        let cli_options = OpenOptions {
            format: InputFormat::Json,
            object_mode: ObjectMode::Entries,
            object_mode_origin: ObjectModeOrigin::Cli,
            lazy_threshold_bytes: 1,
            ..OpenOptions::default()
        };
        let cli_error = JsonAdapter::json()
            .open(InputSource::Path(path.clone()), &cli_options)
            .err()
            .expect("CLI scalar rejected");
        assert!(cli_error
            .to_string()
            .contains("object mode 'entries' requires an input with a selected object/map"));

        let saved_options = OpenOptions {
            object_mode: ObjectMode::Record,
            object_mode_origin: ObjectModeOrigin::SavedView,
            ..cli_options
        };
        let saved_error = JsonAdapter::json()
            .open(InputSource::Path(path), &saved_options)
            .err()
            .expect("saved scalar rejected");
        let message = saved_error.to_string();
        assert!(message.contains("saved object_mode 'record'"));
        assert!(message.contains("was ignored"));
        assert!(message.contains("does not identify a tabular object or array"));
    }

    #[test]
    fn lazy_keyed_offsets_handle_whitespace_escaped_keys_and_nested_values() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested.json");
        std::fs::write(
            &path,
            "{ \n \"a\\/b\" : {\"shared\":1,\"nested\":[1,{\"x\":2}]},\n\"quote\\\"key\":{\"shared\":2},\"third\":{\"shared\":3} }",
        )
        .expect("write");
        let options = OpenOptions {
            format: InputFormat::Json,
            object_mode: ObjectMode::Entries,
            object_mode_origin: ObjectModeOrigin::Cli,
            lazy_threshold_bytes: 1,
            schema_scan_bytes: 1,
            ..OpenOptions::default()
        };
        let mut table = JsonAdapter::json()
            .open(InputSource::Path(path), &options)
            .expect("open")
            .into_implicit_table()
            .unwrap();
        assert_eq!(
            table.store.row(RowIndex(0)).unwrap().unwrap().cells[0],
            CellValue::Text("a/b".to_owned())
        );
        assert_eq!(
            table.store.row(RowIndex(1)).unwrap().unwrap().cells[0],
            CellValue::Text("quote\"key".to_owned())
        );
        table
            .store
            .ensure_indexed_through(RowIndex(usize::MAX))
            .unwrap();
        assert_eq!(table.store.row_count(), RowCount::Exact(3));
    }

    #[test]
    fn lazy_keyed_object_rejects_late_duplicates_without_advancing_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("duplicate.json");
        std::fs::write(&path, large_keyed_json(65, true)).expect("write");
        let options = OpenOptions {
            format: InputFormat::Json,
            lazy_threshold_bytes: 1,
            schema_scan_bytes: 1,
            ..OpenOptions::default()
        };
        let mut table = JsonAdapter::json()
            .open(InputSource::Path(path), &options)
            .expect("open")
            .into_implicit_table()
            .unwrap();
        let prior = table.store.row_count();
        let error = table
            .store
            .ensure_indexed_through(RowIndex(usize::MAX))
            .expect_err("duplicate");
        assert!(error
            .to_string()
            .contains("duplicate object member key 'k0'"));
        assert_ne!(table.store.row_count(), RowCount::Exact(66));
        assert!(matches!(prior, RowCount::AtLeast(_)));
    }

    #[test]
    fn lazy_keyed_object_detects_mid_generation_replacement() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("replace.json");
        std::fs::write(&path, large_keyed_json(80, false)).expect("write");
        let options = OpenOptions {
            format: InputFormat::Json,
            lazy_threshold_bytes: 1,
            schema_scan_bytes: 1,
            ..OpenOptions::default()
        };
        let mut table = JsonAdapter::json()
            .open(InputSource::Path(path.clone()), &options)
            .expect("open")
            .into_implicit_table()
            .unwrap();
        let prior = table.store.row_count();
        std::fs::write(&path, r#"{"replacement":{"shared":1}}"#).expect("replace");
        let error = table.store.row(RowIndex(70)).expect_err("source change");
        assert!(error.to_string().contains("reload is required"));
        assert_eq!(table.store.row_count(), prior);
    }
}
