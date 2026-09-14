mod preview;
pub(crate) use preview::PreviewSourceStore;
mod executor;
mod model;
mod query;

pub use executor::*;
pub use model::*;
pub use query::*;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::future::Future;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use csv::ReaderBuilder;
use unicode_width::UnicodeWidthStr;

use crate::ingest::{decode_input, parse_rows, sniff_delimiter, ParseOptions, Quoting};

const LAZY_FILE_SAMPLE_BYTES: u64 = 64 * 1024;
const LAZY_FORWARD_SCAN_BATCH_ROWS: usize = 256;

pub enum SourceQueryTask {
    Blocking(Box<dyn FnOnce() -> anyhow::Result<SourceResult> + Send + 'static>),
    Async(Pin<Box<dyn Future<Output = anyhow::Result<SourceResult>> + Send + 'static>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFieldMetadata {
    pub name: String,
    pub source_types: Vec<String>,
    pub searchable: bool,
    pub aggregatable: bool,
    pub conflict: bool,
    pub runtime: bool,
    pub nested: bool,
    pub multifield: bool,
}

pub trait TableStore: Send {
    /// Columns explicitly present in a structured source row, before null padding.
    fn present_columns(&self, _row: RowId) -> Option<Vec<usize>> {
        None
    }

    fn generation(&self) -> SourceGeneration;
    fn row_count(&self) -> RowCount;
    fn column_count(&self) -> usize;
    fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>>;
    fn ensure_indexed_through(&mut self, index: RowIndex) -> anyhow::Result<IndexProgress>;
    fn index_and_scan_rows(
        &mut self,
        through: RowIndex,
        request: ScanRequest,
        visitor: &mut dyn RowVisitor,
    ) -> anyhow::Result<IndexScanProgress> {
        let index = self.ensure_indexed_through(through)?;
        let scan = self.scan_rows(request, visitor)?;
        Ok(IndexScanProgress { index, scan })
    }
    fn scan_rows(
        &mut self,
        request: ScanRequest,
        visitor: &mut dyn RowVisitor,
    ) -> anyhow::Result<ScanProgress>;
    fn materialize(&mut self) -> anyhow::Result<InMemoryTable>;
    fn source_capabilities(&self) -> SourceOperationCapabilities {
        SourceOperationCapabilities::default()
    }
    fn active_source_query(&self) -> Option<&SourceQuery> {
        None
    }
    fn has_source_filters(&self) -> bool {
        false
    }
    fn execute_source_query(
        &mut self,
        _query: &SourceQuery,
    ) -> anyhow::Result<SourceQueryExecution> {
        Ok(SourceQueryExecution::Unsupported {
            reason: "this source does not execute source-native queries".to_owned(),
        })
    }
    fn source_query_task(&mut self, _query: SourceQuery) -> anyhow::Result<SourceQueryTask> {
        anyhow::bail!("asynchronous source-query replacement is unavailable for this source")
    }
    fn result_extent(&self) -> Option<ResultExtent> {
        match self.row_count() {
            RowCount::Exact(source_rows) => Some(ResultExtent::Complete { source_rows }),
            RowCount::AtLeast(_) | RowCount::Unknown => None,
        }
    }
    fn query_provenance(&self) -> Option<&NativeQueryArtifact> {
        None
    }
    fn result_is_partial(&self) -> bool {
        false
    }
    fn result_warnings(&self) -> &[String] {
        &[]
    }
    fn source_field_catalog(&self) -> Arc<[SourceFieldMetadata]> {
        Arc::from([])
    }
    fn stable_row_identity(
        &mut self,
        _index: RowIndex,
    ) -> anyhow::Result<Option<StableRowIdentity>> {
        Ok(None)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowCount {
    Exact(usize),
    AtLeast(usize),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexProgress {
    pub row_count: RowCount,
    pub schema_delta: SchemaDelta,
    pub bytes_scanned: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanDirection {
    Forward,
    Reverse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanRequest {
    pub start: RowIndex,
    pub direction: ScanDirection,
    pub max_rows: usize,
}

pub trait RowVisitor {
    fn visit(&mut self, index: RowIndex, row: &Row) -> ControlFlow<()>;
}

impl<F> RowVisitor for F
where
    F: FnMut(RowIndex, &Row) -> ControlFlow<()>,
{
    fn visit(&mut self, index: RowIndex, row: &Row) -> ControlFlow<()> {
        self(index, row)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanProgress {
    pub visited: usize,
    pub next: Option<RowIndex>,
    pub reached_end: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexScanProgress {
    pub index: IndexProgress,
    pub scan: ScanProgress,
}

/// Controls how much of a table participates in a non-query aggregation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReductionScope {
    /// Visit at most the first `usize` rows. The result reports whether that
    /// sample happened to reach the end of the table.
    Sampled(usize),
    /// Continue bounded scans until the selected table reaches its end.
    Exact,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReductionResult<T> {
    pub value: T,
    pub rows_scanned: usize,
    pub complete: bool,
}

/// Fold rows without materializing or cloning the table. This is deliberately
/// separate from `ViewTransform`: widths, profiles and color metadata must not
/// change source order or activate a derived result store.
pub fn scan_fold<T, F>(
    store: &mut dyn TableStore,
    scope: ReductionScope,
    mut value: T,
    mut fold: F,
) -> anyhow::Result<ReductionResult<T>>
where
    F: FnMut(&mut T, RowIndex, &Row),
{
    const CHUNK_ROWS: usize = 4_096;

    let limit = match scope {
        ReductionScope::Sampled(limit) => limit,
        ReductionScope::Exact => usize::MAX,
    };
    let mut rows_scanned = 0_usize;
    let mut next = Some(RowIndex(0));
    let mut complete = false;

    while rows_scanned < limit {
        let Some(start) = next else {
            complete = true;
            break;
        };
        let max_rows = CHUNK_ROWS.min(limit.saturating_sub(rows_scanned));
        if max_rows == 0 {
            break;
        }
        let mut visitor = |index: RowIndex, row: &Row| {
            fold(&mut value, index, row);
            ControlFlow::Continue(())
        };
        let progress = store.scan_rows(
            ScanRequest {
                start,
                direction: ScanDirection::Forward,
                max_rows,
            },
            &mut visitor,
        )?;
        rows_scanned = rows_scanned.saturating_add(progress.visited);
        next = progress.next;
        if progress.reached_end {
            complete = true;
            break;
        }
        if progress.visited == 0 || next == Some(start) {
            anyhow::bail!("table scan made no forward progress");
        }
    }

    Ok(ReductionResult {
        value,
        rows_scanned,
        complete,
    })
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ColumnReductionProfile {
    pub max_width: usize,
    pub width_counts: BTreeMap<usize, usize>,
    pub logical_type: LogicalType,
    pub numeric_min_max: Option<(f64, f64)>,
    pub identifiers: BTreeSet<String>,
}

/// Build the reusable per-column facts consumed by width selection, inferred
/// profiles, automatic gradients and identifier coloring in a single scan.
pub fn reduce_column_profiles(
    store: &mut dyn TableStore,
    scope: ReductionScope,
    header: Option<&[String]>,
) -> anyhow::Result<ReductionResult<Vec<ColumnReductionProfile>>> {
    let column_count = store
        .column_count()
        .max(header.map(<[String]>::len).unwrap_or_default());
    let mut initial = vec![ColumnReductionProfile::default(); column_count];
    if let Some(header) = header {
        for (profile, value) in initial.iter_mut().zip(header) {
            let width = UnicodeWidthStr::width(value.as_str());
            profile.max_width = profile.max_width.max(width);
            *profile.width_counts.entry(width).or_default() += 1;
        }
    }
    scan_fold(store, scope, initial, |profiles, _, row| {
        if profiles.len() < row.cells.len() {
            profiles.resize_with(row.cells.len(), ColumnReductionProfile::default);
        }
        for (profile, value) in profiles.iter_mut().zip(&row.cells) {
            observe_profile(profile, value);
        }
    })
}

fn observe_profile(profile: &mut ColumnReductionProfile, value: &CellValue) {
    let rendered = value.display();
    let width = UnicodeWidthStr::width(rendered.as_ref());
    profile.max_width = profile.max_width.max(width);
    *profile.width_counts.entry(width).or_default() += 1;
    profile.logical_type = profile.logical_type.widen(value.logical_type());
    if !rendered.is_empty() {
        profile.identifiers.insert(rendered.into_owned());
    }
    let numeric = match value {
        CellValue::Integer(value) => Some(*value as f64),
        CellValue::Float(value) => Some(*value),
        _ => None,
    }
    .filter(|value| value.is_finite());
    if let Some(value) = numeric {
        profile.numeric_min_max = Some(match profile.numeric_min_max {
            Some((min, max)) => (min.min(value), max.max(value)),
            None => (value, value),
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultExtent {
    Complete { source_rows: usize },
    Truncated { source_rows: usize, limit: usize },
}

#[derive(Debug, Clone, PartialEq)]
pub enum NativeQueryLanguage {
    Sql,
    Esql,
}

impl std::fmt::Display for NativeQueryLanguage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Sql => "SQL",
            Self::Esql => "ES|QL",
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum NativeQueryParameter {
    Value(SourceOperand),
    Identifier(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct NativeQueryArtifact {
    pub language: NativeQueryLanguage,
    pub base: Option<String>,
    pub logical: String,
    pub parameters: Vec<NativeQueryParameter>,
    pub copyable: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceResultMetadata {
    pub extent: Option<ResultExtent>,
    pub is_partial: bool,
    pub warnings: Vec<String>,
}

pub struct SourceResult {
    pub definition: TableDefinition,
    pub store: Box<dyn TableStore>,
    pub metadata: SourceResultMetadata,
    pub query: Option<NativeQueryArtifact>,
}

impl SourceResult {
    pub fn from_store(definition: TableDefinition, store: Box<dyn TableStore>) -> Self {
        let metadata = SourceResultMetadata {
            extent: store.result_extent(),
            is_partial: store.result_is_partial(),
            warnings: store.result_warnings().to_vec(),
        };
        let query = store.query_provenance().cloned();
        Self {
            definition,
            store,
            metadata,
            query,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StableRowIdentity {
    SqliteRowId(i64),
    PrimaryKey(Vec<CellValue>),
    ElasticsearchDocument { index: String, id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityTransition {
    Preserved,
    ResetUnavailable,
}

pub enum SourceQueryExecution {
    SourceExecuted(SourceResult),
    BoundedLocal(SourceResult),
    Unsupported { reason: String },
}

pub struct FileSourceQueryStore {
    base: Option<Box<dyn TableStore>>,
    shared_base: Option<Arc<Mutex<Box<dyn TableStore>>>>,
    definition: TableDefinition,
    active_query: SourceQuery,
    result: Option<InMemoryTable>,
    extent: Option<ResultExtent>,
}

impl FileSourceQueryStore {
    pub fn passthrough(
        base: Box<dyn TableStore>,
        definition: TableDefinition,
        query: SourceQuery,
    ) -> Self {
        Self {
            base: Some(base),
            shared_base: None,
            definition,
            active_query: query,
            result: None,
            extent: None,
        }
    }

    pub fn execute_initial(
        base: Box<dyn TableStore>,
        definition: &mut TableDefinition,
        query: SourceQuery,
    ) -> anyhow::Result<Self> {
        let base = Arc::new(Mutex::new(base));
        let result = Self::execute_shared(base, definition.clone(), query)?;
        *definition = result.definition.clone();
        Ok(result)
    }

    fn execute_shared(
        base: Arc<Mutex<Box<dyn TableStore>>>,
        mut definition: TableDefinition,
        query: SourceQuery,
    ) -> anyhow::Result<Self> {
        validate_source_query(&definition, &query)?;
        if !query.order_by.is_empty() {
            anyhow::bail!(
                "source sorting is unavailable for streaming delimited and structured files"
            );
        }
        let (result, extent) = execute_streaming_file_query(&base, &mut definition, &query)?;
        Ok(Self {
            base: None,
            shared_base: Some(base),
            definition,
            active_query: query,
            result: Some(result),
            extent: Some(extent),
        })
    }

    fn promote_base(&mut self) -> anyhow::Result<Arc<Mutex<Box<dyn TableStore>>>> {
        if let Some(base) = &self.shared_base {
            return Ok(base.clone());
        }
        let base = self
            .base
            .take()
            .ok_or_else(|| anyhow::anyhow!("file source base store is unavailable"))?;
        let shared = Arc::new(Mutex::new(base));
        self.shared_base = Some(shared.clone());
        Ok(shared)
    }
}

fn execute_streaming_file_query(
    base: &Arc<Mutex<Box<dyn TableStore>>>,
    definition: &mut TableDefinition,
    query: &SourceQuery,
) -> anyhow::Result<(InMemoryTable, ResultExtent)> {
    execute_streaming_file_query_with_chunk_hook(base, definition, query, || {})
}

fn execute_streaming_file_query_with_chunk_hook(
    base: &Arc<Mutex<Box<dyn TableStore>>>,
    definition: &mut TableDefinition,
    query: &SourceQuery,
    mut after_chunk: impl FnMut(),
) -> anyhow::Result<(InMemoryTable, ResultExtent)> {
    const CHUNK: usize = 1_024;
    let limit = query.limit.get();
    let mut matched = Vec::new();
    let mut next = Some(RowIndex(0));
    let mut reached_end = false;
    while matched.len() <= limit {
        let Some(start) = next else {
            reached_end = true;
            break;
        };
        let mut visitor = |_: RowIndex, row: &Row| {
            if query
                .filters
                .iter()
                .all(|filter| file_source_filter_matches(filter, row))
            {
                matched.push(row.clone());
                if matched.len() > limit {
                    return ControlFlow::Break(());
                }
            }
            ControlFlow::Continue(())
        };
        let progress = {
            let mut store = base
                .lock()
                .map_err(|_| anyhow::anyhow!("file source query lock was poisoned"))?;
            store.index_and_scan_rows(
                RowIndex(start.0.saturating_add(CHUNK.saturating_sub(1))),
                ScanRequest {
                    start,
                    direction: ScanDirection::Forward,
                    max_rows: CHUNK,
                },
                &mut visitor,
            )?
        };
        after_chunk();
        definition.apply_delta(progress.index.schema_delta)?;
        next = progress.scan.next;
        if progress.scan.reached_end {
            reached_end = true;
            break;
        }
        if matched.len() > limit {
            break;
        }
        if progress.scan.visited == 0 || next == Some(start) {
            anyhow::bail!("file source filter made no forward progress");
        }
        std::thread::yield_now();
    }
    let truncated = matched.len() > limit;
    matched.truncate(limit);
    let source_rows = matched.len();
    let extent = if truncated || !reached_end {
        ResultExtent::Truncated { source_rows, limit }
    } else {
        ResultExtent::Complete { source_rows }
    };
    Ok((InMemoryTable::from_rows(query.generation, matched)?, extent))
}

fn file_source_filter_matches(filter: &SourceFilter, row: &Row) -> bool {
    if matches!(filter.scope, SourceFilterScope::WholeRecord) {
        let mut record = String::new();
        for (index, value) in row.cells.iter().enumerate() {
            if index > 0 {
                record.push('\t');
            }
            record.push_str(&value.display());
        }
        let record = CellValue::Text(record);
        return match filter.operator {
            SourceFilterOperator::IsNull => false,
            SourceFilterOperator::IsNotNull => true,
            operator => filter
                .operand
                .as_ref()
                .is_some_and(|operand| file_value_matches(&record, operator, operand)),
        };
    }
    let SourceFilterScope::Column(column) = filter.scope else {
        unreachable!("whole-record scope handled above");
    };
    let value = row.cells.get(column.ordinal as usize);
    match filter.operator {
        SourceFilterOperator::IsNull => value.is_none() || matches!(value, Some(CellValue::Null)),
        SourceFilterOperator::IsNotNull => {
            matches!(value, Some(value) if !matches!(value, CellValue::Null))
        }
        operator => {
            let Some(operand) = filter.operand.as_ref() else {
                return false;
            };
            value.is_some_and(|value| file_value_matches(value, operator, operand))
        }
    }
}

fn file_value_matches(
    value: &CellValue,
    operator: SourceFilterOperator,
    operand: &SourceOperand,
) -> bool {
    use std::cmp::Ordering;
    let ordering = match (value, operand) {
        (CellValue::Null, SourceOperand::Null) => Some(Ordering::Equal),
        (CellValue::Boolean(left), SourceOperand::Boolean(right)) => Some(left.cmp(right)),
        (CellValue::Integer(left), SourceOperand::Integer(right)) => Some(left.cmp(right)),
        (CellValue::Integer(left), SourceOperand::Float(right)) => {
            (*left as f64).partial_cmp(right)
        }
        (CellValue::Float(left), SourceOperand::Integer(right)) => {
            left.partial_cmp(&(*right as f64))
        }
        (CellValue::Float(left), SourceOperand::Float(right)) => left.partial_cmp(right),
        (CellValue::Text(left), SourceOperand::Text(right))
        | (CellValue::Json(left), SourceOperand::Text(right)) => Some(left.cmp(right)),
        (CellValue::Binary(left), SourceOperand::Binary(right)) => Some(left.cmp(right)),
        _ => None,
    };
    match operator {
        SourceFilterOperator::Equal => ordering == Some(Ordering::Equal),
        SourceFilterOperator::NotEqual => ordering.is_some_and(|value| value != Ordering::Equal),
        SourceFilterOperator::LessThan => ordering == Some(Ordering::Less),
        SourceFilterOperator::LessThanOrEqual => {
            ordering.is_some_and(|value| value != Ordering::Greater)
        }
        SourceFilterOperator::GreaterThan => ordering == Some(Ordering::Greater),
        SourceFilterOperator::GreaterThanOrEqual => {
            ordering.is_some_and(|value| value != Ordering::Less)
        }
        SourceFilterOperator::Contains => {
            !matches!(operand, SourceOperand::Null)
                && value
                    .display()
                    .contains(source_operand_display(operand).as_ref())
        }
        SourceFilterOperator::Prefix => {
            !matches!(operand, SourceOperand::Null)
                && value
                    .display()
                    .starts_with(source_operand_display(operand).as_ref())
        }
        SourceFilterOperator::IsNull | SourceFilterOperator::IsNotNull => false,
    }
}

fn source_operand_display(value: &SourceOperand) -> std::borrow::Cow<'_, str> {
    match value {
        SourceOperand::Null => std::borrow::Cow::Borrowed(""),
        SourceOperand::Boolean(value) => {
            std::borrow::Cow::Borrowed(if *value { "true" } else { "false" })
        }
        SourceOperand::Integer(value) => std::borrow::Cow::Owned(value.to_string()),
        SourceOperand::Float(value) => std::borrow::Cow::Owned(value.to_string()),
        SourceOperand::Text(value) => std::borrow::Cow::Borrowed(value),
        SourceOperand::Binary(value) => String::from_utf8_lossy(value),
    }
}

impl TableStore for FileSourceQueryStore {
    fn generation(&self) -> SourceGeneration {
        self.definition.generation
    }

    fn row_count(&self) -> RowCount {
        if let Some(result) = &self.result {
            result.row_count()
        } else if let Some(base) = &self.base {
            base.row_count()
        } else {
            self.shared_base
                .as_ref()
                .expect("file source has an owned or shared base")
                .try_lock()
                .map(|store| store.row_count())
                .unwrap_or(RowCount::Unknown)
        }
    }

    fn column_count(&self) -> usize {
        self.result
            .as_ref()
            .map(TableStore::column_count)
            .unwrap_or(self.definition.columns.len())
    }

    fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
        if let Some(result) = &mut self.result {
            result.row(index)
        } else if let Some(base) = &mut self.base {
            base.row(index)
        } else {
            self.shared_base
                .as_ref()
                .expect("file source has an owned or shared base")
                .lock()
                .map_err(|_| anyhow::anyhow!("file source query lock was poisoned"))?
                .row(index)
        }
    }

    fn ensure_indexed_through(&mut self, index: RowIndex) -> anyhow::Result<IndexProgress> {
        if let Some(result) = &mut self.result {
            return result.ensure_indexed_through(index);
        }
        let progress = if let Some(base) = &mut self.base {
            base.ensure_indexed_through(index)?
        } else {
            self.shared_base
                .as_ref()
                .expect("file source has an owned or shared base")
                .lock()
                .map_err(|_| anyhow::anyhow!("file source query lock was poisoned"))?
                .ensure_indexed_through(index)?
        };
        self.definition.apply_delta(progress.schema_delta.clone())?;
        Ok(progress)
    }

    fn scan_rows(
        &mut self,
        request: ScanRequest,
        visitor: &mut dyn RowVisitor,
    ) -> anyhow::Result<ScanProgress> {
        if let Some(result) = &mut self.result {
            result.scan_rows(request, visitor)
        } else if let Some(base) = &mut self.base {
            base.scan_rows(request, visitor)
        } else {
            self.shared_base
                .as_ref()
                .expect("file source has an owned or shared base")
                .lock()
                .map_err(|_| anyhow::anyhow!("file source query lock was poisoned"))?
                .scan_rows(request, visitor)
        }
    }

    fn materialize(&mut self) -> anyhow::Result<InMemoryTable> {
        if let Some(result) = &mut self.result {
            result.materialize()
        } else if let Some(base) = &mut self.base {
            base.materialize()
        } else {
            self.shared_base
                .as_ref()
                .expect("file source has an owned or shared base")
                .lock()
                .map_err(|_| anyhow::anyhow!("file source query lock was poisoned"))?
                .materialize()
        }
    }

    fn source_capabilities(&self) -> SourceOperationCapabilities {
        SourceOperationCapabilities {
            filters: vec![
                SourceFilterOperator::Equal,
                SourceFilterOperator::NotEqual,
                SourceFilterOperator::LessThan,
                SourceFilterOperator::LessThanOrEqual,
                SourceFilterOperator::GreaterThan,
                SourceFilterOperator::GreaterThanOrEqual,
                SourceFilterOperator::Contains,
                SourceFilterOperator::Prefix,
                SourceFilterOperator::IsNull,
                SourceFilterOperator::IsNotNull,
            ],
            sorting: CapabilityStatus::Unavailable {
                reason: "source sorting would require an unbounded file materialization".to_owned(),
            },
            configurable_limit: true,
        }
    }

    fn active_source_query(&self) -> Option<&SourceQuery> {
        Some(&self.active_query)
    }

    fn has_source_filters(&self) -> bool {
        !self.active_query.filters.is_empty()
    }

    fn execute_source_query(
        &mut self,
        query: &SourceQuery,
    ) -> anyhow::Result<SourceQueryExecution> {
        let base = self.promote_base()?;
        let definition = self.definition.clone();
        let store = Box::new(Self::execute_shared(
            base,
            definition.clone(),
            query.clone(),
        )?) as Box<dyn TableStore>;
        Ok(SourceQueryExecution::BoundedLocal(
            SourceResult::from_store(definition, store),
        ))
    }

    fn source_query_task(&mut self, query: SourceQuery) -> anyhow::Result<SourceQueryTask> {
        let base = self.promote_base()?;
        let definition = self.definition.clone();
        Ok(SourceQueryTask::Blocking(Box::new(move || {
            let store = Box::new(Self::execute_shared(base, definition.clone(), query)?)
                as Box<dyn TableStore>;
            Ok(SourceResult::from_store(definition, store))
        })))
    }

    fn result_extent(&self) -> Option<ResultExtent> {
        self.extent.or_else(|| match self.row_count() {
            RowCount::Exact(source_rows) => Some(ResultExtent::Complete { source_rows }),
            RowCount::AtLeast(_) | RowCount::Unknown => None,
        })
    }
}

pub struct OffsetTableStore {
    inner: Box<dyn TableStore>,
    skip: usize,
}

impl OffsetTableStore {
    pub fn new(inner: Box<dyn TableStore>, skip: usize) -> Self {
        Self { inner, skip }
    }

    fn adjusted_count(&self) -> RowCount {
        match self.inner.row_count() {
            RowCount::Exact(count) => RowCount::Exact(count.saturating_sub(self.skip)),
            RowCount::AtLeast(count) => RowCount::AtLeast(count.saturating_sub(self.skip)),
            RowCount::Unknown => RowCount::Unknown,
        }
    }

    fn adjusted_scan_request(&self, request: ScanRequest) -> ScanRequest {
        let max_rows = if request.direction == ScanDirection::Reverse {
            request.max_rows.min(request.start.0.saturating_add(1))
        } else {
            request.max_rows
        };
        ScanRequest {
            start: RowIndex(request.start.0.saturating_add(self.skip)),
            max_rows,
            ..request
        }
    }

    fn adjusted_scan_progress(
        &self,
        mut progress: ScanProgress,
        direction: ScanDirection,
    ) -> ScanProgress {
        if direction == ScanDirection::Reverse
            && progress.next.is_some_and(|index| index.0 < self.skip)
        {
            progress.next = None;
            progress.reached_end = true;
        } else {
            progress.next = progress
                .next
                .map(|index| RowIndex(index.0.saturating_sub(self.skip)));
        }
        progress
    }
}

impl TableStore for OffsetTableStore {
    fn generation(&self) -> SourceGeneration {
        self.inner.generation()
    }

    fn row_count(&self) -> RowCount {
        self.adjusted_count()
    }

    fn column_count(&self) -> usize {
        self.inner.column_count()
    }

    fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
        self.inner.row(RowIndex(index.0.saturating_add(self.skip)))
    }

    fn ensure_indexed_through(&mut self, index: RowIndex) -> anyhow::Result<IndexProgress> {
        let mut progress = self
            .inner
            .ensure_indexed_through(RowIndex(index.0.saturating_add(self.skip)))?;
        progress.row_count = self.adjusted_count();
        Ok(progress)
    }

    fn index_and_scan_rows(
        &mut self,
        through: RowIndex,
        request: ScanRequest,
        visitor: &mut dyn RowVisitor,
    ) -> anyhow::Result<IndexScanProgress> {
        let skip = self.skip;
        let mut shifted =
            |index: RowIndex, row: &Row| visitor.visit(RowIndex(index.0.saturating_sub(skip)), row);
        let mut progress = self.inner.index_and_scan_rows(
            RowIndex(through.0.saturating_add(skip)),
            self.adjusted_scan_request(request),
            &mut shifted,
        )?;
        progress.index.row_count = self.adjusted_count();
        progress.scan = self.adjusted_scan_progress(progress.scan, request.direction);
        Ok(progress)
    }

    fn scan_rows(
        &mut self,
        request: ScanRequest,
        visitor: &mut dyn RowVisitor,
    ) -> anyhow::Result<ScanProgress> {
        let skip = self.skip;
        let mut shifted =
            |index: RowIndex, row: &Row| visitor.visit(RowIndex(index.0.saturating_sub(skip)), row);
        let progress = self
            .inner
            .scan_rows(self.adjusted_scan_request(request), &mut shifted)?;
        Ok(self.adjusted_scan_progress(progress, request.direction))
    }

    fn materialize(&mut self) -> anyhow::Result<InMemoryTable> {
        let materialized = self.inner.materialize()?;
        InMemoryTable::from_rows(
            self.generation(),
            materialized
                .rows()
                .iter()
                .skip(self.skip)
                .cloned()
                .collect(),
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InMemoryTable {
    generation: SourceGeneration,
    rows: Vec<Row>,
    column_count: usize,
}

impl InMemoryTable {
    pub fn new(rows: Vec<Vec<String>>) -> Self {
        Self::from_text_rows(SourceGeneration::new(), rows)
    }

    pub fn from_text_rows(generation: SourceGeneration, rows: Vec<Vec<String>>) -> Self {
        let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
        Self {
            generation,
            rows: rows
                .into_iter()
                .enumerate()
                .map(|(index, cells)| Row::from_text(generation, index, cells))
                .collect(),
            column_count,
        }
    }

    pub fn from_rows(generation: SourceGeneration, rows: Vec<Row>) -> anyhow::Result<Self> {
        if rows.iter().any(|row| row.id.generation != generation) {
            anyhow::bail!("row belongs to a different source generation");
        }
        let column_count = rows.iter().map(|row| row.cells.len()).max().unwrap_or(0);
        Ok(Self {
            generation,
            rows,
            column_count,
        })
    }

    pub fn row_ref(&self, index: RowIndex) -> Option<&Row> {
        self.rows.get(index.0)
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }
}

impl TableStore for InMemoryTable {
    fn generation(&self) -> SourceGeneration {
        self.generation
    }

    fn row_count(&self) -> RowCount {
        RowCount::Exact(self.rows.len())
    }

    fn column_count(&self) -> usize {
        self.column_count
    }

    fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
        Ok(self.rows.get(index.0).cloned())
    }

    fn ensure_indexed_through(&mut self, _index: RowIndex) -> anyhow::Result<IndexProgress> {
        Ok(IndexProgress {
            row_count: self.row_count(),
            schema_delta: SchemaDelta::default(),
            bytes_scanned: 0,
        })
    }

    fn scan_rows(
        &mut self,
        request: ScanRequest,
        visitor: &mut dyn RowVisitor,
    ) -> anyhow::Result<ScanProgress> {
        let mut visited = 0;
        let mut current = request.start.0;
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
        let next = self.rows.get(current).map(|_| RowIndex(current));
        Ok(ScanProgress {
            visited,
            next,
            reached_end: next.is_none(),
        })
    }

    fn materialize(&mut self) -> anyhow::Result<InMemoryTable> {
        Ok(self.clone())
    }
}

#[derive(Debug, Clone)]
/// File-backed delimited table that incrementally indexes parser-provided
/// logical-record offsets.
pub struct LazyFileTable {
    generation: SourceGeneration,
    path: PathBuf,
    offsets: Vec<u64>,
    options: ParseOptions,
    column_count: usize,
    scan_offset: u64,
    eof: bool,
    fingerprint: SourceFingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceFingerprint {
    len: u64,
    change_marker: SourceChangeMarker,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SourceChangeMarker {
    Modified(SystemTime),
    SampleHash(u64),
}

impl LazyFileTable {
    pub fn supports_options(options: &ParseOptions) -> bool {
        ensure_byte_indexed_encoding(options).is_ok()
    }

    pub fn open(path: impl AsRef<Path>, options: ParseOptions) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut options = options;
        if options.encoding.is_none() {
            options.encoding = Some(sniff_file_encoding(&path)?);
        }
        ensure_byte_indexed_encoding(&options)?;
        if options.delimiter.is_none() {
            options.delimiter = Some(sniff_file_delimiter(&path, &options)?);
        }

        Ok(Self {
            generation: SourceGeneration::new(),
            fingerprint: source_fingerprint(&path)?,
            path,
            offsets: Vec::new(),
            options,
            column_count: 0,
            scan_offset: 0,
            eof: false,
        })
    }

    fn ensure_source_unchanged(&self) -> anyhow::Result<()> {
        if source_fingerprint(&self.path)? != self.fingerprint {
            anyhow::bail!("source changed during incremental access; reload is required");
        }
        Ok(())
    }

    fn added_column_delta(&self, previous_column_count: usize) -> SchemaDelta {
        SchemaDelta {
            added_columns: (previous_column_count..self.column_count)
                .map(|ordinal| ColumnDefinition {
                    id: ColumnId {
                        generation: self.generation,
                        ordinal: ordinal as u32,
                    },
                    source_identity: ColumnSourceIdentity::Delimited {
                        ordinal,
                        name: None,
                    },
                    display_name: format!("Column {}", ordinal + 1),
                    source_declared_type: None,
                    source_type: LogicalType::Text,
                    type_origin: TypeOrigin::Declared,
                })
                .collect(),
            ..SchemaDelta::default()
        }
    }

    fn index_through(&mut self, index: RowIndex) -> anyhow::Result<()> {
        if self.eof || self.offsets.len() > index.0 {
            return Ok(());
        }
        self.ensure_source_unchanged()?;
        let base_offset = self.scan_offset;
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(base_offset))?;
        let mut reader = csv_reader_builder(&self.options).from_reader(file);
        let mut record = csv::ByteRecord::new();
        let mut new_offsets = Vec::new();
        let mut new_column_count = self.column_count;
        let mut eof = false;

        while self.offsets.len() + new_offsets.len() <= index.0 {
            if !reader.read_byte_record(&mut record)? {
                eof = true;
                break;
            }
            let relative = byte_record_offset(&record)?;
            new_offsets.push(base_offset + relative);
            new_column_count = new_column_count.max(record.len());
        }
        let new_scan_offset = base_offset + reader.position().byte();
        self.ensure_source_unchanged()?;

        self.offsets.extend(new_offsets);
        self.column_count = new_column_count;
        self.scan_offset = new_scan_offset;
        self.eof = eof;
        Ok(())
    }

    fn scan_unindexed_rows_forward(
        &mut self,
        request: ScanRequest,
        visitor: &mut dyn RowVisitor,
    ) -> anyhow::Result<ScanProgress> {
        debug_assert_eq!(request.direction, ScanDirection::Forward);
        debug_assert_eq!(request.start.0, self.offsets.len());

        if request.max_rows == 0 {
            return Ok(ScanProgress {
                visited: 0,
                next: Some(request.start),
                reached_end: false,
            });
        }
        if self.eof {
            return Ok(ScanProgress {
                visited: 0,
                next: None,
                reached_end: true,
            });
        }

        self.ensure_source_unchanged()?;
        let base_offset = self.scan_offset;
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(base_offset))?;
        let mut reader = csv_reader_builder(&self.options).from_reader(file);
        let mut record = csv::ByteRecord::new();
        let mut new_offsets = Vec::new();
        let mut new_column_count = self.column_count;
        let mut visited = 0;
        let mut eof = false;
        let mut visitor_broke = false;

        while visited < request.max_rows && !eof && !visitor_broke {
            let batch_start = new_offsets.len();
            let batch_rows = request
                .max_rows
                .saturating_sub(visited)
                .min(LAZY_FORWARD_SCAN_BATCH_ROWS);
            let mut buffered_records = Vec::with_capacity(batch_rows);
            while buffered_records.len() < batch_rows {
                if !reader.read_byte_record(&mut record)? {
                    eof = true;
                    break;
                }
                let relative = byte_record_offset(&record)?;
                new_offsets.push(base_offset + relative);
                new_column_count = new_column_count.max(record.len());
                buffered_records.push(record.clone());
            }

            for (offset, record) in buffered_records.iter().enumerate() {
                let index = RowIndex(self.offsets.len() + batch_start + offset);
                let row = row_from_byte_record(
                    record,
                    &self.options,
                    self.generation,
                    index,
                    new_column_count,
                )?;
                visited += 1;
                if visitor.visit(index, &row).is_break() {
                    visitor_broke = true;
                    break;
                }
            }
        }

        let new_scan_offset = base_offset + reader.position().byte();
        self.ensure_source_unchanged()?;
        self.offsets.extend(new_offsets);
        self.column_count = new_column_count;
        self.scan_offset = new_scan_offset;
        self.eof = eof;

        if visitor_broke {
            return Ok(ScanProgress {
                visited,
                next: None,
                reached_end: false,
            });
        }
        Ok(ScanProgress {
            visited,
            next: (!eof).then_some(RowIndex(self.offsets.len())),
            reached_end: eof,
        })
    }

    fn read_row(&self, index: RowIndex) -> anyhow::Result<Option<Row>> {
        let Some(offset) = self.offsets.get(index.0).copied() else {
            return Ok(None);
        };
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut reader = csv_reader_builder(&self.options).from_reader(file);
        let mut record = csv::ByteRecord::new();
        if !reader.read_byte_record(&mut record)? {
            return Ok(None);
        }
        Ok(Some(row_from_byte_record(
            &record,
            &self.options,
            self.generation,
            index,
            self.column_count,
        )?))
    }
}

fn ensure_byte_indexed_encoding(options: &ParseOptions) -> anyhow::Result<()> {
    let Some(encoding) = options.encoding.as_deref() else {
        return Ok(());
    };
    let normalized = encoding.trim().to_ascii_lowercase().replace('_', "-");
    if matches!(normalized.as_str(), "utf-16" | "utf-16le" | "utf-16be") {
        anyhow::bail!(
            "lazy byte-indexed table storage does not support {encoding}; use materialized decoding instead"
        );
    }
    Ok(())
}

impl TableStore for LazyFileTable {
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
        self.column_count
    }

    fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
        self.index_through(index)?;
        self.ensure_source_unchanged()?;
        self.read_row(index)
    }

    fn ensure_indexed_through(&mut self, index: RowIndex) -> anyhow::Result<IndexProgress> {
        let previous_column_count = self.column_count;
        self.index_through(index)?;
        Ok(IndexProgress {
            row_count: self.row_count(),
            schema_delta: self.added_column_delta(previous_column_count),
            bytes_scanned: self.scan_offset,
        })
    }

    fn index_and_scan_rows(
        &mut self,
        through: RowIndex,
        request: ScanRequest,
        visitor: &mut dyn RowVisitor,
    ) -> anyhow::Result<IndexScanProgress> {
        let scan_target = request
            .start
            .0
            .saturating_add(request.max_rows.saturating_sub(1));
        if request.direction == ScanDirection::Forward
            && request.max_rows > 0
            && request.start.0 == self.offsets.len()
            && scan_target >= through.0
        {
            let previous_column_count = self.column_count;
            let scan = self.scan_unindexed_rows_forward(request, visitor)?;
            return Ok(IndexScanProgress {
                index: IndexProgress {
                    row_count: self.row_count(),
                    schema_delta: self.added_column_delta(previous_column_count),
                    bytes_scanned: self.scan_offset,
                },
                scan,
            });
        }

        let index = self.ensure_indexed_through(through)?;
        let scan = self.scan_rows(request, visitor)?;
        Ok(IndexScanProgress { index, scan })
    }

    fn scan_rows(
        &mut self,
        request: ScanRequest,
        visitor: &mut dyn RowVisitor,
    ) -> anyhow::Result<ScanProgress> {
        if request.max_rows == 0 {
            return Ok(ScanProgress {
                visited: 0,
                next: Some(request.start),
                reached_end: false,
            });
        }
        if request.direction == ScanDirection::Forward {
            if request.start.0 == self.offsets.len() {
                return self.scan_unindexed_rows_forward(request, visitor);
            }
            let target = request
                .start
                .0
                .saturating_add(request.max_rows.saturating_sub(1));
            self.index_through(RowIndex(target))?;
            self.ensure_source_unchanged()?;
            let Some(offset) = self.offsets.get(request.start.0).copied() else {
                return Ok(ScanProgress {
                    visited: 0,
                    next: None,
                    reached_end: self.eof,
                });
            };

            let mut file = File::open(&self.path)?;
            file.seek(SeekFrom::Start(offset))?;
            let mut reader = csv_reader_builder(&self.options).from_reader(file);
            let mut record = csv::ByteRecord::new();
            let mut visited = 0;
            let mut current = request.start.0;
            while visited < request.max_rows && current < self.offsets.len() {
                if !reader.read_byte_record(&mut record)? {
                    break;
                }
                let row = row_from_byte_record(
                    &record,
                    &self.options,
                    self.generation,
                    RowIndex(current),
                    self.column_count,
                )?;
                visited += 1;
                current += 1;
                if visitor.visit(RowIndex(current - 1), &row).is_break() {
                    return Ok(ScanProgress {
                        visited,
                        next: None,
                        reached_end: false,
                    });
                }
            }
            self.ensure_source_unchanged()?;
            let reached_end = self.eof && current >= self.offsets.len();
            return Ok(ScanProgress {
                visited,
                next: (!reached_end).then_some(RowIndex(current)),
                reached_end,
            });
        }

        self.index_through(request.start)?;
        self.ensure_source_unchanged()?;
        let Some(_) = self.offsets.get(request.start.0) else {
            return Ok(ScanProgress {
                visited: 0,
                next: None,
                reached_end: self.eof,
            });
        };
        let row_count = request.max_rows.min(request.start.0.saturating_add(1));
        let first = request.start.0.saturating_add(1).saturating_sub(row_count);
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(self.offsets[first]))?;
        let mut reader = csv_reader_builder(&self.options).from_reader(file);
        let mut record = csv::ByteRecord::new();
        let mut rows = Vec::with_capacity(row_count);
        for ordinal in first..=request.start.0 {
            if !reader.read_byte_record(&mut record)? {
                anyhow::bail!("indexed CSV record could not be read at row {ordinal}");
            }
            rows.push(row_from_byte_record(
                &record,
                &self.options,
                self.generation,
                RowIndex(ordinal),
                self.column_count,
            )?);
        }
        self.ensure_source_unchanged()?;

        let mut visited = 0;
        for row in rows.iter().rev() {
            visited += 1;
            if visitor
                .visit(RowIndex(row.id.ordinal as usize), row)
                .is_break()
            {
                return Ok(ScanProgress {
                    visited,
                    next: None,
                    reached_end: false,
                });
            }
        }
        let reached_end = first == 0;
        Ok(ScanProgress {
            visited,
            next: (!reached_end).then_some(RowIndex(first - 1)),
            reached_end,
        })
    }

    fn materialize(&mut self) -> anyhow::Result<InMemoryTable> {
        self.ensure_source_unchanged()?;
        let data = std::fs::read(&self.path)?;
        Ok(InMemoryTable::from_text_rows(
            self.generation,
            parse_rows(&data, &self.options)?,
        ))
    }
}

fn source_fingerprint(path: &Path) -> anyhow::Result<SourceFingerprint> {
    let metadata = std::fs::metadata(path)?;
    let len = metadata.len();
    let change_marker = match metadata.modified() {
        Ok(modified) => SourceChangeMarker::Modified(modified),
        Err(_) => SourceChangeMarker::SampleHash(source_sample_hash(path, len)?),
    };
    Ok(SourceFingerprint { len, change_marker })
}

fn source_sample_hash(path: &Path, len: u64) -> anyhow::Result<u64> {
    let mut file = File::open(path)?;
    let mut hasher = DefaultHasher::new();
    let mut sample = vec![0_u8; LAZY_FILE_SAMPLE_BYTES as usize];
    let first_len = file.read(&mut sample)?;
    sample[..first_len].hash(&mut hasher);
    if len > LAZY_FILE_SAMPLE_BYTES {
        file.seek(SeekFrom::Start(len.saturating_sub(LAZY_FILE_SAMPLE_BYTES)))?;
        let last_len = file.read(&mut sample)?;
        sample[..last_len].hash(&mut hasher);
    }
    Ok(hasher.finish())
}

fn byte_record_offset(record: &csv::ByteRecord) -> anyhow::Result<u64> {
    record
        .position()
        .map(|position| position.byte())
        .ok_or_else(|| anyhow::anyhow!("CSV parser did not report a logical-record byte offset"))
}

fn sniff_file_delimiter(path: &Path, options: &ParseOptions) -> anyhow::Result<u8> {
    let mut sample = Vec::new();
    File::open(path)?
        .take(LAZY_FILE_SAMPLE_BYTES)
        .read_to_end(&mut sample)?;
    let decoded = decode_input(&sample, options.encoding.as_deref())?;
    Ok(sniff_delimiter(&decoded.text).unwrap_or(b','))
}

fn sniff_file_encoding(path: &Path) -> anyhow::Result<String> {
    let mut sample = Vec::new();
    File::open(path)?
        .take(LAZY_FILE_SAMPLE_BYTES)
        .read_to_end(&mut sample)?;
    Ok(decode_input(&sample, None)?.encoding)
}

fn row_from_byte_record(
    record: &csv::ByteRecord,
    options: &ParseOptions,
    generation: SourceGeneration,
    index: RowIndex,
    column_count: usize,
) -> anyhow::Result<Row> {
    let mut cells = record
        .iter()
        .map(|field| decode_input(field, options.encoding.as_deref()).map(|decoded| decoded.text))
        .collect::<Result<Vec<_>, _>>()?;
    cells.resize(column_count, String::new());
    Ok(Row::from_text(generation, index.0, cells))
}

fn csv_reader_builder(options: &ParseOptions) -> ReaderBuilder {
    let mut builder = ReaderBuilder::new();
    builder
        .has_headers(false)
        .flexible(true)
        .delimiter(options.delimiter.unwrap_or(b','))
        .quote(options.quote_char);
    if options.quoting == Some(Quoting::None) {
        builder.quoting(false);
    }
    builder
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ChunkedQueryStore {
        generation: SourceGeneration,
        calls: usize,
    }

    impl TableStore for ChunkedQueryStore {
        fn generation(&self) -> SourceGeneration {
            self.generation
        }

        fn row_count(&self) -> RowCount {
            RowCount::Exact(2)
        }

        fn column_count(&self) -> usize {
            1
        }

        fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
            Ok((index.0 < 2).then(|| {
                Row::new(
                    RowId {
                        generation: self.generation,
                        ordinal: index.0 as u64,
                    },
                    vec![CellValue::Integer(index.0 as i64)],
                )
            }))
        }

        fn ensure_indexed_through(&mut self, _index: RowIndex) -> anyhow::Result<IndexProgress> {
            Ok(IndexProgress {
                row_count: RowCount::Exact(2),
                schema_delta: SchemaDelta::default(),
                bytes_scanned: 0,
            })
        }

        fn index_and_scan_rows(
            &mut self,
            _through: RowIndex,
            _request: ScanRequest,
            visitor: &mut dyn RowVisitor,
        ) -> anyhow::Result<IndexScanProgress> {
            let index = self.calls;
            self.calls += 1;
            let row = self.row(RowIndex(index))?.expect("chunk row");
            let _ = visitor.visit(RowIndex(index), &row);
            Ok(IndexScanProgress {
                index: self.ensure_indexed_through(RowIndex(index))?,
                scan: ScanProgress {
                    visited: 1,
                    next: (index == 0).then_some(RowIndex(1)),
                    reached_end: index > 0,
                },
            })
        }

        fn scan_rows(
            &mut self,
            _request: ScanRequest,
            _visitor: &mut dyn RowVisitor,
        ) -> anyhow::Result<ScanProgress> {
            anyhow::bail!("test store uses combined indexing and scanning")
        }

        fn materialize(&mut self) -> anyhow::Result<InMemoryTable> {
            Ok(InMemoryTable::from_text_rows(self.generation, Vec::new()))
        }
    }

    fn text_row(table: &mut dyn TableStore, index: usize) -> Vec<String> {
        table
            .row(RowIndex(index))
            .expect("row read")
            .expect("row")
            .display_cells()
    }

    fn text_rows(table: &mut dyn TableStore) -> Vec<Vec<String>> {
        table
            .materialize()
            .expect("materialize")
            .rows()
            .iter()
            .map(Row::display_cells)
            .collect()
    }

    fn finish_index(table: &mut dyn TableStore) {
        table
            .ensure_indexed_through(RowIndex(usize::MAX))
            .expect("finish indexing");
    }

    #[test]
    fn stores_rectangular_rows() {
        let mut table = InMemoryTable::new(vec![vec!["a".to_owned(), "b".to_owned()]]);
        assert_eq!(table.row_count(), RowCount::Exact(1));
        assert_eq!(table.column_count(), 2);
        assert_eq!(text_row(&mut table, 0), ["a", "b"]);
    }

    #[test]
    fn file_source_null_filters_treat_missing_late_cells_as_null() {
        let generation = SourceGeneration::new();
        let row = Row::new(
            RowId {
                generation,
                ordinal: 0,
            },
            vec![CellValue::Text("early row".to_owned())],
        );
        let filter = |operator| SourceFilter {
            scope: SourceFilterScope::Column(ColumnId {
                generation,
                ordinal: 1,
            }),
            operator,
            operand: None,
        };

        assert!(file_source_filter_matches(
            &filter(SourceFilterOperator::IsNull),
            &row
        ));
        assert!(!file_source_filter_matches(
            &filter(SourceFilterOperator::IsNotNull),
            &row
        ));
    }

    #[test]
    fn file_source_text_filters_do_not_treat_null_as_an_empty_wildcard() {
        let generation = SourceGeneration::new();
        let row = Row::new(
            RowId {
                generation,
                ordinal: 0,
            },
            vec![CellValue::Text("alpha".to_owned())],
        );
        let filter = |scope, operator, operand| SourceFilter {
            scope,
            operator,
            operand: Some(operand),
        };
        let column = SourceFilterScope::Column(ColumnId {
            generation,
            ordinal: 0,
        });

        assert!(!file_source_filter_matches(
            &filter(
                column.clone(),
                SourceFilterOperator::Contains,
                SourceOperand::Null
            ),
            &row
        ));
        assert!(!file_source_filter_matches(
            &filter(column, SourceFilterOperator::Prefix, SourceOperand::Null),
            &row
        ));
        assert!(file_source_filter_matches(
            &filter(
                SourceFilterScope::WholeRecord,
                SourceFilterOperator::Contains,
                SourceOperand::Text("alpha".to_owned())
            ),
            &row
        ));
    }

    #[test]
    fn file_query_releases_the_base_store_after_each_chunk() {
        let generation = SourceGeneration::new();
        let base: Arc<Mutex<Box<dyn TableStore>>> =
            Arc::new(Mutex::new(Box::new(ChunkedQueryStore {
                generation,
                calls: 0,
            })));
        let definition = TableDefinition {
            generation,
            columns: vec![ColumnDefinition {
                id: ColumnId {
                    generation,
                    ordinal: 0,
                },
                source_identity: ColumnSourceIdentity::Positional(0),
                display_name: "value".to_owned(),
                source_declared_type: None,
                source_type: LogicalType::Integer,
                type_origin: TypeOrigin::Declared,
            }],
            schema_state: SchemaState::Complete,
            relation: RelationMetadata::implicit("test", true),
        };
        let query = SourceQuery::new(generation, std::num::NonZeroUsize::new(10).unwrap());
        let mut definition = definition;
        let mut observed_chunks = 0;

        execute_streaming_file_query_with_chunk_hook(&base, &mut definition, &query, || {
            let _store = base
                .try_lock()
                .expect("base store must be unlocked between chunks");
            observed_chunks += 1;
        })
        .expect("query result");

        assert_eq!(observed_chunks, 2);
    }

    #[test]
    fn passthrough_file_source_promotes_to_shared_storage_only_for_async_queries() {
        let generation = SourceGeneration::new();
        let definition = TableDefinition {
            generation,
            columns: vec![ColumnDefinition {
                id: ColumnId {
                    generation,
                    ordinal: 0,
                },
                source_identity: ColumnSourceIdentity::Positional(0),
                display_name: "value".to_owned(),
                source_declared_type: None,
                source_type: LogicalType::Text,
                type_origin: TypeOrigin::Declared,
            }],
            schema_state: SchemaState::Complete,
            relation: RelationMetadata::implicit("test", true),
        };
        let query = SourceQuery::new(
            generation,
            std::num::NonZeroUsize::new(10).expect("non-zero limit"),
        );
        let base = Box::new(InMemoryTable::from_text_rows(
            generation,
            vec![vec!["value".to_owned()]],
        ));
        let mut store = FileSourceQueryStore::passthrough(base, definition, query.clone());

        assert!(store.base.is_some());
        assert!(store.shared_base.is_none());
        assert_eq!(
            store.row(RowIndex(0)).expect("direct row").unwrap().cells[0],
            CellValue::Text("value".to_owned())
        );
        assert!(store.base.is_some());

        let _task = store
            .source_query_task(query)
            .expect("background source query");
        assert!(store.base.is_none());
        assert!(store.shared_base.is_some());
    }

    #[test]
    fn offset_table_reverse_scans_do_not_cross_skipped_rows() {
        let rows = vec![
            vec!["header".to_owned()],
            vec!["a".to_owned()],
            vec!["b".to_owned()],
            vec!["c".to_owned()],
        ];
        let request = ScanRequest {
            start: RowIndex(2),
            direction: ScanDirection::Reverse,
            max_rows: usize::MAX,
        };

        let mut scan_store = OffsetTableStore::new(Box::new(InMemoryTable::new(rows.clone())), 1);
        let mut scanned = Vec::new();
        let mut collect = |index: RowIndex, row: &Row| {
            scanned.push((index, row.display_cells()));
            ControlFlow::Continue(())
        };
        let progress = scan_store
            .scan_rows(request, &mut collect)
            .expect("reverse scan");
        assert_eq!(
            scanned,
            [
                (RowIndex(2), vec!["c".to_owned()]),
                (RowIndex(1), vec!["b".to_owned()]),
                (RowIndex(0), vec!["a".to_owned()]),
            ]
        );
        assert_eq!(progress.visited, 3);
        assert_eq!(progress.next, None);
        assert!(progress.reached_end);

        let mut combined_store = OffsetTableStore::new(Box::new(InMemoryTable::new(rows)), 1);
        let mut scanned = Vec::new();
        let mut collect = |index: RowIndex, row: &Row| {
            scanned.push((index, row.display_cells()));
            ControlFlow::Continue(())
        };
        let progress = combined_store
            .index_and_scan_rows(RowIndex(2), request, &mut collect)
            .expect("indexed reverse scan");
        assert_eq!(scanned.len(), 3);
        assert_eq!(progress.scan.next, None);
        assert!(progress.scan.reached_end);
    }

    #[test]
    fn in_memory_table_tracks_max_column_count() {
        let mut table = InMemoryTable::new(vec![
            vec!["a".to_owned()],
            vec!["1".to_owned(), "2".to_owned(), "3".to_owned()],
        ]);

        assert_eq!(table.row_count(), RowCount::Exact(2));
        assert_eq!(table.column_count(), 3);
        let progress = table
            .ensure_indexed_through(RowIndex(100))
            .expect("no-op indexing");
        assert_eq!(progress.row_count, RowCount::Exact(2));
        assert_eq!(table.materialize().expect("materialize").rows().len(), 2);
    }

    #[test]
    fn lazy_file_table_reads_rows_by_offset_and_materializes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data.csv");
        std::fs::write(&path, "a,b\n1,2\n").expect("write");
        let mut table = LazyFileTable::open(&path, ParseOptions::default()).expect("lazy table");

        assert_eq!(table.row_count(), RowCount::Unknown);
        assert_eq!(text_row(&mut table, 1), ["1", "2"]);
        assert_eq!(table.row_count(), RowCount::AtLeast(2));
        assert_eq!(table.column_count(), 2);
        assert_eq!(
            text_rows(&mut table),
            vec![
                vec!["a".to_owned(), "b".to_owned()],
                vec!["1".to_owned(), "2".to_owned()]
            ]
        );
    }

    #[test]
    fn lazy_file_table_indexes_multiline_csv_records() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data.csv");
        std::fs::write(&path, "a,b\n\"hello\nworld\",2\nx,y\n").expect("write");
        let mut table = LazyFileTable::open(&path, ParseOptions::default()).expect("lazy table");

        assert_eq!(table.row_count(), RowCount::Unknown);
        assert_eq!(text_row(&mut table, 1), ["hello\nworld", "2"]);
        assert_eq!(table.column_count(), 2);
        finish_index(&mut table);
        assert_eq!(table.row_count(), RowCount::Exact(3));
        assert_eq!(
            text_rows(&mut table),
            vec![
                vec!["a".to_owned(), "b".to_owned()],
                vec!["hello\nworld".to_owned(), "2".to_owned()],
                vec!["x".to_owned(), "y".to_owned()]
            ]
        );
    }

    #[test]
    fn lazy_file_table_reverse_scan_reads_an_indexed_range() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data.csv");
        std::fs::write(&path, "a\n\"b\nline\"\nc\nd\n").expect("write");
        let mut table = LazyFileTable::open(&path, ParseOptions::default()).expect("lazy table");
        finish_index(&mut table);
        let mut delivered = Vec::new();
        let mut collect = |index: RowIndex, row: &Row| {
            delivered.push((index, row.display_cells()));
            ControlFlow::Continue(())
        };

        let progress = table
            .scan_rows(
                ScanRequest {
                    start: RowIndex(3),
                    direction: ScanDirection::Reverse,
                    max_rows: 3,
                },
                &mut collect,
            )
            .expect("reverse scan");

        assert_eq!(
            delivered,
            [
                (RowIndex(3), vec!["d".to_owned()]),
                (RowIndex(2), vec!["c".to_owned()]),
                (RowIndex(1), vec!["b\nline".to_owned()]),
            ]
        );
        assert_eq!(progress.next, Some(RowIndex(0)));
        assert!(!progress.reached_end);
    }

    #[test]
    fn lazy_file_table_indexes_and_delivers_new_rows_from_one_forward_scan() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data.csv");
        std::fs::write(&path, "a,b\n1,2\n\"hello\nworld\",3\n").expect("write");
        let mut table = LazyFileTable::open(&path, ParseOptions::default()).expect("lazy table");
        let mut delivered = Vec::new();
        let mut collect = |index: RowIndex, row: &Row| {
            delivered.push((index, row.display_cells()));
            ControlFlow::Continue(())
        };

        let progress = table
            .index_and_scan_rows(
                RowIndex(usize::MAX),
                ScanRequest {
                    start: RowIndex(0),
                    direction: ScanDirection::Forward,
                    max_rows: usize::MAX,
                },
                &mut collect,
            )
            .expect("index and scan");

        assert_eq!(progress.index.row_count, RowCount::Exact(3));
        assert_eq!(progress.scan.visited, 3);
        assert!(progress.scan.reached_end);
        assert_eq!(table.offsets.len(), 3);
        assert_eq!(table.scan_offset, std::fs::metadata(&path).unwrap().len());
        assert_eq!(
            delivered,
            vec![
                (RowIndex(0), vec!["a".to_owned(), "b".to_owned()]),
                (RowIndex(1), vec!["1".to_owned(), "2".to_owned()]),
                (RowIndex(2), vec!["hello\nworld".to_owned(), "3".to_owned()]),
            ]
        );
    }

    #[test]
    fn lazy_file_table_tracks_max_column_count() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ragged.csv");
        std::fs::write(&path, "a\n1,2,3\n").expect("write");
        let mut table = LazyFileTable::open(&path, ParseOptions::default()).expect("lazy table");

        finish_index(&mut table);
        assert_eq!(table.row_count(), RowCount::Exact(2));
        assert_eq!(table.column_count(), 3);
    }

    #[test]
    fn lazy_file_table_reports_columns_added_during_indexing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ragged.csv");
        std::fs::write(&path, "header\none\nwide,x,y\n").expect("write");
        let mut table = LazyFileTable::open(&path, ParseOptions::default()).expect("lazy table");

        assert_eq!(text_row(&mut table, 1), ["one"]);
        assert_eq!(table.column_count(), 1);
        let progress = table
            .ensure_indexed_through(RowIndex(2))
            .expect("index wider row");

        assert_eq!(table.column_count(), 3);
        assert_eq!(progress.schema_delta.added_columns.len(), 2);
        assert_eq!(
            progress
                .schema_delta
                .added_columns
                .iter()
                .map(|column| column.display_name.as_str())
                .collect::<Vec<_>>(),
            ["Column 2", "Column 3"]
        );
    }

    #[test]
    fn lazy_file_table_reports_columns_added_during_forward_scan() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ragged.csv");
        std::fs::write(&path, "header\none\nwide,x,y\n").expect("write");
        let mut table = LazyFileTable::open(&path, ParseOptions::default()).expect("lazy table");
        assert_eq!(text_row(&mut table, 1), ["one"]);
        let mut delivered = Vec::new();
        let mut collect = |_: RowIndex, row: &Row| {
            delivered.push(row.display_cells());
            ControlFlow::Continue(())
        };

        let progress = table
            .index_and_scan_rows(
                RowIndex(2),
                ScanRequest {
                    start: RowIndex(2),
                    direction: ScanDirection::Forward,
                    max_rows: 1,
                },
                &mut collect,
            )
            .expect("scan wider row");

        assert_eq!(delivered, [vec!["wide", "x", "y"]]);
        assert_eq!(progress.index.schema_delta.added_columns.len(), 2);
    }

    #[test]
    fn lazy_forward_scan_pads_earlier_rows_to_the_chunk_width() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ragged.csv");
        std::fs::write(&path, "header\none\nnarrow\nwide,x,y\n").expect("write");
        let mut table = LazyFileTable::open(&path, ParseOptions::default()).expect("lazy table");
        assert_eq!(text_row(&mut table, 1), ["one"]);
        let mut delivered = Vec::new();
        let mut collect = |_: RowIndex, row: &Row| {
            delivered.push(row.display_cells());
            ControlFlow::Continue(())
        };

        table
            .scan_rows(
                ScanRequest {
                    start: RowIndex(2),
                    direction: ScanDirection::Forward,
                    max_rows: 2,
                },
                &mut collect,
            )
            .expect("scan ragged chunk");

        assert_eq!(delivered, [vec!["narrow", "", ""], vec!["wide", "x", "y"]]);
    }

    #[test]
    fn lazy_file_table_pads_ragged_rows_across_read_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ragged.csv");
        std::fs::write(&path, "a,b,c\n1\n2,3,4\n").expect("write");

        let mut unindexed =
            LazyFileTable::open(&path, ParseOptions::default()).expect("lazy table");
        assert_eq!(text_row(&mut unindexed, 0), ["a", "b", "c"]);
        let mut forward_rows = Vec::new();
        let mut collect = |_: RowIndex, row: &Row| {
            forward_rows.push(row.display_cells());
            ControlFlow::Continue(())
        };
        unindexed
            .scan_rows(
                ScanRequest {
                    start: RowIndex(1),
                    direction: ScanDirection::Forward,
                    max_rows: 1,
                },
                &mut collect,
            )
            .expect("scan unindexed ragged row");
        assert_eq!(forward_rows, [vec!["1", "", ""]]);

        let mut indexed = LazyFileTable::open(&path, ParseOptions::default()).expect("lazy table");
        finish_index(&mut indexed);
        assert_eq!(text_row(&mut indexed, 1), ["1", "", ""]);
        let mut indexed_rows = Vec::new();
        let mut collect = |_: RowIndex, row: &Row| {
            indexed_rows.push(row.display_cells());
            ControlFlow::Continue(())
        };
        indexed
            .scan_rows(
                ScanRequest {
                    start: RowIndex(1),
                    direction: ScanDirection::Forward,
                    max_rows: 1,
                },
                &mut collect,
            )
            .expect("scan indexed ragged row");
        assert_eq!(indexed_rows, [vec!["1", "", ""]]);
    }

    #[test]
    fn lazy_file_table_materializes_non_utf8_input_with_parse_options() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("latin1.csv");
        std::fs::write(&path, b"caf\xe9,2\n").expect("write");
        let mut table = LazyFileTable::open(
            &path,
            ParseOptions {
                encoding: Some("latin-1".to_owned()),
                ..ParseOptions::default()
            },
        )
        .expect("lazy table");

        assert_eq!(text_row(&mut table, 0), ["café", "2"]);
        assert_eq!(
            text_rows(&mut table),
            vec![vec!["café".to_owned(), "2".to_owned()]]
        );
    }

    #[test]
    fn lazy_forward_scan_bounds_read_ahead_when_visitor_breaks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data.csv");
        let data = (0..300).map(|row| format!("{row}\n")).collect::<String>();
        std::fs::write(&path, data).expect("write");
        let mut table = LazyFileTable::open(&path, ParseOptions::default()).expect("lazy table");
        let mut visits = 0;
        let mut stop = |_: RowIndex, _: &Row| {
            visits += 1;
            ControlFlow::Break(())
        };

        let progress = table
            .scan_rows(
                ScanRequest {
                    start: RowIndex(0),
                    direction: ScanDirection::Forward,
                    max_rows: usize::MAX,
                },
                &mut stop,
            )
            .expect("bounded scan");

        assert_eq!(visits, 1);
        assert_eq!(progress.visited, 1);
        assert_eq!(table.offsets.len(), LAZY_FORWARD_SCAN_BATCH_ROWS);
        assert!(!table.eof);
    }

    #[test]
    fn lazy_file_table_uses_one_inferred_encoding_for_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("latin1.csv");
        std::fs::write(&path, b"ascii,ok\ncaf\xe9,22\n").expect("write");
        let mut table = LazyFileTable::open(&path, ParseOptions::default()).expect("lazy table");

        assert_eq!(text_row(&mut table, 0), ["ascii", "ok"]);
        assert_eq!(text_row(&mut table, 1), ["café", "22"]);
        assert_eq!(
            text_rows(&mut table),
            vec![
                vec!["ascii".to_owned(), "ok".to_owned()],
                vec!["café".to_owned(), "22".to_owned()]
            ]
        );
    }

    #[test]
    fn lazy_file_table_sniffs_delimiter_from_decoded_sample() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("utf16.tsv");
        let mut bytes = vec![0xff, 0xfe];
        for unit in "a\tb\n1\t2\n".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        std::fs::write(&path, bytes).expect("write");
        let options = ParseOptions {
            encoding: Some("utf-16".to_owned()),
            ..ParseOptions::default()
        };

        assert_eq!(
            sniff_file_delimiter(&path, &options).expect("delimiter"),
            b'\t'
        );
        let mut materialized = LazyFileTable {
            generation: SourceGeneration::new(),
            fingerprint: source_fingerprint(&path).expect("fingerprint"),
            path,
            offsets: Vec::new(),
            options: ParseOptions {
                delimiter: Some(b'\t'),
                ..options
            },
            column_count: 0,
            scan_offset: 0,
            eof: false,
        };
        assert_eq!(
            text_rows(&mut materialized),
            vec![
                vec!["a".to_owned(), "b".to_owned()],
                vec!["1".to_owned(), "2".to_owned()]
            ]
        );
    }

    #[test]
    fn lazy_file_table_rejects_utf16_byte_indexing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("utf16.tsv");
        let mut bytes = vec![0xff, 0xfe];
        for unit in "a\tb\n1\t2\n".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        std::fs::write(&path, bytes).expect("write");

        let error = LazyFileTable::open(
            &path,
            ParseOptions {
                encoding: Some("utf-16".to_owned()),
                ..ParseOptions::default()
            },
        )
        .expect_err("utf16 lazy table error");
        assert!(
            error
                .to_string()
                .contains("lazy byte-indexed table storage does not support utf-16"),
            "{error}"
        );
    }

    #[test]
    fn lazy_indexing_failure_preserves_last_valid_progress() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data.csv");
        std::fs::write(&path, "a,b\n1,2\n3,4\n").expect("write");
        let mut table = LazyFileTable::open(&path, ParseOptions::default()).expect("lazy table");
        table
            .ensure_indexed_through(RowIndex(0))
            .expect("initial index");
        let prior_count = table.row_count();
        let prior_offset = table.scan_offset;

        std::fs::write(&path, "changed\n").expect("replace");
        let error = table
            .ensure_indexed_through(RowIndex(2))
            .expect_err("changed source");
        assert!(error.to_string().contains("reload is required"));
        assert_eq!(table.row_count(), prior_count);
        assert_eq!(table.scan_offset, prior_offset);
    }

    #[test]
    fn missing_csv_record_position_is_an_indexing_error() {
        let record = csv::ByteRecord::new();
        let error = byte_record_offset(&record).expect_err("missing record position");
        assert!(error.to_string().contains("byte offset"));
    }

    #[test]
    fn fallback_sample_hash_detects_same_length_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data.csv");
        std::fs::write(&path, "alpha").expect("write initial");
        let initial = source_sample_hash(&path, 5).expect("initial hash");

        std::fs::write(&path, "omega").expect("write replacement");
        let replacement = source_sample_hash(&path, 5).expect("replacement hash");

        assert_ne!(initial, replacement);
    }

    #[test]
    fn lazy_store_reports_source_query_as_unsupported_without_mutating_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data.csv");
        std::fs::write(&path, "a\nb\n").expect("write");
        let mut table = LazyFileTable::open(&path, ParseOptions::default()).expect("lazy table");
        let query = SourceQuery::new(table.generation(), std::num::NonZeroUsize::new(10).unwrap());
        assert!(matches!(
            table.execute_source_query(&query).expect("capability"),
            SourceQueryExecution::Unsupported { .. }
        ));
        assert_eq!(text_row(&mut table, 0), ["a"]);
    }

    #[test]
    fn sampled_scan_fold_is_bounded_and_reports_partial_result() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data.csv");
        std::fs::write(&path, "1\n2\n3\n4\n").expect("write");
        let mut table = LazyFileTable::open(&path, ParseOptions::default()).expect("lazy table");

        let result = scan_fold(
            &mut table,
            ReductionScope::Sampled(2),
            0_i64,
            |sum, _, row| {
                *sum += row.cells[0].display().parse::<i64>().expect("integer");
            },
        )
        .expect("sampled fold");

        assert_eq!(result.value, 3);
        assert_eq!(result.rows_scanned, 2);
        assert!(!result.complete);
        assert_eq!(table.row_count(), RowCount::AtLeast(2));
    }

    #[test]
    fn exact_scan_fold_reaches_eof_without_materializing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data.csv");
        std::fs::write(&path, "1\n2\n3\n4\n").expect("write");
        let mut table = LazyFileTable::open(&path, ParseOptions::default()).expect("lazy table");

        let result = scan_fold(
            &mut table,
            ReductionScope::Exact,
            (0_i64, 0_i64),
            |(min, max), _, row| {
                let value = row.cells[0].display().parse::<i64>().expect("integer");
                *min = (*min).min(value);
                *max = (*max).max(value);
            },
        )
        .expect("exact fold");

        assert_eq!(result.value, (0, 4));
        assert_eq!(result.rows_scanned, 4);
        assert!(result.complete);
        assert_eq!(table.row_count(), RowCount::Exact(4));
    }

    #[test]
    fn shared_column_reduction_collects_width_type_gradient_and_identifiers() {
        let generation = SourceGeneration::new();
        let rows = vec![
            Row::new(
                RowId {
                    generation,
                    ordinal: 0,
                },
                vec![CellValue::Integer(2), CellValue::Text("beta".to_owned())],
            ),
            Row::new(
                RowId {
                    generation,
                    ordinal: 1,
                },
                vec![CellValue::Float(9.5), CellValue::Text("alpha".to_owned())],
            ),
        ];
        let mut table = InMemoryTable::from_rows(generation, rows).expect("table");

        let result = reduce_column_profiles(
            &mut table,
            ReductionScope::Exact,
            Some(&["number".to_owned(), "name".to_owned()]),
        )
        .expect("profiles");

        assert!(result.complete);
        assert_eq!(result.value[0].logical_type, LogicalType::Float);
        assert_eq!(result.value[0].numeric_min_max, Some((2.0, 9.5)));
        assert_eq!(result.value[0].max_width, "number".len());
        assert_eq!(
            result.value[1].identifiers,
            BTreeSet::from(["alpha".to_owned(), "beta".to_owned()])
        );
    }
}
