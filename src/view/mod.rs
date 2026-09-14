mod column;

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::rc::Rc;

use unicode_width::UnicodeWidthStr;

use crate::ingest::OpenedTable;
use crate::ops::filter::{ActiveFilter, FilterCondition, FilterKind, FilterMode, FilterParseError};
use crate::ops::search::CaseInsensitiveQuery;
use crate::ops::sort::{
    parse_bool_key, parse_numeric_scalar, sort_rows_by_specs, NumericColumnProfile, SortDirection,
    SortMode, SortSpec,
};
use crate::table::{
    InMemoryTable, NullPlacement, RowCount, RowIndex, SourceGeneration, TableDefinition, TableStore,
};
#[cfg(feature = "saved-views")]
use crate::theme::ConditionalValue;
use crate::theme::{gradient_color_ref, identifier_color_ref, ConditionalColorRule};
use column::{ColumnIndex, Columns};

const MAX_ACTIVE_SORT_KEYS: usize = 3;

#[derive(Clone)]
struct SharedTableStore(Rc<RefCell<Box<dyn TableStore>>>);

impl fmt::Debug for SharedTableStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SharedTableStore")
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryRefresh {
    Applied,
    NotStoreBacked,
    Failed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ColumnWidthMode {
    #[default]
    Mode,
    Max,
    Fixed(u16),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Position {
    pub row: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VisibleCellStyleContext<'a> {
    pub(crate) conditional_color: Option<Cow<'a, str>>,
    pub(crate) search_match: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    pub origin: Position,
    pub height: usize,
    pub width: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnAlignment {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveSortKey {
    pub column: usize,
    pub mode: SortMode,
    pub direction: SortDirection,
    pub nulls: NullPlacement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnTypeChoice {
    Text,
    Date,
    Ip,
    Float,
    Integer,
    SemVer,
    Boolean,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnFormatChoice {
    Plain,
    Locale,
    Uppercase,
    Lowercase,
    Char,
    Bit,
    Word,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnSortChoice {
    None,
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnNullPlacementChoice {
    Inherited,
    First,
    Last,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnFilterSummary {
    pub mode: &'static str,
    pub kind: &'static str,
    pub input: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub visible_column: usize,
    pub source_column: usize,
    pub name: String,
    pub visible: bool,
    pub alignment: Option<ColumnAlignment>,
    pub column_type: ColumnTypeChoice,
    pub format: ColumnFormatChoice,
    pub sort: ColumnSortChoice,
    pub nulls: ColumnNullPlacementChoice,
    pub canonical_source: Option<String>,
    pub source_type: Option<String>,
    pub source_declared_type: Option<String>,
    pub source_operations: Vec<String>,
    pub filters: Vec<ColumnFilterSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfoUpdate {
    pub visible: bool,
    pub alignment: Option<ColumnAlignment>,
    pub column_type: ColumnTypeChoice,
    pub format: ColumnFormatChoice,
    pub sort: ColumnSortChoice,
    pub nulls: ColumnNullPlacementChoice,
    pub clear_filters: bool,
}

#[allow(
    dead_code,
    reason = "saved-views metadata variants are feature-applied"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ColumnTypeMetadata {
    #[default]
    Text,
    Date,
    Ip,
    Float,
    Int,
    SemVer,
    BooleanWord,
    BooleanChar,
    BooleanBit,
}

#[allow(dead_code, reason = "saved-views display variants are feature-applied")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum DisplayFormatMetadata {
    #[default]
    Plain,
    Locale,
    Mask,
    Uppercase,
    Lowercase,
    BooleanChar,
    BooleanBit,
    BooleanWord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NumberMaskMetadata {
    grouped: bool,
    decimal_places: usize,
}

impl NumberMaskMetadata {
    #[cfg(feature = "saved-views")]
    fn to_mask(self) -> String {
        let mut mask = if self.grouped {
            "#,##0".to_owned()
        } else {
            "0".to_owned()
        };
        if self.decimal_places > 0 {
            mask.push('.');
            mask.push_str(&"0".repeat(self.decimal_places));
        }
        mask
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LocaleMetadata {
    grouping_separator: char,
    decimal_separator: char,
}

impl Default for LocaleMetadata {
    fn default() -> Self {
        Self::en_us()
    }
}

impl LocaleMetadata {
    fn en_us() -> Self {
        Self {
            grouping_separator: ',',
            decimal_separator: '.',
        }
    }

    #[cfg(feature = "saved-views")]
    fn from_posix(value: Option<&str>) -> Self {
        let value = value
            .map(str::to_owned)
            .or_else(system_locale)
            .unwrap_or_else(|| "en_US".to_owned());
        let language = value.split(['.', '@']).next().unwrap_or("en_US");
        match language {
            "de_DE" | "es_ES" | "it_IT" | "nl_NL" => Self {
                grouping_separator: '.',
                decimal_separator: ',',
            },
            "fr_FR" | "fr_BE" => Self {
                grouping_separator: ' ',
                decimal_separator: ',',
            },
            _ => Self::en_us(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ColumnDisplayMetadata {
    column_type: ColumnTypeMetadata,
    format: DisplayFormatMetadata,
    mask: Option<NumberMaskMetadata>,
    locale: LocaleMetadata,
}

#[derive(Debug, Clone, Default)]
struct ColumnColorMetadata {
    numeric_min_max: Option<(f64, f64)>,
    identifier_color_refs: BTreeMap<usize, BTreeMap<String, String>>,
    gradient_color_refs: BTreeMap<usize, Vec<String>>,
}

impl Viewport {
    pub fn new(height: usize, width: usize) -> Self {
        Self {
            origin: Position::default(),
            height,
            width,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TableView {
    preview_preparing: bool,
    table_definition: Option<TableDefinition>,
    source_store: Option<InMemoryTable>,
    incremental_store: Option<SharedTableStore>,
    query_store: Option<SharedTableStore>,
    base_cached_rows: Option<Vec<Vec<String>>>,
    base_cached_row_ids: Option<Vec<crate::table::RowId>>,
    active_source_query: Option<crate::table::SourceQuery>,
    source_capabilities: crate::table::SourceOperationCapabilities,
    source_result_extent: Option<crate::table::ResultExtent>,
    source_result_is_partial: bool,
    source_result_warnings: Vec<String>,
    source_query_provenance: Option<crate::table::NativeQueryArtifact>,
    source_query_coordinator: Rc<RefCell<crate::table::SourceQueryCoordinator>>,
    pending_source_query: Option<crate::table::SourceQuery>,
    pending_cursor_identity: Option<crate::table::StableRowIdentity>,
    pending_mark_identity: Option<crate::table::StableRowIdentity>,
    active_view_transform: Option<crate::table::ViewTransform>,
    committed_filters: Vec<ActiveFilter>,
    committed_sort_keys: Vec<ActiveSortKey>,
    row_ids: Vec<crate::table::RowId>,
    object_mode: Option<crate::ingest::ObjectModeResolution>,
    source_status: Option<String>,
    header: Option<Vec<String>>,
    header_visible: bool,
    rows: Vec<Vec<String>>,
    visible_rows: Vec<usize>,
    filters: Vec<ActiveFilter>,
    cursor: Position,
    viewport: Viewport,
    mark: Option<Position>,
    mark_identity: Option<(crate::table::RowId, crate::table::ColumnId)>,
    column_width_mode: ColumnWidthMode,
    column_gap: usize,
    column_widths: Vec<usize>,
    sampled_column_widths: Vec<usize>,
    computed_column_widths_cache: Vec<usize>,
    terminal_width: usize,
    column_width_modified: BTreeSet<usize>,
    hidden_columns: BTreeSet<usize>,
    column_alignment_overrides: Vec<Option<ColumnAlignment>>,
    column_label_overrides: Vec<Option<String>>,
    view_nulls: Option<NullPlacement>,
    column_nulls: Vec<Option<NullPlacement>>,
    column_display: Vec<ColumnDisplayMetadata>,
    column_color_rules: Vec<Vec<ConditionalColorRule>>,
    column_color_metadata: Vec<ColumnColorMetadata>,
    column_metadata_modified: BTreeSet<usize>,
    sort_keys: Vec<ActiveSortKey>,
    columns: Columns,
    #[cfg(feature = "saved-views")]
    pending_saved_columns: BTreeMap<String, crate::saved_views::ColumnView>,
    #[cfg(feature = "saved-views")]
    saved_column_locale: Option<String>,
    #[cfg(feature = "saved-views")]
    saved_column_widths: Vec<Option<crate::saved_views::ColumnWidth>>,
    #[cfg(feature = "saved-views")]
    pending_saved_sorts: Vec<crate::saved_views::SortKey>,
    #[cfg(feature = "saved-views")]
    pending_saved_filters: Vec<crate::saved_views::SavedFilter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReloadState {
    pub cursor: Position,
    pub viewport_origin: Position,
    pub column_width_mode: ColumnWidthMode,
    pub column_gap: usize,
    pub column_widths: Vec<usize>,
    pub search: Option<String>,
}

impl ReloadState {
    pub fn capture(
        view: &TableView,
        column_width_mode: ColumnWidthMode,
        column_gap: usize,
        column_widths: Vec<usize>,
        search: Option<String>,
    ) -> Self {
        Self {
            cursor: view.cursor(),
            viewport_origin: view.viewport().origin,
            column_width_mode,
            column_gap,
            column_widths,
            search,
        }
    }

    pub fn apply_to(&self, view: &mut TableView) {
        view.viewport.origin = self.viewport_origin;
        view.goto(self.cursor.row, self.cursor.column);
    }
}

impl TableView {
    pub fn classify(rows: Vec<Vec<String>>, viewport: Viewport) -> Self {
        let has_header = rows.len() > 1
            && rows
                .first()
                .is_some_and(|row| !row.iter().any(|cell| cell.parse::<f64>().is_ok()));

        let (header, rows) = if has_header {
            let mut rows = rows;
            (Some(rows.remove(0)), rows)
        } else {
            (None, rows)
        };

        let columns = Columns::infer(header.as_deref(), &rows);
        #[cfg(feature = "saved-views")]
        let column_count = columns.len();
        let visible_rows = (0..rows.len()).collect();

        Self {
            preview_preparing: false,
            table_definition: None,
            source_store: None,
            incremental_store: None,
            query_store: None,
            base_cached_rows: None,
            base_cached_row_ids: None,
            active_source_query: None,
            source_capabilities: crate::table::SourceOperationCapabilities::default(),
            source_result_extent: Some(crate::table::ResultExtent::Complete {
                source_rows: rows.len(),
            }),
            source_result_is_partial: false,
            source_result_warnings: Vec::new(),
            source_query_provenance: None,
            source_query_coordinator: Rc::new(RefCell::new(
                crate::table::SourceQueryCoordinator::default(),
            )),
            pending_source_query: None,
            pending_cursor_identity: None,
            pending_mark_identity: None,
            active_view_transform: None,
            committed_filters: Vec::new(),
            committed_sort_keys: Vec::new(),
            row_ids: Vec::new(),
            object_mode: None,
            source_status: None,
            header_visible: header.is_some(),
            header,
            rows,
            visible_rows,
            filters: Vec::new(),
            cursor: Position::default(),
            viewport,
            mark: None,
            mark_identity: None,
            column_width_mode: ColumnWidthMode::Mode,
            column_gap: 2,
            column_widths: Vec::new(),
            sampled_column_widths: Vec::new(),
            computed_column_widths_cache: Vec::new(),
            terminal_width: 0,
            column_width_modified: BTreeSet::new(),
            hidden_columns: BTreeSet::new(),
            column_alignment_overrides: vec![None; columns.len()],
            column_label_overrides: vec![None; columns.len()],
            view_nulls: None,
            column_nulls: vec![None; columns.len()],
            column_display: vec![ColumnDisplayMetadata::default(); columns.len()],
            column_color_rules: vec![Vec::new(); columns.len()],
            column_color_metadata: vec![ColumnColorMetadata::default(); columns.len()],
            column_metadata_modified: BTreeSet::new(),
            sort_keys: Vec::new(),
            columns,
            #[cfg(feature = "saved-views")]
            pending_saved_columns: BTreeMap::new(),
            #[cfg(feature = "saved-views")]
            saved_column_locale: None,
            #[cfg(feature = "saved-views")]
            saved_column_widths: vec![None; column_count],
            #[cfg(feature = "saved-views")]
            pending_saved_sorts: Vec::new(),
            #[cfg(feature = "saved-views")]
            pending_saved_filters: Vec::new(),
        }
    }

    pub fn from_opened_table(mut opened: OpenedTable, viewport: Viewport) -> anyhow::Result<Self> {
        let header_visible = opened.definition.relation.header_visible;
        let generation = opened.definition.generation;
        let object_mode = opened.object_mode;
        let mut source_result_warnings = opened.warnings;
        source_result_warnings.extend(opened.store.result_warnings().iter().cloned());
        source_result_warnings.sort();
        source_result_warnings.dedup();
        let source_status =
            (!source_result_warnings.is_empty()).then(|| source_result_warnings.join("; "));
        let active_source_query = opened.store.active_source_query().cloned();
        let source_capabilities = opened.store.source_capabilities();
        let source_result_extent = opened.store.result_extent();
        let source_result_is_partial = opened.store.result_is_partial();
        let source_query_provenance = opened.store.query_provenance().cloned();
        let initial_target = viewport.height.saturating_sub(1);
        let progress = opened
            .store
            .ensure_indexed_through(RowIndex(initial_target))?;
        opened.definition.apply_delta(progress.schema_delta)?;
        let mut rows = Vec::new();
        let mut row_ids = Vec::new();
        for index in 0..=initial_target {
            let Some(row) = opened.store.row(RowIndex(index))? else {
                break;
            };
            row_ids.push(row.id);
            rows.push(row.display_cells());
        }
        let header = Some(
            opened
                .definition
                .columns
                .iter()
                .map(|column| column.display_name.clone())
                .collect::<Vec<_>>(),
        );
        let columns = Columns::infer(header.as_deref(), &rows);
        #[cfg(feature = "saved-views")]
        let column_count = columns.len();
        let visible_rows = (0..rows.len()).collect();
        Ok(Self {
            preview_preparing: false,
            table_definition: Some(opened.definition),
            source_store: None,
            incremental_store: Some(SharedTableStore(Rc::new(RefCell::new(opened.store)))),
            query_store: None,
            base_cached_rows: None,
            base_cached_row_ids: None,
            active_source_query,
            source_capabilities,
            source_result_extent,
            source_result_is_partial,
            source_result_warnings,
            source_query_provenance,
            source_query_coordinator: Rc::new(RefCell::new(
                crate::table::SourceQueryCoordinator::default(),
            )),
            pending_source_query: None,
            pending_cursor_identity: None,
            pending_mark_identity: None,
            active_view_transform: Some(crate::table::ViewTransform {
                generation,
                filters: Vec::new(),
                order_by: Vec::new(),
            }),
            committed_filters: Vec::new(),
            committed_sort_keys: Vec::new(),
            row_ids,
            object_mode,
            source_status,
            header_visible,
            header,
            rows,
            visible_rows,
            filters: Vec::new(),
            cursor: Position::default(),
            viewport,
            mark: None,
            mark_identity: None,
            column_width_mode: ColumnWidthMode::Mode,
            column_gap: 2,
            column_widths: Vec::new(),
            sampled_column_widths: Vec::new(),
            computed_column_widths_cache: Vec::new(),
            terminal_width: 0,
            column_width_modified: BTreeSet::new(),
            hidden_columns: BTreeSet::new(),
            column_alignment_overrides: vec![None; columns.len()],
            column_label_overrides: vec![None; columns.len()],
            view_nulls: None,
            column_nulls: vec![None; columns.len()],
            column_display: vec![ColumnDisplayMetadata::default(); columns.len()],
            column_color_rules: vec![Vec::new(); columns.len()],
            column_color_metadata: vec![ColumnColorMetadata::default(); columns.len()],
            column_metadata_modified: BTreeSet::new(),
            sort_keys: Vec::new(),
            columns,
            #[cfg(feature = "saved-views")]
            pending_saved_columns: BTreeMap::new(),
            #[cfg(feature = "saved-views")]
            saved_column_locale: None,
            #[cfg(feature = "saved-views")]
            saved_column_widths: vec![None; column_count],
            #[cfg(feature = "saved-views")]
            pending_saved_sorts: Vec::new(),
            #[cfg(feature = "saved-views")]
            pending_saved_filters: Vec::new(),
        })
    }

    pub fn source_generation(&self) -> Option<SourceGeneration> {
        self.table_definition
            .as_ref()
            .map(|definition| definition.generation)
    }

    pub fn table_definition(&self) -> Option<&TableDefinition> {
        self.table_definition.as_ref()
    }

    pub fn active_source_query(&self) -> Option<&crate::table::SourceQuery> {
        self.active_source_query.as_ref()
    }

    pub fn source_capabilities(&self) -> &crate::table::SourceOperationCapabilities {
        &self.source_capabilities
    }

    pub fn source_column_name_for_id(&self, id: crate::table::ColumnId) -> Option<String> {
        let definition = self.table_definition.as_ref()?;
        let index = definition
            .columns
            .iter()
            .position(|column| column.id == id)?;
        definition.canonical_column_key(index).or_else(|| {
            self.header
                .as_ref()
                .and_then(|header| header.get(index))
                .cloned()
        })
    }

    pub fn view_transform_summary(&self) -> String {
        format!(
            "{} view filter(s), {} view sort key(s), nulls {:?}",
            self.filters.len(),
            self.sort_keys.len(),
            self.view_nulls.unwrap_or(NullPlacement::Last)
        )
    }

    pub fn clear_view_operations(&mut self) {
        self.filters.clear();
        self.sort_keys.clear();
        self.apply_query_configuration();
    }

    pub fn toggle_view_null_placement(&mut self) {
        let next = match self.view_nulls.unwrap_or(NullPlacement::Last) {
            NullPlacement::First => Some(NullPlacement::Last),
            NullPlacement::Last => Some(NullPlacement::First),
        };
        self.set_view_null_placement(next);
    }

    pub fn visible_row_count(&self) -> usize {
        self.visible_rows.len()
    }

    pub fn source_operations_for_column(&self, column: crate::table::ColumnId) -> Vec<String> {
        let Some(query) = &self.active_source_query else {
            return Vec::new();
        };
        let mut operations = query
            .filters
            .iter()
            .filter(|filter| {
                matches!(
                    filter.scope,
                    crate::table::SourceFilterScope::Column(id) if id == column
                )
            })
            .map(|filter| format!("source filter: {}", filter.operator))
            .collect::<Vec<_>>();
        operations.extend(
            query
                .order_by
                .iter()
                .filter(|sort| sort.column == column)
                .map(|sort| format!("source sort: {:?}", sort.direction)),
        );
        operations
    }

    pub fn source_result_extent(&self) -> Option<crate::table::ResultExtent> {
        self.incremental_store
            .as_ref()
            .and_then(|store| store.0.borrow().result_extent())
            .or(self.source_result_extent)
    }

    pub fn source_result_is_partial(&self) -> bool {
        self.incremental_store
            .as_ref()
            .is_some_and(|store| store.0.borrow().result_is_partial())
            || self.source_result_is_partial
    }

    pub fn source_result_warnings(&self) -> &[String] {
        &self.source_result_warnings
    }

    pub fn source_field_catalog(&self) -> std::sync::Arc<[crate::table::SourceFieldMetadata]> {
        self.incremental_store
            .as_ref()
            .map(|store| store.0.borrow().source_field_catalog())
            .unwrap_or_default()
    }

    pub fn source_query_provenance(&self) -> Option<&crate::table::NativeQueryArtifact> {
        self.source_query_provenance.as_ref()
    }

    pub fn fetched_source_row_count(&self) -> usize {
        match self
            .incremental_store
            .as_ref()
            .map(|store| store.0.borrow().row_count())
            .or_else(|| self.source_store.as_ref().map(TableStore::row_count))
        {
            Some(RowCount::Exact(count) | RowCount::AtLeast(count)) => count,
            Some(RowCount::Unknown) | None => self.rows.len(),
        }
    }

    pub fn request_source_query(&mut self, query: crate::table::SourceQuery) -> bool {
        let Some(definition) = self.table_definition.as_ref() else {
            self.source_status = Some("Source queries require a store-backed table".to_owned());
            return false;
        };
        if let Err(error) = crate::table::validate_source_query(definition, &query) {
            self.source_status = Some(format!("Source query validation failed: {error}"));
            return false;
        }
        let cursor_identity = self.current_stable_row_identity();
        let mark_identity = self.mark_stable_row_identity();
        let Some(source) = self.incremental_store.as_ref() else {
            self.source_status = Some("Source query replacement is unavailable".to_owned());
            return false;
        };
        let task = match source.0.borrow_mut().source_query_task(query.clone()) {
            Ok(task) => task,
            Err(error) => {
                self.source_status = Some(error.to_string());
                return false;
            }
        };
        let revision = self.source_query_coordinator.borrow_mut().request(task);
        self.pending_source_query = Some(query);
        self.pending_cursor_identity = cursor_identity;
        self.pending_mark_identity = mark_identity;
        self.source_status = Some(format!("Source query revision {revision} is running"));
        true
    }

    pub fn source_query_is_pending(&self) -> bool {
        self.source_query_coordinator.borrow().is_pending()
    }

    pub fn poll_source_query(&mut self) -> bool {
        let event = self.source_query_coordinator.borrow_mut().poll();
        match event {
            Some(crate::table::SourceQueryCoordinatorEvent::Ready { revision, result }) => {
                let Some(query) = self.pending_source_query.take() else {
                    return false;
                };
                let cursor_identity = self.pending_cursor_identity.take();
                let mark_identity = self.pending_mark_identity.take();
                let applied = self.activate_source_replacement(
                    query,
                    *result,
                    cursor_identity,
                    mark_identity,
                );
                if applied {
                    self.source_status =
                        Some(format!("Source query revision {revision} completed"));
                }
                applied
            }
            Some(crate::table::SourceQueryCoordinatorEvent::Failed { error, .. }) => {
                self.pending_source_query = None;
                self.pending_cursor_identity = None;
                self.pending_mark_identity = None;
                self.source_status = Some(format!(
                    "Source query failed; prior result retained: {error}"
                ));
                false
            }
            None => false,
        }
    }

    pub fn await_latest_source_query(&mut self) -> anyhow::Result<()> {
        while self.source_query_is_pending() {
            self.poll_source_query();
            if self.source_query_is_pending() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        if let crate::table::SourceQueryProgress::Failed { error, .. } =
            self.source_query_coordinator.borrow().progress()
        {
            anyhow::bail!("latest source query failed: {error}");
        }
        Ok(())
    }

    pub fn replace_source_query(&mut self, query: crate::table::SourceQuery) -> bool {
        let Some(definition) = self.table_definition.clone() else {
            self.source_status = Some("Source queries require a store-backed table".to_owned());
            return false;
        };
        if let Err(error) = crate::table::validate_source_query(&definition, &query) {
            self.source_status = Some(format!("Source query validation failed: {error}"));
            return false;
        }
        let cursor_identity = self.current_stable_row_identity();
        let mark_identity = self.mark_stable_row_identity();
        let Some(source) = self.incremental_store.clone() else {
            self.source_status = Some("Source query replacement is unavailable".to_owned());
            return false;
        };
        let execution = source.0.borrow_mut().execute_source_query(&query);
        let replacement = match execution {
            Ok(
                crate::table::SourceQueryExecution::SourceExecuted(store)
                | crate::table::SourceQueryExecution::BoundedLocal(store),
            ) => store,
            Ok(crate::table::SourceQueryExecution::Unsupported { reason }) => {
                self.source_status = Some(format!("Source query unsupported: {reason}"));
                return false;
            }
            Err(error) => {
                self.source_status = Some(format!(
                    "Source query failed; prior result retained: {error}"
                ));
                return false;
            }
        };
        self.activate_source_replacement(query, replacement, cursor_identity, mark_identity)
    }

    fn activate_source_replacement(
        &mut self,
        _query: crate::table::SourceQuery,
        replacement: crate::table::SourceResult,
        cursor_identity: Option<crate::table::StableRowIdentity>,
        mark_identity: Option<crate::table::StableRowIdentity>,
    ) -> bool {
        let viewport = self.viewport;
        let opened = crate::ingest::OpenedTable {
            generation: replacement.definition.generation,
            definition: replacement.definition,
            store: replacement.store,
            object_mode: self.object_mode,
            warnings: replacement.metadata.warnings,
        };
        let mut next = match Self::from_opened_table(opened, viewport) {
            Ok(next) => next,
            Err(error) => {
                self.source_status = Some(format!(
                    "Source query loading failed; prior result retained: {error}"
                ));
                return false;
            }
        };
        next.restore_view_settings_from(self);
        next.restore_stable_identities(cursor_identity, mark_identity);
        *self = next;
        true
    }

    pub fn object_mode_resolution(&self) -> Option<crate::ingest::ObjectModeResolution> {
        self.object_mode
    }

    pub fn set_view_null_placement(&mut self, nulls: Option<NullPlacement>) {
        let previous = self.view_nulls;
        self.view_nulls = nulls;
        for key in &mut self.sort_keys {
            key.nulls = self
                .column_nulls
                .get(key.column)
                .copied()
                .flatten()
                .or(self.view_nulls)
                .unwrap_or(NullPlacement::Last);
        }
        if !self.apply_query_configuration() {
            self.view_nulls = previous;
        }
    }

    pub fn resolved_null_placement(&self, source_column: usize) -> NullPlacement {
        self.column_nulls
            .get(source_column)
            .copied()
            .flatten()
            .or(self.view_nulls)
            .unwrap_or(NullPlacement::Last)
    }

    pub fn row_count_state(&self) -> RowCount {
        if self.view_transform_is_active() {
            return self
                .query_store
                .as_ref()
                .map(|store| store.0.borrow().row_count())
                .unwrap_or(RowCount::Exact(self.rows.len()));
        }
        self.source_store
            .as_ref()
            .map(TableStore::row_count)
            .or_else(|| {
                self.incremental_store
                    .as_ref()
                    .map(|store| store.0.borrow().row_count())
            })
            .unwrap_or(RowCount::Exact(self.rows.len()))
    }

    pub fn ensure_source_indexed_through(&mut self, row: usize) -> anyhow::Result<RowCount> {
        if let Some(source) = &self.source_store {
            return Ok(source.row_count());
        }
        let loading_query_result = self.view_transform_is_active() && self.query_store.is_some();
        let shared = if loading_query_result {
            self.query_store.clone()
        } else {
            self.incremental_store.clone()
        };
        let Some(shared) = shared else {
            return Ok(RowCount::Exact(self.rows.len()));
        };
        let previous_len = self.rows.len();
        let mut store = shared.0.borrow_mut();
        let mut appended = Vec::new();
        let progress_result = if row >= previous_len {
            let max_rows = row.saturating_sub(previous_len).saturating_add(1);
            let mut collect_row = |_: RowIndex, source_row: &crate::table::Row| {
                appended.push(source_row.clone());
                std::ops::ControlFlow::Continue(())
            };
            store
                .index_and_scan_rows(
                    RowIndex(row),
                    crate::table::ScanRequest {
                        start: RowIndex(previous_len),
                        direction: crate::table::ScanDirection::Forward,
                        max_rows,
                    },
                    &mut collect_row,
                )
                .map(|progress| progress.index)
        } else {
            store.ensure_indexed_through(RowIndex(row))
        };
        let progress = match progress_result {
            Ok(progress) => progress,
            Err(error) => {
                drop(store);
                self.source_status = Some(format!("Indexing failed: {error}"));
                return Err(error);
            }
        };
        if appended.is_empty() && row < previous_len {
            let available = match progress.row_count {
                RowCount::Exact(count) | RowCount::AtLeast(count) => count,
                RowCount::Unknown => 0,
            };
            let max_rows = available.saturating_sub(previous_len).min(256);
            if max_rows > 0 {
                let mut collect_row = |_: RowIndex, source_row: &crate::table::Row| {
                    appended.push(source_row.clone());
                    std::ops::ControlFlow::Continue(())
                };
                if let Err(error) = store.scan_rows(
                    crate::table::ScanRequest {
                        start: RowIndex(previous_len),
                        direction: crate::table::ScanDirection::Forward,
                        max_rows,
                    },
                    &mut collect_row,
                ) {
                    drop(store);
                    self.source_status = Some(format!("Indexing failed: {error}"));
                    return Err(error);
                }
            }
        }
        drop(store);
        self.apply_source_schema_delta(progress.schema_delta)?;
        if self.view_transform_is_active() && !loading_query_result {
            return Ok(self.row_count_state());
        }
        for source_row in appended {
            self.row_ids.push(source_row.id);
            let mut cells = source_row.display_cells();
            cells.resize(self.source_column_count(), String::new());
            self.rows.push(cells);
        }
        self.visible_rows.extend(previous_len..self.rows.len());
        if self.rows.len().saturating_sub(previous_len) >= 256 {
            self.source_status = Some(format!("Indexed {} rows", self.rows.len()));
        }
        Ok(progress.row_count)
    }

    fn view_transform_is_active(&self) -> bool {
        self.active_view_transform
            .as_ref()
            .is_some_and(|query| !query.filters.is_empty() || !query.order_by.is_empty())
    }

    pub fn take_source_status(&mut self) -> Option<String> {
        self.source_status.take()
    }

    pub fn progressive_search(
        &mut self,
        query: &str,
        direction: crate::ops::search::SearchDirection,
    ) -> Option<Position> {
        let matcher = CaseInsensitiveQuery::new(query)?;
        let start = self.cursor;
        match direction {
            crate::ops::search::SearchDirection::Forward => {
                let mut position = self.next_search_position(start)?;
                loop {
                    if let Some(value) = self.search_value_at(position) {
                        if matcher.matches(&value) {
                            return Some(position);
                        }
                        position = self.next_search_position(position)?;
                        continue;
                    }
                    let previous = self.rows.len();
                    let _ = self.ensure_source_indexed_through(previous.saturating_add(255));
                    if self.rows.len() == previous {
                        break;
                    }
                }
                let mut position = Position::default();
                while position != start {
                    if self
                        .search_value_at(position)
                        .is_some_and(|value| matcher.matches(&value))
                    {
                        return Some(position);
                    }
                    position = self.next_search_position(position)?;
                }
                None
            }
            crate::ops::search::SearchDirection::Reverse => {
                let mut position = self.previous_search_position(start);
                while let Some(candidate) = position {
                    if self
                        .search_value_at(candidate)
                        .is_some_and(|value| matcher.matches(&value))
                    {
                        return Some(candidate);
                    }
                    position = self.previous_search_position(candidate);
                }
                while !matches!(self.row_count_state(), RowCount::Exact(_)) {
                    let previous = self.rows.len();
                    let _ = self.ensure_source_indexed_through(previous.saturating_add(255));
                    if self.rows.len() == previous {
                        break;
                    }
                }
                let mut position = self.last_search_position();
                while let Some(candidate) = position {
                    if candidate == start {
                        break;
                    }
                    if self
                        .search_value_at(candidate)
                        .is_some_and(|value| matcher.matches(&value))
                    {
                        return Some(candidate);
                    }
                    position = self.previous_search_position(candidate);
                }
                None
            }
        }
    }

    pub fn progressive_skip_to_change(
        &mut self,
        axis: crate::ops::skip::Axis,
        direction: crate::ops::skip::Direction,
        count: usize,
    ) -> Position {
        let mut position = self.cursor;
        for _ in 0..count.max(1) {
            let Some(start_value) = self.search_value_at(position) else {
                break;
            };
            let mut candidate = position;
            loop {
                candidate = match (axis, direction) {
                    (crate::ops::skip::Axis::Row, crate::ops::skip::Direction::Forward) => {
                        let next = candidate.row.saturating_add(1);
                        let _ = self.ensure_source_indexed_through(next);
                        Position {
                            row: next,
                            column: candidate.column,
                        }
                    }
                    (crate::ops::skip::Axis::Row, crate::ops::skip::Direction::Backward) => {
                        let Some(row) = candidate.row.checked_sub(1) else {
                            break;
                        };
                        Position {
                            row,
                            column: candidate.column,
                        }
                    }
                    (crate::ops::skip::Axis::Column, crate::ops::skip::Direction::Forward) => {
                        Position {
                            row: candidate.row,
                            column: candidate.column.saturating_add(1),
                        }
                    }
                    (crate::ops::skip::Axis::Column, crate::ops::skip::Direction::Backward) => {
                        let Some(column) = candidate.column.checked_sub(1) else {
                            break;
                        };
                        Position {
                            row: candidate.row,
                            column,
                        }
                    }
                };
                let Some(value) = self.search_value_at(candidate) else {
                    break;
                };
                if value != start_value {
                    position = candidate;
                    break;
                }
            }
        }
        position
    }

    fn search_value_at(&self, position: Position) -> Option<String> {
        let row_index = *self.visible_rows.get(position.row)?;
        let row = self.rows.get(row_index)?;
        let source_column = self.source_column_for_visible(position.column)?;
        Some(self.render_source_cell(source_column, row.get(source_column).map(String::as_str)))
    }

    fn next_search_position(&self, position: Position) -> Option<Position> {
        if position.column + 1 < self.column_count() {
            Some(Position {
                row: position.row,
                column: position.column + 1,
            })
        } else {
            Some(Position {
                row: position.row.checked_add(1)?,
                column: 0,
            })
        }
    }

    fn previous_search_position(&self, position: Position) -> Option<Position> {
        if position.column > 0 {
            Some(Position {
                row: position.row,
                column: position.column - 1,
            })
        } else {
            let row = position.row.checked_sub(1)?;
            Some(Position {
                row,
                column: self.column_count().saturating_sub(1),
            })
        }
    }

    fn last_search_position(&self) -> Option<Position> {
        (!self.rows.is_empty() && self.column_count() > 0).then_some(Position {
            row: self.rows.len() - 1,
            column: self.column_count() - 1,
        })
    }

    fn apply_source_schema_delta(
        &mut self,
        delta: crate::table::SchemaDelta,
    ) -> anyhow::Result<()> {
        if delta.is_empty() {
            #[cfg(feature = "saved-views")]
            if delta.completed {
                self.apply_pending_saved_operations(true);
            }
            return Ok(());
        }
        #[cfg(feature = "saved-views")]
        let completed = delta.completed;
        #[cfg(feature = "saved-views")]
        let old_count = self.source_column_count();
        let Some(definition) = self.table_definition.as_mut() else {
            return Ok(());
        };
        definition.apply_delta(delta)?;
        let new_count = definition.columns.len();
        self.header = Some(
            definition
                .columns
                .iter()
                .map(|column| column.display_name.clone())
                .collect(),
        );
        for row in &mut self.rows {
            row.resize(new_count, String::new());
        }
        self.column_alignment_overrides.resize(new_count, None);
        self.column_label_overrides.resize(new_count, None);
        self.column_nulls.resize(new_count, None);
        self.column_display
            .resize(new_count, ColumnDisplayMetadata::default());
        self.column_color_rules.resize(new_count, Vec::new());
        self.column_color_metadata
            .resize(new_count, ColumnColorMetadata::default());
        #[cfg(feature = "saved-views")]
        self.saved_column_widths.resize(new_count, None);
        self.columns = Columns::infer(self.header.as_deref(), &self.rows);
        self.computed_column_widths_cache.clear();
        #[cfg(feature = "saved-views")]
        {
            let mut resolved = crate::saved_views::ResolvedColumns {
                columns: vec![None; new_count],
                pending: BTreeMap::new(),
                warnings: Vec::new(),
            };
            for index in old_count..new_count {
                let key = self
                    .table_definition
                    .as_ref()
                    .and_then(|definition| definition.canonical_column_key(index));
                let Some(key) = key else { continue };
                if let Some(column_view) = self.pending_saved_columns.remove(&key) {
                    resolved.columns[index] = Some(crate::saved_views::ResolvedColumnView {
                        column_index: index,
                        source_key: key,
                        view: column_view,
                    });
                }
            }
            if resolved.columns.iter().any(Option::is_some) {
                let locale = self.saved_column_locale.clone();
                self.apply_saved_columns(&resolved, locale.as_deref());
            }
            if completed && !self.pending_saved_columns.is_empty() {
                self.source_status = Some(format!(
                    "Saved view columns not found: {}",
                    self.pending_saved_columns
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                self.pending_saved_columns.clear();
            }
            self.apply_pending_saved_operations(completed);
        }
        Ok(())
    }

    pub fn with_column_width_mode(mut self, mode: ColumnWidthMode) -> Self {
        self.column_width_mode = mode;
        self
    }

    pub fn header(&self) -> Option<&[String]> {
        self.header.as_deref()
    }

    pub fn rendered_header(&self) -> Option<Vec<String>> {
        self.rendered_source_header().map(|header| {
            header
                .iter()
                .enumerate()
                .filter(|(source_column, _)| self.source_column_visible(*source_column))
                .map(|(_, cell)| cell.clone())
                .collect()
        })
    }

    /// Header labels for document output, excluding TUI sort/filter glyphs.
    pub fn output_header(&self) -> Option<Vec<String>> {
        self.header.as_ref().map(|header| {
            header
                .iter()
                .enumerate()
                .filter(|(source_column, _)| self.source_column_visible(*source_column))
                .map(|(_, cell)| cell.clone())
                .collect()
        })
    }

    fn rendered_source_header(&self) -> Option<Vec<String>> {
        self.header.as_ref().map(|header| {
            header
                .iter()
                .enumerate()
                .map(|(source_column, cell)| {
                    format!(
                        "{}{}{cell}",
                        self.source_column_sort_indicator(source_column)
                            .unwrap_or_default(),
                        self.source_column_filter_indicator(source_column)
                            .unwrap_or_default()
                    )
                })
                .collect()
        })
    }

    pub fn header_visible(&self) -> bool {
        self.header_visible
    }

    pub fn rows(&self) -> &[Vec<String>] {
        &self.rows
    }

    pub fn row_count(&self) -> usize {
        self.visible_rows.len()
    }

    pub fn total_row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn visible_rows(&self) -> impl Iterator<Item = &Vec<String>> {
        self.visible_rows
            .iter()
            .filter_map(|row_idx| self.rows.get(*row_idx))
    }

    pub fn visible_rows_vec(&self) -> Vec<Vec<String>> {
        self.visible_rows()
            .map(|row| self.visible_cells_for_row(row))
            .collect()
    }

    pub fn rendered_visible_row(&self, row: usize) -> Option<Vec<String>> {
        self.visible_row(row)
            .map(|row| self.visible_cells_for_row(row))
    }

    pub fn visible_raw_rows_vec(&self) -> Vec<Vec<String>> {
        self.visible_rows()
            .map(|row| {
                self.visible_source_columns()
                    .into_iter()
                    .map(|source_column| row.get(source_column).cloned().unwrap_or_default())
                    .collect()
            })
            .collect()
    }

    pub fn search_rows_vec(&self) -> Vec<Vec<String>> {
        self.visible_rows()
            .map(|row| {
                self.visible_source_columns()
                    .into_iter()
                    .map(|source_column| {
                        let raw = row
                            .get(source_column)
                            .map(String::as_str)
                            .unwrap_or_default();
                        let rendered = self.render_source_cell(source_column, Some(raw));
                        if rendered == raw {
                            raw.to_owned()
                        } else {
                            format!("{raw}\n{rendered}")
                        }
                    })
                    .collect()
            })
            .collect()
    }

    fn visible_cells_for_row(&self, row: &[String]) -> Vec<String> {
        self.visible_source_columns()
            .into_iter()
            .map(|source_column| {
                self.render_source_cell(source_column, row.get(source_column).map(String::as_str))
            })
            .collect()
    }

    pub fn current_cell(&self) -> Option<&str> {
        let source_column = self.source_column_for_visible(self.cursor.column)?;
        self.visible_row(self.cursor.row)?
            .get(source_column)
            .map(String::as_str)
    }

    pub fn current_raw_cell(&self) -> Option<&str> {
        self.current_cell()
    }

    pub fn current_cell_rendered(&self) -> Option<String> {
        let source_column = self.source_column_for_visible(self.cursor.column)?;
        let raw = self
            .visible_row(self.cursor.row)?
            .get(source_column)
            .map(String::as_str);
        Some(self.render_source_cell(source_column, raw))
    }

    pub fn current_cell_matches(&self, query: &str) -> bool {
        let Some(source_column) = self.source_column_for_visible(self.cursor.column) else {
            return false;
        };
        let Some(raw) = self
            .visible_row(self.cursor.row)
            .and_then(|row| row.get(source_column).map(String::as_str))
        else {
            return false;
        };
        let Some(query) = CaseInsensitiveQuery::new(query) else {
            return false;
        };
        if query.matches(raw) {
            return true;
        }
        let rendered = self.render_source_cell(source_column, Some(raw));
        self.search_matches_cell(raw, &rendered, Some(&query))
    }

    #[cfg(all(test, feature = "saved-views"))]
    fn visible_cell_matches_search_query(
        &self,
        row: usize,
        visible_column: usize,
        query: &str,
    ) -> bool {
        let Some(rendered_row) = self.rendered_visible_row(row) else {
            return false;
        };
        let Some(rendered) = rendered_row.get(visible_column) else {
            return false;
        };
        let Some(query) = CaseInsensitiveQuery::new(query) else {
            return false;
        };
        self.visible_cell_style_context(row, visible_column, rendered, Some(&query))
            .search_match
    }

    #[cfg(all(test, feature = "saved-views"))]
    fn visible_cell_style_context(
        &self,
        row: usize,
        visible_column: usize,
        rendered: &str,
        query: Option<&CaseInsensitiveQuery<'_>>,
    ) -> VisibleCellStyleContext<'_> {
        let Some(source_column) = self.source_column_for_visible(visible_column) else {
            return VisibleCellStyleContext {
                conditional_color: None,
                search_match: false,
            };
        };
        self.source_cell_style_context(row, source_column, rendered, query)
    }

    pub(crate) fn source_cell_style_context(
        &self,
        row: usize,
        source_column: usize,
        rendered: &str,
        query: Option<&CaseInsensitiveQuery<'_>>,
    ) -> VisibleCellStyleContext<'_> {
        let Some(source_row) = self.source_row_for_visible_row(row) else {
            return VisibleCellStyleContext {
                conditional_color: None,
                search_match: false,
            };
        };
        let raw = self
            .rows
            .get(source_row)
            .and_then(|row| row.get(source_column).map(String::as_str))
            .unwrap_or_default();
        VisibleCellStyleContext {
            conditional_color: self.conditional_color_for_source_cell(source_column, raw, rendered),
            search_match: self.search_matches_cell(raw, rendered, query),
        }
    }

    fn search_matches_cell(
        &self,
        raw: &str,
        rendered: &str,
        query: Option<&CaseInsensitiveQuery<'_>>,
    ) -> bool {
        let Some(query) = query else {
            return false;
        };
        if rendered == raw {
            query.matches(raw)
        } else {
            query.matches(raw) || query.matches(rendered)
        }
    }

    pub fn output_conditional_color(&self, row: usize, visible_column: usize) -> Option<String> {
        let source_column = self.source_column_for_visible(visible_column)?;
        let source_row = self.source_row_for_visible_row(row)?;
        let raw = self
            .rows
            .get(source_row)
            .and_then(|row| row.get(source_column).map(String::as_str))
            .unwrap_or_default();
        let rendered = self.render_source_cell(source_column, Some(raw));
        self.conditional_color_for_source_cell(source_column, raw, &rendered)
            .map(Cow::into_owned)
    }

    #[cfg(all(test, feature = "saved-views"))]
    fn conditional_color_for_visible_cell(
        &self,
        row: usize,
        visible_column: usize,
    ) -> Option<String> {
        self.output_conditional_color(row, visible_column)
    }

    fn conditional_color_for_source_cell<'a>(
        &'a self,
        source_column: usize,
        raw: &str,
        rendered: &str,
    ) -> Option<Cow<'a, str>> {
        let rules = self.column_color_rules.get(source_column)?;
        if rules.is_empty() {
            return None;
        }
        let metadata = self.column_color_metadata.get(source_column);
        let min_max = metadata.and_then(|metadata| metadata.numeric_min_max);
        let mut numeric = None;

        rules
            .iter()
            .enumerate()
            .find_map(|(rule_idx, rule)| match rule {
                ConditionalColorRule::Identifiers { .. } => metadata
                    .and_then(|metadata| metadata.identifier_color_refs.get(&rule_idx))
                    .and_then(|color_refs| color_refs.get(rendered))
                    .map(|color_ref| Cow::Borrowed(color_ref.as_str())),
                ConditionalColorRule::AutoGradient { colors, steps } => {
                    let numeric = *numeric.get_or_insert_with(|| {
                        parse_numeric_scalar(raw, self.source_numeric_column_profile(source_column))
                    });
                    let value = numeric?;
                    let (min, max) = min_max?;
                    if colors.is_empty() || max <= min {
                        return colors.first().map(|color| Cow::Borrowed(color.as_str()));
                    }
                    let steps = (*steps).max(1);
                    let ratio = ((value - min) / (max - min)).clamp(0.0, 1.0);
                    let bucket = (ratio * steps as f64).floor().min((steps - 1) as f64) as usize;
                    metadata
                        .and_then(|metadata| metadata.gradient_color_refs.get(&rule_idx))
                        .and_then(|color_refs| color_refs.get(bucket))
                        .map(|color_ref| Cow::Borrowed(color_ref.as_str()))
                        .or_else(|| Some(Cow::Owned(gradient_color_ref(colors, bucket, steps))))
                }
                _ => {
                    let numeric = *numeric.get_or_insert_with(|| {
                        parse_numeric_scalar(raw, self.source_numeric_column_profile(source_column))
                    });
                    rule.color_ref_for(raw, rendered, numeric, min_max)
                }
            })
    }

    pub fn visible_row(&self, row: usize) -> Option<&Vec<String>> {
        self.visible_rows
            .get(row)
            .and_then(|row_idx| self.rows.get(*row_idx))
    }

    pub fn source_row_for_visible_row(&self, row: usize) -> Option<usize> {
        self.visible_rows.get(row).copied()
    }

    pub fn cursor(&self) -> Position {
        self.cursor
    }

    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    pub fn column_gap(&self) -> usize {
        self.column_gap
    }

    pub fn column_width_mode(&self) -> ColumnWidthMode {
        self.column_width_mode
    }

    pub(crate) fn is_numeric_column(&self, column: usize) -> bool {
        self.source_column_for_visible(column)
            .is_some_and(|source_column| self.columns.is_numeric(ColumnIndex::new(source_column)))
    }

    fn source_numeric_column_profile(&self, source_column: usize) -> NumericColumnProfile {
        self.columns
            .numeric_profile(ColumnIndex::new(source_column))
    }

    fn reparse_numeric_filters(&mut self, rows: &[Vec<String>]) {
        let columns = Columns::infer(self.header.as_deref(), rows);
        for filter in &mut self.filters {
            if filter.kind != FilterKind::Numeric {
                continue;
            }
            if let Ok(condition) = FilterCondition::parse(
                FilterKind::Numeric,
                &filter.input,
                columns.numeric_profile(ColumnIndex::new(filter.column)),
            ) {
                filter.condition = condition;
            }
        }
    }

    pub(crate) fn filter_kind_enabled(&self, column: usize, kind: FilterKind) -> bool {
        kind != FilterKind::Numeric || self.is_numeric_column(column)
    }

    pub(crate) fn default_filter_kind(&self, column: usize) -> FilterKind {
        if self.is_numeric_column(column) {
            FilterKind::Numeric
        } else {
            FilterKind::Text
        }
    }

    pub fn filtered_columns(&self) -> Vec<usize> {
        let mut columns = self
            .filters
            .iter()
            .filter_map(|filter| self.visible_column_for_source(filter.column))
            .collect::<Vec<_>>();
        columns.sort_unstable();
        columns.dedup();
        columns
    }

    pub fn column_has_filter(&self, column: usize) -> bool {
        self.source_column_for_visible(column)
            .is_some_and(|source_column| self.source_column_has_filter(source_column))
    }

    fn source_column_has_filter(&self, source_column: usize) -> bool {
        self.filters
            .iter()
            .any(|filter| filter.column == source_column)
    }

    fn source_column_filter_indicator(&self, source_column: usize) -> Option<&'static str> {
        let mut filters = self
            .filters
            .iter()
            .filter(|filter| filter.column == source_column);
        let first = filters.next()?;
        if filters.next().is_some() {
            return Some("±");
        }
        Some(match first.mode {
            FilterMode::In => "+",
            FilterMode::Out => "-",
        })
    }

    fn source_column_sort_indicator(&self, source_column: usize) -> Option<&'static str> {
        self.sort_keys
            .iter()
            .find(|key| key.column == source_column)
            .map(|key| match key.direction {
                SortDirection::Ascending => "▲",
                SortDirection::Descending => "▼",
            })
    }

    pub fn mark(&self) -> Option<Position> {
        self.mark
    }

    pub fn resize_viewport(&mut self, height: usize, width: usize) {
        self.viewport.height = height.max(1);
        self.viewport.width = width.max(1);
        self.keep_cursor_visible();
        let last_visible_row = self
            .viewport
            .origin
            .row
            .saturating_add(self.viewport.height.saturating_sub(1));
        let _ = self.ensure_source_indexed_through(last_visible_row);
    }

    pub fn set_terminal_width(&mut self, width: usize) {
        let width = width.max(1);
        if self.terminal_width != width {
            self.terminal_width = width;
            self.computed_column_widths_cache.clear();
        }
    }

    pub fn toggle_header(&mut self) {
        if self.header.is_some() {
            self.header_visible = !self.header_visible;
        }
    }

    pub fn goto(&mut self, row: usize, column: usize) {
        let _ = self.ensure_source_indexed_through(row);
        self.cursor.row = row.min(self.visible_rows.len().saturating_sub(1));
        self.cursor.column = column.min(self.column_count().saturating_sub(1));
        self.keep_cursor_visible();
    }

    pub fn move_by(&mut self, row_delta: isize, column_delta: isize) {
        let row = self.cursor.row.saturating_add_signed(row_delta);
        let column = self.cursor.column.saturating_add_signed(column_delta);
        self.goto(row, column);
    }

    pub fn page_by(&mut self, row_pages: isize, column_pages: isize, count: usize) {
        let count = count.max(1) as isize;
        let row_delta = row_pages * self.viewport.height.max(1) as isize * count;
        let column_delta = column_pages * self.viewport.width.max(1) as isize * count;
        self.move_by(row_delta, column_delta);
    }

    pub fn goto_top(&mut self) {
        self.goto(0, self.cursor.column);
    }

    pub fn goto_bottom(&mut self) {
        let _ = self.ensure_source_indexed_through(usize::MAX);
        self.goto(
            self.visible_rows.len().saturating_sub(1),
            self.cursor.column,
        );
    }

    pub fn goto_user_row(&mut self, row: usize) {
        self.goto(row.saturating_sub(1), self.cursor.column);
    }

    pub fn goto_user_column(&mut self, column: usize) {
        self.goto(self.cursor.row, column.saturating_sub(1));
    }

    pub fn set_mark(&mut self) {
        self.mark = Some(self.cursor);
        self.mark_identity = self
            .visible_rows
            .get(self.cursor.row)
            .and_then(|row| self.row_ids.get(*row))
            .copied()
            .zip(
                self.source_column_for_visible(self.cursor.column)
                    .and_then(|column| {
                        self.table_definition
                            .as_ref()
                            .and_then(|definition| definition.columns.get(column))
                            .map(|column| column.id)
                    }),
            );
    }

    pub fn goto_mark(&mut self) {
        if let Some((row_id, column_id)) = self.mark_identity {
            let row = self
                .row_ids
                .iter()
                .position(|candidate| *candidate == row_id);
            let column = self
                .table_definition
                .as_ref()
                .and_then(|definition| {
                    definition
                        .columns
                        .iter()
                        .position(|column| column.id == column_id)
                })
                .and_then(|source| self.visible_column_for_source(source));
            if let (Some(row), Some(column)) = (row, column) {
                self.goto(row, column);
                self.mark = Some(self.cursor);
            }
            return;
        }
        if let Some(mark) = self.mark {
            self.goto(mark.row, mark.column);
        }
    }

    pub fn column_count(&self) -> usize {
        self.visible_source_columns().len()
    }

    pub fn source_column_count(&self) -> usize {
        self.columns.len()
    }

    pub fn hidden_column_count(&self) -> usize {
        self.hidden_columns.len()
    }

    #[cfg(feature = "saved-views")]
    pub fn is_numeric_source_column(&self, source_column: usize) -> bool {
        self.columns.is_numeric(ColumnIndex::new(source_column))
    }

    #[cfg(feature = "saved-views")]
    pub fn type_sort_mode_for_source(&self, source_column: usize) -> SortMode {
        self.sort_mode_for_source(source_column, SortMode::Lexical)
    }

    pub fn sort_current_column(&mut self, mode: SortMode, direction: SortDirection) {
        let Some(column) = self.source_column_for_visible(self.cursor.column) else {
            return;
        };
        let mode = self.sort_mode_for_source(column, mode);
        self.activate_sort_key(column, mode, direction);
        self.computed_column_widths_cache.clear();
        self.apply_query_configuration();
        self.keep_cursor_visible();
    }

    pub fn clear_current_column_sort(&mut self) {
        let Some(column) = self.source_column_for_visible(self.cursor.column) else {
            return;
        };
        self.sort_keys.retain(|key| key.column != column);
        self.computed_column_widths_cache.clear();
        self.apply_query_configuration();
        self.keep_cursor_visible();
    }

    pub fn sort_keys(&self) -> &[ActiveSortKey] {
        &self.sort_keys
    }

    fn activate_sort_key(&mut self, column: usize, mode: SortMode, direction: SortDirection) {
        if self.sort_keys.first().is_some_and(|key| {
            key.column == column
                && key.mode == mode
                && key.direction == direction
                && key.nulls == self.resolved_null_placement(column)
        }) {
            self.sort_keys.remove(0);
            return;
        }

        self.sort_keys.retain(|key| key.column != column);
        self.sort_keys.insert(
            0,
            ActiveSortKey {
                column,
                mode,
                direction,
                nulls: self.resolved_null_placement(column),
            },
        );
        self.sort_keys.truncate(MAX_ACTIVE_SORT_KEYS);
    }

    fn apply_query_configuration(&mut self) -> bool {
        if self.preview_preparing {
            return true;
        }
        match self.refresh_view_transform() {
            QueryRefresh::Applied => return true,
            QueryRefresh::Failed => {
                self.filters = self.committed_filters.clone();
                self.sort_keys = self.committed_sort_keys.clone();
                return false;
            }
            QueryRefresh::NotStoreBacked => {}
        }

        let specs = self
            .sort_keys
            .iter()
            .map(|key| SortSpec {
                column: key.column,
                mode: key.mode,
                direction: key.direction,
                numeric_profile: self.source_numeric_column_profile(key.column),
            })
            .collect::<Vec<_>>();
        sort_rows_by_specs(&mut self.rows, &specs);
        self.visible_rows = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(row_idx, row)| self.row_passes_filters(row).then_some(row_idx))
            .collect();
        self.committed_filters = self.filters.clone();
        self.committed_sort_keys = self.sort_keys.clone();
        self.keep_cursor_visible();
        true
    }

    fn refresh_view_transform(&mut self) -> QueryRefresh {
        let Some(definition) = self.table_definition.clone() else {
            return QueryRefresh::NotStoreBacked;
        };
        let selected_id = self
            .visible_rows
            .get(self.cursor.row)
            .and_then(|index| self.row_ids.get(*index))
            .copied();
        let query = crate::table::ViewTransform {
            generation: definition.generation,
            filters: self
                .filters
                .iter()
                .filter_map(|filter| {
                    let column = definition.columns.get(filter.column)?.id;
                    let mode = match filter.mode {
                        FilterMode::In => crate::table::FilterMode::In,
                        FilterMode::Out => crate::table::FilterMode::Out,
                    };
                    let predicate = match &filter.condition {
                        FilterCondition::Text(value) => crate::table::ViewFilterPredicate::Text {
                            value: value.clone(),
                            domain: crate::table::ValueDomain::RawOrRendered,
                        },
                        FilterCondition::Regex(regex) => crate::table::ViewFilterPredicate::Regex {
                            pattern: regex.as_str().to_owned(),
                            domain: crate::table::ValueDomain::RawOrRendered,
                        },
                        FilterCondition::Numeric { operator, operand } => {
                            crate::table::ViewFilterPredicate::Numeric {
                                operator: match operator {
                                    crate::ops::filter::NumericOperator::LessThan => {
                                        crate::table::NumericOperator::LessThan
                                    }
                                    crate::ops::filter::NumericOperator::LessThanOrEqual => {
                                        crate::table::NumericOperator::LessThanOrEqual
                                    }
                                    crate::ops::filter::NumericOperator::GreaterThan => {
                                        crate::table::NumericOperator::GreaterThan
                                    }
                                    crate::ops::filter::NumericOperator::GreaterThanOrEqual => {
                                        crate::table::NumericOperator::GreaterThanOrEqual
                                    }
                                    crate::ops::filter::NumericOperator::Equal => {
                                        crate::table::NumericOperator::Equal
                                    }
                                },
                                operand: *operand,
                            }
                        }
                    };
                    Some(crate::table::ViewFilter {
                        column,
                        mode,
                        predicate,
                    })
                })
                .collect(),
            order_by: self
                .sort_keys
                .iter()
                .filter_map(|key| {
                    Some(crate::table::ViewSort {
                        column: definition.columns.get(key.column)?.id,
                        mode: table_sort_mode(key.mode),
                        direction: match key.direction {
                            SortDirection::Ascending => crate::table::SortDirection::Ascending,
                            SortDirection::Descending => crate::table::SortDirection::Descending,
                        },
                        nulls: key.nulls,
                    })
                })
                .collect(),
        };

        if let Err(error) = crate::table::validate_view_transform(&definition, &query) {
            self.source_status = Some(format!("Query validation failed: {error}"));
            return QueryRefresh::Failed;
        }

        if query.filters.is_empty() && query.order_by.is_empty() {
            let base_rows = self.source_store.as_ref().map(|base| {
                (
                    base.rows()
                        .iter()
                        .map(crate::table::Row::display_cells)
                        .collect::<Vec<_>>(),
                    base.rows().iter().map(|row| row.id).collect::<Vec<_>>(),
                )
            });
            let cached_rows = self
                .base_cached_rows
                .clone()
                .zip(self.base_cached_row_ids.clone());
            if let Some((rows, row_ids)) = base_rows.or(cached_rows) {
                self.rows = rows;
                self.row_ids = row_ids;
            }
            self.query_store = None;
            self.base_cached_rows = None;
            self.base_cached_row_ids = None;
            self.visible_rows = (0..self.rows.len()).collect();
            self.active_view_transform = Some(query);
            self.committed_filters = self.filters.clone();
            self.committed_sort_keys = self.sort_keys.clone();
            self.restore_selected_row(selected_id);
            return QueryRefresh::Applied;
        }

        let Some(shared) = self.incremental_store.clone() else {
            return QueryRefresh::NotStoreBacked;
        };
        let base = if let Some(base) = self.source_store.clone() {
            base
        } else {
            self.source_status = Some("Materializing bounded source result".to_owned());
            match shared.0.borrow_mut().materialize() {
                Ok(base) => base,
                Err(error) => {
                    self.source_status = Some(format!(
                        "Materialization failed; prior view retained: {error}"
                    ));
                    return QueryRefresh::Failed;
                }
            }
        };
        let base_rows = base
            .rows()
            .iter()
            .map(crate::table::Row::display_cells)
            .collect::<Vec<_>>();
        let base_columns = Columns::infer(self.header.as_deref(), &base_rows);
        let numeric_profiles = definition
            .columns
            .iter()
            .map(|column| {
                base_columns.numeric_profile(ColumnIndex::new(column.id.ordinal as usize))
            })
            .collect::<Vec<_>>();
        let result = match crate::table::execute_local_view_transform_with_profiles(
            &base,
            &definition,
            &query,
            &|column| {
                numeric_profiles
                    .get(column.ordinal as usize)
                    .copied()
                    .unwrap_or_default()
            },
            &|_, value| value.display().into_owned(),
        ) {
            Ok(result) => result,
            Err(error) => {
                self.source_status = Some(format!(
                    "Query execution failed; prior view retained: {error}"
                ));
                return QueryRefresh::Failed;
            }
        };
        self.source_store = Some(base);
        self.rows = result
            .rows()
            .iter()
            .map(crate::table::Row::display_cells)
            .collect();
        self.row_ids = result.rows().iter().map(|row| row.id).collect();
        self.visible_rows = (0..self.rows.len()).collect();
        self.query_store = None;

        self.active_view_transform = Some(query);
        self.committed_filters = self.filters.clone();
        self.committed_sort_keys = self.sort_keys.clone();
        self.restore_selected_row(selected_id);
        QueryRefresh::Applied
    }

    fn restore_selected_row(&mut self, selected_id: Option<crate::table::RowId>) {
        if let Some(selected_id) = selected_id {
            if let Some(position) = self.row_ids.iter().position(|id| *id == selected_id) {
                self.cursor.row = position;
            }
        }
        self.cursor.row = self.cursor.row.min(self.rows.len().saturating_sub(1));
    }

    fn current_stable_row_identity(&mut self) -> Option<crate::table::StableRowIdentity> {
        let source_index = self
            .visible_rows
            .get(self.cursor.row)
            .and_then(|row| self.row_ids.get(*row))
            .map(|id| id.ordinal as usize)?;
        self.incremental_store
            .as_ref()?
            .0
            .borrow_mut()
            .stable_row_identity(RowIndex(source_index))
            .ok()
            .flatten()
    }

    fn mark_stable_row_identity(&mut self) -> Option<crate::table::StableRowIdentity> {
        let source_index = self.mark_identity?.0.ordinal as usize;
        self.incremental_store
            .as_ref()?
            .0
            .borrow_mut()
            .stable_row_identity(RowIndex(source_index))
            .ok()
            .flatten()
    }

    fn restore_stable_identities(
        &mut self,
        cursor_identity: Option<crate::table::StableRowIdentity>,
        mark_identity: Option<crate::table::StableRowIdentity>,
    ) {
        if !self.view_transform_is_active()
            && (cursor_identity.is_some() || mark_identity.is_some())
        {
            let mut cursor_found = false;
            let mut mark_found = false;
            if let Some(store) = &self.incremental_store {
                let mut store = store.0.borrow_mut();
                let mut index = self.row_ids.len();
                loop {
                    let source_row = match store.row(RowIndex(index)) {
                        Ok(Some(row)) => row,
                        Ok(None) | Err(_) => break,
                    };
                    let identity = store.stable_row_identity(RowIndex(index)).ok().flatten();
                    cursor_found |= identity.as_ref() == cursor_identity.as_ref();
                    mark_found |= identity.as_ref() == mark_identity.as_ref();
                    self.row_ids.push(source_row.id);
                    let mut cells = source_row.display_cells();
                    cells.resize(self.source_column_count(), String::new());
                    self.rows.push(cells);
                    index = index.saturating_add(1);
                    if cursor_identity.as_ref().is_none_or(|_| cursor_found)
                        && mark_identity.as_ref().is_none_or(|_| mark_found)
                    {
                        break;
                    }
                }
            }
            self.visible_rows = (0..self.rows.len()).collect();
        }
        let identities = if let Some(store) = &self.incremental_store {
            let mut store = store.0.borrow_mut();
            self.row_ids
                .iter()
                .map(|id| {
                    store
                        .stable_row_identity(RowIndex(id.ordinal as usize))
                        .ok()
                        .flatten()
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        if let Some(cursor_identity) = cursor_identity {
            if let Some(row) = identities
                .iter()
                .position(|identity| identity.as_ref() == Some(&cursor_identity))
            {
                self.cursor.row = row;
            } else {
                self.cursor.row = 0;
            }
        } else {
            self.cursor.row = 0;
        }
        if let Some(mark_identity) = mark_identity {
            if let Some(row) = identities
                .iter()
                .position(|identity| identity.as_ref() == Some(&mark_identity))
            {
                if let Some((_, column_id)) = self.mark_identity {
                    let new_row_id = self.row_ids[row];
                    self.mark_identity = Some((new_row_id, column_id));
                    if let Some(mark) = &mut self.mark {
                        mark.row = row;
                    }
                }
            } else {
                self.mark = None;
                self.mark_identity = None;
            }
        } else {
            self.mark = None;
            self.mark_identity = None;
        }
        self.keep_cursor_visible();
    }

    pub fn active_view_transform(&self) -> Option<&crate::table::ViewTransform> {
        self.active_view_transform.as_ref()
    }

    fn sort_mode_for_source(&self, source_column: usize, requested: SortMode) -> SortMode {
        if requested != SortMode::Lexical {
            return requested;
        }
        match self
            .column_display
            .get(source_column)
            .map(|metadata| metadata.column_type)
            .unwrap_or_default()
        {
            #[cfg(feature = "saved-views")]
            ColumnTypeMetadata::Date => SortMode::Date,
            #[cfg(feature = "saved-views")]
            ColumnTypeMetadata::Ip => SortMode::Ip,
            #[cfg(feature = "saved-views")]
            ColumnTypeMetadata::SemVer => SortMode::SemVer,
            #[cfg(feature = "saved-views")]
            ColumnTypeMetadata::BooleanWord
            | ColumnTypeMetadata::BooleanChar
            | ColumnTypeMetadata::BooleanBit => SortMode::Boolean,
            _ => requested,
        }
    }

    pub(crate) fn apply_filter(
        &mut self,
        column: usize,
        mode: FilterMode,
        kind: FilterKind,
        input: String,
    ) -> Result<(), FilterParseError> {
        let Some(source_column) = self.source_column_for_visible(column) else {
            return Ok(());
        };
        if kind == FilterKind::Numeric && !self.is_numeric_column(column) {
            return Err(FilterParseError::NumericUnavailable);
        }
        let condition = FilterCondition::parse(
            kind,
            &input,
            self.source_numeric_column_profile(source_column),
        )?;
        self.filters.push(ActiveFilter::new(
            source_column,
            mode,
            kind,
            input,
            condition,
        ));
        self.computed_column_widths_cache.clear();
        self.apply_query_configuration();
        Ok(())
    }

    pub fn clear_filters_for_column(&mut self, column: usize) {
        if let Some(source_column) = self.source_column_for_visible(column) {
            self.filters.retain(|filter| filter.column != source_column);
        }
        self.computed_column_widths_cache.clear();
        self.apply_query_configuration();
    }

    pub fn set_column_gap(&mut self, gap: usize) {
        self.column_gap = gap;
        self.computed_column_widths_cache.clear();
    }

    pub fn adjust_column_gap(&mut self, delta: isize) {
        self.column_gap = self.column_gap.saturating_add_signed(delta);
        self.computed_column_widths_cache.clear();
    }

    pub fn set_column_width_mode(&mut self, mode: ColumnWidthMode) {
        self.column_width_mode = mode;
        self.column_widths.clear();
        self.sampled_column_widths.clear();
        self.computed_column_widths_cache.clear();
        self.column_width_modified.clear();
        if mode == ColumnWidthMode::Max {
            match self.exact_reduction_profiles() {
                Ok(Some(profiles)) => {
                    self.sampled_column_widths = profiles
                        .into_iter()
                        .map(|profile| profile.max_width.clamp(1, 250))
                        .collect();
                }
                Ok(None) => {}
                Err(error) => self.source_status = Some(format!("Profiling failed: {error}")),
            }
        }
    }

    pub fn toggle_variable_column_width_mode(&mut self) {
        self.column_width_mode = match self.column_width_mode {
            ColumnWidthMode::Mode => ColumnWidthMode::Max,
            ColumnWidthMode::Max | ColumnWidthMode::Fixed(_) => ColumnWidthMode::Mode,
        };
        self.column_widths.clear();
        self.sampled_column_widths.clear();
        self.computed_column_widths_cache.clear();
        self.column_width_modified.clear();
    }

    pub fn set_all_column_widths(&mut self, width: usize) {
        self.column_width_mode = ColumnWidthMode::Fixed(width.clamp(1, u16::MAX as usize) as u16);
        self.column_widths.clear();
        self.computed_column_widths_cache.clear();
        self.column_width_modified = (0..self.source_column_count()).collect();
    }

    pub fn set_current_column_width(&mut self, width: usize) {
        self.ensure_custom_column_widths();
        let Some(source_column) = self.source_column_for_visible(self.cursor.column) else {
            return;
        };
        if let Some(column_width) = self.column_widths.get_mut(source_column) {
            *column_width = width.max(1);
            self.column_width_modified.insert(source_column);
        }
    }

    pub fn maximize_current_column_width(&mut self) {
        let max_widths = self.computed_column_widths(ColumnWidthMode::Max);
        let Some(source_column) = self.source_column_for_visible(self.cursor.column) else {
            return;
        };
        if let Some(width) = max_widths.get(source_column) {
            self.set_current_column_width(*width);
        }
    }

    pub fn adjust_all_column_widths(&mut self, delta: isize) {
        self.ensure_custom_column_widths();
        for width in &mut self.column_widths {
            *width = width.saturating_add_signed(delta).max(1);
        }
        self.column_width_modified = (0..self.source_column_count()).collect();
    }

    pub fn adjust_current_column_width(&mut self, steps: isize) {
        if steps == 0 {
            return;
        }
        let Some(source_column) = self.source_column_for_visible(self.cursor.column) else {
            return;
        };
        let max_width = self
            .cached_rendered_value_width(source_column)
            .max(self.rendered_source_header_width(source_column));
        self.ensure_custom_column_widths();
        if let Some(width) = self.column_widths.get_mut(source_column) {
            for _ in 0..steps.unsigned_abs() {
                let adjustment = (*width / 5).max(1);
                *width = if steps.is_positive() {
                    if *width < max_width {
                        width.saturating_add(adjustment).min(max_width)
                    } else {
                        *width
                    }
                } else {
                    width.saturating_sub(adjustment).max(1)
                };
            }
            self.column_width_modified.insert(source_column);
        }
    }

    pub fn effective_column_widths(&self) -> Vec<usize> {
        let mut source_widths = if self.column_widths.len() == self.source_column_count() {
            self.column_widths.clone()
        } else if self.sampled_column_widths.len() == self.source_column_count() {
            self.sampled_column_widths.clone()
        } else {
            self.computed_column_widths(self.column_width_mode)
        };
        if self.column_widths.len() != self.source_column_count()
            && !matches!(self.column_width_mode, ColumnWidthMode::Fixed(_))
        {
            self.cap_automatic_widths(&mut source_widths);
        }
        self.visible_source_columns()
            .into_iter()
            .map(|source_column| source_widths.get(source_column).copied().unwrap_or(1))
            .collect()
    }

    pub fn effective_column_widths_cached(&mut self) -> Vec<usize> {
        if self.column_widths.len() == self.source_column_count() {
            let source_widths = &self.column_widths;
            return self
                .visible_source_columns_iter()
                .map(|source_column| source_widths.get(source_column).copied().unwrap_or(1))
                .collect();
        }
        if matches!(self.column_width_mode, ColumnWidthMode::Fixed(_)) {
            return self.effective_column_widths();
        }
        if self.sampled_column_widths.len() != self.source_column_count() {
            let observed = self.computed_column_widths(self.column_width_mode);
            if self.sampled_column_widths.is_empty() {
                self.sampled_column_widths = observed;
            } else {
                self.sampled_column_widths
                    .truncate(self.source_column_count());
                let sampled_len = self.sampled_column_widths.len();
                self.sampled_column_widths
                    .extend(observed.into_iter().skip(sampled_len));
            }
        }
        if self.computed_column_widths_cache.len() != self.source_column_count() {
            self.computed_column_widths_cache = self.sampled_column_widths.clone();
            let cap = self.automatic_column_width_cap();
            for width in &mut self.computed_column_widths_cache {
                *width = (*width).min(cap);
            }
        }
        let source_widths = &self.computed_column_widths_cache;
        self.visible_source_columns_iter()
            .map(|source_column| source_widths.get(source_column).copied().unwrap_or(1))
            .collect()
    }

    /// Explicit per-column widths for document output. Automatic widths are
    /// intentionally unspecified and are measured by the selected adapter.
    pub fn output_column_width_overrides(&self) -> Vec<Option<usize>> {
        match self.column_width_mode {
            ColumnWidthMode::Fixed(width) => {
                vec![Some(width as usize); self.column_count()]
            }
            ColumnWidthMode::Mode | ColumnWidthMode::Max => self
                .visible_source_columns_iter()
                .map(|source_column| {
                    self.column_width_modified
                        .contains(&source_column)
                        .then(|| self.column_widths.get(source_column).copied())
                        .flatten()
                })
                .collect(),
        }
    }

    /// Finish indexing/query execution and load the complete logical result
    /// without consulting the viewport. This is the preparation boundary used
    /// by output adapters and by post-interactive serialization.
    pub(crate) fn defer_preview_preparation(&mut self) {
        self.preview_preparing = true;
    }

    pub(crate) fn prepare_preview(
        &mut self,
        limit: usize,
        colored: bool,
        full_schema: bool,
    ) -> anyhow::Result<Option<crate::table::RowCount>> {
        let needs_full_materialization = self
            .filters
            .iter()
            .any(|filter| filter.kind == FilterKind::Numeric);
        #[cfg(feature = "saved-views")]
        let needs_full_materialization = needs_full_materialization
            || self
                .pending_saved_filters
                .iter()
                .any(|filter| matches!(filter.kind, crate::saved_views::FilterKind::Numeric));
        let has_sort = !self.sort_keys.is_empty();
        let materialize_before_filtering = has_sort || needs_full_materialization;
        #[cfg(feature = "saved-views")]
        let has_sort = has_sort || !self.pending_saved_sorts.is_empty();
        let remaining;
        if materialize_before_filtering || has_sort {
            // Discover the full sorting schema before a query store consumes its deltas.
            if let Some(shared) = self.incremental_store.clone() {
                let progress = shared
                    .0
                    .borrow_mut()
                    .ensure_indexed_through(RowIndex(usize::MAX))?;
                self.apply_source_schema_delta(progress.schema_delta)?;
                if needs_full_materialization {
                    let base = shared.0.borrow_mut().materialize()?;
                    let rows = base
                        .rows()
                        .iter()
                        .map(crate::table::Row::display_cells)
                        .collect::<Vec<_>>();
                    self.reparse_numeric_filters(&rows);
                    self.source_store = Some(base);
                }
            }
            self.preview_preparing = false;
            self.apply_query_configuration();
            self.complete_for_output_with_color(false)?;
            let total = self.rows.len();
            self.rows.truncate(limit);
            self.row_ids.truncate(limit);
            self.visible_rows = (0..self.rows.len()).collect();
            remaining = (total > limit).then_some(
                if has_sort || needs_full_materialization || self.filters.is_empty() {
                    crate::table::RowCount::Exact(total.saturating_sub(limit))
                } else {
                    crate::table::RowCount::Unknown
                },
            );
        } else {
            let shared = self
                .incremental_store
                .clone()
                .ok_or_else(|| anyhow::anyhow!("preview requires a source store"))?;
            if full_schema {
                let progress = shared
                    .0
                    .borrow_mut()
                    .ensure_indexed_through(RowIndex(usize::MAX))?;
                self.apply_source_schema_delta(progress.schema_delta)?;
            }
            let mut selected: Vec<Vec<String>> = Vec::new();
            let mut ids = Vec::new();
            let mut index = 0;
            let mut more = false;
            let mut matched_total: usize = 0;
            let mut prefix_state = None;
            let mut deferred_until = None;
            loop {
                // Retain the emitted schema while lookahead checks later matching rows.
                if selected.len() == limit && prefix_state.is_none() {
                    prefix_state = Some(self.clone());
                }
                #[cfg(feature = "saved-views")]
                let pending_before = !self.pending_saved_filters.is_empty();
                let progress = shared
                    .0
                    .borrow_mut()
                    .ensure_indexed_through(RowIndex(index))?;
                self.apply_source_schema_delta(progress.schema_delta)?;
                #[cfg(feature = "saved-views")]
                let pending_resolved = pending_before && self.pending_saved_filters.is_empty();
                #[cfg(not(feature = "saved-views"))]
                let pending_resolved = false;
                if pending_resolved {
                    let deferred_end = deferred_until.take().unwrap_or_default();
                    for deferred_index in 0..deferred_end {
                        let Some(row) = shared.0.borrow_mut().row(RowIndex(deferred_index))? else {
                            break;
                        };
                        let row_id = row.id;
                        let cells = row.display_cells();
                        if self.row_passes_filters(&cells) {
                            matched_total += 1;
                            if selected.len() == limit {
                                more = true;
                                break;
                            }
                            ids.push(row_id);
                            selected.push(cells);
                        }
                    }
                    if more {
                        break;
                    }
                }
                let Some(row) = shared.0.borrow_mut().row(RowIndex(index))? else {
                    break;
                };
                index += 1;
                let cells = row.display_cells();
                #[cfg(feature = "saved-views")]
                if !self.pending_saved_filters.is_empty() {
                    deferred_until = Some(index);
                    continue;
                }
                if self.row_passes_filters(&cells) {
                    matched_total += 1;
                    if selected.len() == limit {
                        more = true;
                        break;
                    }
                    ids.push(row.id);
                    selected.push(cells);
                }
            }
            let count = shared.0.borrow().row_count();
            remaining = if more {
                match count {
                    crate::table::RowCount::Exact(total)
                        if self.filters.is_empty() || index >= total =>
                    {
                        let remainder = if self.filters.is_empty() {
                            total.saturating_sub(limit)
                        } else {
                            matched_total.saturating_sub(limit)
                        };
                        Some(crate::table::RowCount::Exact(remainder))
                    }
                    _ => Some(crate::table::RowCount::Unknown),
                }
            } else {
                None
            };
            if let Some(prefix) = prefix_state {
                *self = prefix;
            }
            self.rows = selected;
            self.row_ids = ids;
        }
        let source_filter_active = self.incremental_store.as_ref().is_some_and(|shared| {
            let store = shared.0.borrow();
            store.has_source_filters()
                || store
                    .active_source_query()
                    .is_some_and(|query| !query.filters.is_empty())
        });
        if (!full_schema || source_filter_active) && !self.row_ids.is_empty() {
            if let Some(shared) = &self.incremental_store {
                let mut store = shared.0.borrow_mut();
                let row_ids = if full_schema && source_filter_active {
                    let mut row_ids = Vec::new();
                    store.scan_rows(
                        crate::table::ScanRequest {
                            start: RowIndex(0),
                            direction: crate::table::ScanDirection::Forward,
                            max_rows: usize::MAX,
                        },
                        &mut |_, row: &crate::table::Row| {
                            row_ids.push(row.id);
                            std::ops::ControlFlow::Continue(())
                        },
                    )?;
                    row_ids
                } else {
                    self.row_ids.clone()
                };
                let present = row_ids
                    .iter()
                    .map(|id| store.present_columns(*id))
                    .collect::<Option<Vec<_>>>();
                if let Some(present) = present {
                    let present = present.into_iter().flatten().collect::<BTreeSet<_>>();
                    self.hidden_columns.extend(
                        (0..self.source_column_count()).filter(|column| !present.contains(column)),
                    );
                }
            }
        }
        let columns = self.source_column_count();
        for row in &mut self.rows {
            row.resize(columns, String::new());
        }
        // The output projection must no longer pull rows or profile the source.
        self.incremental_store = None;
        self.source_store = None;
        self.query_store = None;
        self.active_view_transform = None;
        self.preview_preparing = false;
        self.visible_rows = (0..self.rows.len()).collect();
        self.columns = Columns::infer(self.header.as_deref(), &self.rows);
        if colored {
            self.rebuild_column_color_metadata();
        }
        #[cfg(feature = "saved-views")]
        self.resolve_saved_column_widths();
        Ok(remaining)
    }

    pub fn complete_for_output(&mut self) -> anyhow::Result<()> {
        self.complete_for_output_with_color(true)
    }

    pub(crate) fn complete_for_output_with_color(&mut self, colored: bool) -> anyhow::Result<()> {
        if let Some(shared) = self.incremental_store.clone() {
            let progress = shared
                .0
                .borrow_mut()
                .ensure_indexed_through(RowIndex(usize::MAX))?;
            self.apply_source_schema_delta(progress.schema_delta)?;
        }

        if self.view_transform_is_active() {
            match self.refresh_view_transform() {
                QueryRefresh::Applied | QueryRefresh::NotStoreBacked => {}
                QueryRefresh::Failed => {
                    anyhow::bail!(
                        "{}",
                        self.source_status
                            .clone()
                            .unwrap_or_else(|| "query preparation failed".to_owned())
                    );
                }
            }
        }

        let selected_store = if self.view_transform_is_active() {
            self.query_store.clone()
        } else {
            self.incremental_store.clone()
        };
        if let Some(shared) = selected_store {
            let materialized = shared.0.borrow_mut().materialize()?;
            self.rows = materialized
                .rows()
                .iter()
                .map(crate::table::Row::display_cells)
                .collect();
            self.row_ids = materialized.rows().iter().map(|row| row.id).collect();
            self.visible_rows = (0..self.rows.len()).collect();
            if !self.view_transform_is_active() {
                self.source_store = Some(materialized);
            }
        }

        self.columns = Columns::infer(self.header.as_deref(), &self.rows);
        self.sampled_column_widths.clear();
        self.computed_column_widths_cache.clear();
        if colored {
            self.rebuild_column_color_metadata();
        }
        self.keep_cursor_visible();
        Ok(())
    }

    pub fn column_alignment_override(&self, column: usize) -> Option<ColumnAlignment> {
        let source_column = self.source_column_for_visible(column)?;
        self.column_alignment_overrides
            .get(source_column)
            .copied()
            .flatten()
    }

    pub fn column_alignment(&self, column: usize) -> ColumnAlignment {
        let Some(source_column) = self.source_column_for_visible(column) else {
            return ColumnAlignment::Left;
        };
        if let Some(alignment) = self
            .column_alignment_overrides
            .get(source_column)
            .copied()
            .flatten()
        {
            return alignment;
        }
        match self
            .column_display
            .get(source_column)
            .map(|metadata| metadata.column_type)
            .unwrap_or_default()
        {
            ColumnTypeMetadata::Float | ColumnTypeMetadata::Int => ColumnAlignment::Right,
            _ if self.columns.is_numeric(ColumnIndex::new(source_column)) => ColumnAlignment::Right,
            _ => ColumnAlignment::Left,
        }
    }

    pub fn current_column_info(&self) -> Option<ColumnInfo> {
        let visible_column = self.cursor.column;
        let source_column = self.source_column_for_visible(visible_column)?;
        let display = self
            .column_display
            .get(source_column)
            .copied()
            .unwrap_or_default();
        let sort = self
            .sort_keys
            .iter()
            .find(|key| key.column == source_column)
            .map(|key| match key.direction {
                SortDirection::Ascending => ColumnSortChoice::Ascending,
                SortDirection::Descending => ColumnSortChoice::Descending,
            })
            .unwrap_or(ColumnSortChoice::None);
        let filters = self
            .filters
            .iter()
            .filter(|filter| filter.column == source_column)
            .map(|filter| ColumnFilterSummary {
                mode: match filter.mode {
                    FilterMode::In => "in",
                    FilterMode::Out => "out",
                },
                kind: match filter.kind {
                    FilterKind::Text => "text",
                    FilterKind::Regex => "regex",
                    FilterKind::Numeric => "numeric",
                },
                input: filter.input.clone(),
            })
            .collect();
        Some(ColumnInfo {
            visible_column,
            source_column,
            name: self
                .header
                .as_ref()
                .and_then(|header| header.get(source_column))
                .cloned()
                .unwrap_or_else(|| format!("Column {}", source_column + 1)),
            visible: self.source_column_visible(source_column),
            alignment: self
                .column_alignment_overrides
                .get(source_column)
                .copied()
                .flatten(),
            column_type: column_type_choice(display.column_type),
            format: column_format_choice(display.format),
            sort,
            nulls: match self.column_nulls.get(source_column).copied().flatten() {
                None => ColumnNullPlacementChoice::Inherited,
                Some(NullPlacement::First) => ColumnNullPlacementChoice::First,
                Some(NullPlacement::Last) => ColumnNullPlacementChoice::Last,
            },
            canonical_source: self
                .table_definition
                .as_ref()
                .and_then(|definition| definition.canonical_column_key(source_column)),
            source_type: self
                .table_definition
                .as_ref()
                .and_then(|definition| definition.columns.get(source_column))
                .map(|column| format!("{:?}", column.source_type)),
            source_declared_type: self
                .table_definition
                .as_ref()
                .and_then(|definition| definition.columns.get(source_column))
                .and_then(|column| column.source_declared_type.clone()),
            source_operations: self
                .table_definition
                .as_ref()
                .and_then(|definition| definition.columns.get(source_column))
                .map(|column| self.source_operations_for_column(column.id))
                .unwrap_or_default(),
            filters,
        })
    }

    pub fn apply_current_column_info(&mut self, update: ColumnInfoUpdate) {
        let Some(source_column) = self.source_column_for_visible(self.cursor.column) else {
            return;
        };

        if update.visible {
            self.hidden_columns.remove(&source_column);
        } else if self.column_count() > 1 {
            self.hidden_columns.insert(source_column);
        }

        if let Some(slot) = self.column_alignment_overrides.get_mut(source_column) {
            *slot = update.alignment;
        }
        if let Some(display) = self.column_display.get_mut(source_column) {
            display.column_type = column_type_metadata_from_choice(update.column_type);
            display.format = display_format_metadata_from_choice(update.format);
            display.mask = None;
        }
        if let Some(slot) = self.column_nulls.get_mut(source_column) {
            *slot = match update.nulls {
                ColumnNullPlacementChoice::Inherited => None,
                ColumnNullPlacementChoice::First => Some(NullPlacement::First),
                ColumnNullPlacementChoice::Last => Some(NullPlacement::Last),
            };
        }
        self.column_metadata_modified.insert(source_column);
        self.rebuild_column_color_metadata_for(source_column);

        if update.clear_filters {
            self.filters.retain(|filter| filter.column != source_column);
        }

        self.sort_keys.retain(|key| key.column != source_column);
        match update.sort {
            ColumnSortChoice::None => {}
            ColumnSortChoice::Ascending | ColumnSortChoice::Descending => {
                self.activate_sort_key(
                    source_column,
                    self.sort_mode_for_source(source_column, SortMode::Lexical),
                    match update.sort {
                        ColumnSortChoice::Ascending => SortDirection::Ascending,
                        ColumnSortChoice::Descending => SortDirection::Descending,
                        ColumnSortChoice::None => unreachable!(),
                    },
                );
            }
        }

        self.sampled_column_widths.clear();
        self.computed_column_widths_cache.clear();
        self.apply_query_configuration();
        self.keep_cursor_visible();
    }

    pub fn restore_view_settings_from(&mut self, previous: &TableView) {
        let source_column_count = self.source_column_count();
        let previous_column_count = previous.source_column_count();
        let remap = (0..previous_column_count)
            .map(|old_index| {
                let old_identity = previous
                    .table_definition
                    .as_ref()
                    .and_then(|definition| definition.columns.get(old_index))
                    .map(|column| &column.source_identity);
                old_identity
                    .and_then(|identity| {
                        self.table_definition.as_ref().and_then(|definition| {
                            definition
                                .columns
                                .iter()
                                .position(|column| &column.source_identity == identity)
                        })
                    })
                    .or_else(|| (old_index < source_column_count).then_some(old_index))
            })
            .collect::<Vec<_>>();
        let previous_cursor_source = previous.source_column_for_visible(previous.cursor.column);

        self.column_width_mode = previous.column_width_mode;
        self.column_gap = previous.column_gap;
        self.column_widths = vec![1; source_column_count];
        for (old_index, width) in previous.column_widths.iter().copied().enumerate() {
            if let Some(new_index) = remap.get(old_index).copied().flatten() {
                self.column_widths[new_index] = width;
            }
        }
        if previous.column_widths.is_empty() {
            self.column_widths.clear();
        }
        self.sampled_column_widths.clear();
        self.computed_column_widths_cache.clear();
        self.column_width_modified = previous
            .column_width_modified
            .iter()
            .filter_map(|index| remap.get(*index).copied().flatten())
            .collect();
        self.hidden_columns = previous
            .hidden_columns
            .iter()
            .filter_map(|index| remap.get(*index).copied().flatten())
            .collect();
        if self.hidden_columns.len() >= source_column_count {
            self.hidden_columns.clear();
        }
        self.column_alignment_overrides = vec![None; source_column_count];
        self.column_label_overrides = vec![None; source_column_count];
        self.view_nulls = previous.view_nulls;
        self.column_nulls = vec![None; source_column_count];
        self.column_display = vec![ColumnDisplayMetadata::default(); source_column_count];
        self.column_color_rules = vec![Vec::new(); source_column_count];
        for (old_index, new_index) in remap
            .iter()
            .enumerate()
            .filter_map(|(old, new)| new.map(|new| (old, new)))
        {
            self.column_alignment_overrides[new_index] = previous
                .column_alignment_overrides
                .get(old_index)
                .copied()
                .flatten();
            self.column_label_overrides[new_index] = previous
                .column_label_overrides
                .get(old_index)
                .cloned()
                .flatten();
            self.column_nulls[new_index] = previous.column_nulls.get(old_index).copied().flatten();
            self.column_display[new_index] = previous
                .column_display
                .get(old_index)
                .copied()
                .unwrap_or_default();
            self.column_color_rules[new_index] = previous
                .column_color_rules
                .get(old_index)
                .cloned()
                .unwrap_or_default();
        }
        self.rebuild_column_color_metadata();
        self.column_metadata_modified = previous
            .column_metadata_modified
            .iter()
            .filter_map(|index| remap.get(*index).copied().flatten())
            .collect();
        self.sort_keys = previous
            .sort_keys
            .iter()
            .filter_map(|key| {
                Some(ActiveSortKey {
                    column: remap.get(key.column).copied().flatten()?,
                    ..*key
                })
            })
            .collect();
        if self.source_generation() == previous.source_generation() {
            self.mark = previous.mark;
            self.mark_identity = previous.mark_identity;
        } else {
            self.mark = None;
            self.mark_identity = None;
        }
        self.filters = previous
            .filters
            .iter()
            .filter_map(|filter| {
                let mut filter = filter.clone();
                filter.column = remap.get(filter.column).copied().flatten()?;
                Some(filter)
            })
            .collect();
        #[cfg(feature = "saved-views")]
        {
            self.pending_saved_columns = previous.pending_saved_columns.clone();
            self.saved_column_locale = previous.saved_column_locale.clone();
            self.pending_saved_sorts = previous.pending_saved_sorts.clone();
            self.pending_saved_filters = previous.pending_saved_filters.clone();
        }
        self.apply_query_configuration();
        if let Some(new_source_column) = previous_cursor_source
            .and_then(|old| remap.get(old).copied().flatten())
            .and_then(|source| self.visible_column_for_source(source))
        {
            self.cursor.column = new_source_column;
        }
    }

    pub fn hide_current_column(&mut self) {
        if self.column_count() <= 1 {
            return;
        }
        if let Some(source_column) = self.source_column_for_visible(self.cursor.column) {
            self.hidden_columns.insert(source_column);
            self.keep_cursor_visible();
        }
    }

    pub fn hide_columns_left(&mut self, count: usize) {
        let count = count.max(1);
        let current = self.cursor.column;
        let start = current.saturating_sub(count);
        let to_hide = (start..current)
            .filter_map(|column| self.source_column_for_visible(column))
            .collect::<Vec<_>>();
        for source_column in to_hide {
            self.hidden_columns.insert(source_column);
        }
        self.goto(
            self.cursor.row,
            start.min(self.column_count().saturating_sub(1)),
        );
    }

    pub fn hide_columns_right(&mut self, count: usize) {
        let count = count.max(1);
        let current = self.cursor.column;
        let to_hide = (current + 1..=current.saturating_add(count))
            .filter_map(|column| self.source_column_for_visible(column))
            .collect::<Vec<_>>();
        for source_column in to_hide {
            self.hidden_columns.insert(source_column);
        }
        self.keep_cursor_visible();
    }

    pub fn show_hidden_left(&mut self, count: usize) {
        let Some(current_source) = self.source_column_for_visible(self.cursor.column) else {
            return;
        };
        for (shown, source_column) in (0..current_source).rev().enumerate() {
            if shown >= count.max(1) || !self.hidden_columns.contains(&source_column) {
                break;
            }
            self.hidden_columns.remove(&source_column);
        }
        self.cursor.column = self
            .visible_column_for_source(current_source)
            .unwrap_or(self.cursor.column);
        self.keep_cursor_visible();
    }

    pub fn show_hidden_right(&mut self, count: usize) {
        let Some(current_source) = self.source_column_for_visible(self.cursor.column) else {
            return;
        };
        for (shown, source_column) in (current_source + 1..self.source_column_count()).enumerate() {
            if shown >= count.max(1) || !self.hidden_columns.contains(&source_column) {
                break;
            }
            self.hidden_columns.remove(&source_column);
        }
        self.keep_cursor_visible();
    }

    pub fn hidden_boundary_before(&self, column: usize) -> bool {
        let visible_columns = self.visible_source_columns();
        let Some(&source_column) = visible_columns.get(column) else {
            return false;
        };
        let previous_source = column
            .checked_sub(1)
            .and_then(|previous| visible_columns.get(previous).copied());
        let start = previous_source.map_or(0, |previous| previous + 1);
        (start..source_column).any(|source_column| self.hidden_columns.contains(&source_column))
    }

    pub fn hidden_boundary_after_last(&self) -> bool {
        let Some(last_visible) = self.visible_source_columns().last().copied() else {
            return false;
        };
        (last_visible + 1..self.source_column_count())
            .any(|source_column| self.hidden_columns.contains(&source_column))
    }

    #[cfg(feature = "saved-views")]
    pub fn apply_saved_columns(
        &mut self,
        resolved: &crate::saved_views::ResolvedColumns,
        locale: Option<&str>,
    ) {
        self.saved_column_widths
            .resize(self.source_column_count(), None);
        self.pending_saved_columns.extend(resolved.pending.clone());
        self.saved_column_locale = locale.map(ToOwned::to_owned);
        for (source_column, resolved_column) in resolved.columns.iter().enumerate() {
            let Some(label) = resolved_column
                .as_ref()
                .and_then(|resolved| resolved.view.label.as_ref())
            else {
                continue;
            };
            if let Some(header) = self.header.as_mut() {
                if let Some(name) = header.get_mut(source_column) {
                    *name = label.clone();
                }
            }
            if let Some(definition) = self.table_definition.as_mut() {
                if let Some(column) = definition.columns.get_mut(source_column) {
                    column.display_name = label.clone();
                }
            }
            if let Some(slot) = self.column_label_overrides.get_mut(source_column) {
                *slot = Some(label.clone());
            }
            self.column_metadata_modified.insert(source_column);
        }
        self.ensure_custom_column_widths();
        let header_widths = self.computed_header_widths();
        let content_widths = self.computed_content_widths();
        let mode_widths = self.computed_column_widths(ColumnWidthMode::Mode);
        let max_widths = self.computed_column_widths(ColumnWidthMode::Max);
        let locale = LocaleMetadata::from_posix(locale);

        for (source_column, resolved_column) in resolved.columns.iter().enumerate() {
            let Some(resolved_column) = resolved_column else {
                continue;
            };
            let column_view = &resolved_column.view;
            if let Some(slot) = self.column_nulls.get_mut(source_column) {
                *slot = column_view.nulls;
            }
            let inferred_column_type = if self.columns.is_numeric(ColumnIndex::new(source_column)) {
                ColumnTypeMetadata::Float
            } else {
                ColumnTypeMetadata::Text
            };
            if let Some(display) = self.column_display.get_mut(source_column) {
                display.column_type = column_view
                    .column_type
                    .map(column_type_metadata)
                    .unwrap_or(inferred_column_type);
                display.format = column_view
                    .format
                    .map(display_format_metadata)
                    .or_else(|| {
                        column_view
                            .mask
                            .as_ref()
                            .map(|_| DisplayFormatMetadata::Mask)
                    })
                    .unwrap_or(DisplayFormatMetadata::Plain);
                display.mask = column_view.mask.as_ref().map(|mask| NumberMaskMetadata {
                    grouped: mask.grouped,
                    decimal_places: mask.decimal_places,
                });
                display.locale = locale;
                if column_view.column_type.is_some()
                    || column_view.format.is_some()
                    || column_view.mask.is_some()
                {
                    self.column_metadata_modified.insert(source_column);
                }
            }
            let colors = column_view.colors.clone();
            let color_metadata = self.build_column_color_metadata(source_column, &colors, None);
            if let Some(slot) = self.column_color_rules.get_mut(source_column) {
                *slot = colors;
                if !slot.is_empty() {
                    self.column_metadata_modified.insert(source_column);
                }
            }
            if let Some(slot) = self.column_color_metadata.get_mut(source_column) {
                *slot = color_metadata;
            }
            if let Some(width) = column_view.width {
                if let Some(saved_width) = self.saved_column_widths.get_mut(source_column) {
                    *saved_width = matches!(
                        width,
                        crate::saved_views::ColumnWidth::Header
                            | crate::saved_views::ColumnWidth::Content
                            | crate::saved_views::ColumnWidth::Mode
                            | crate::saved_views::ColumnWidth::Max
                    )
                    .then_some(width);
                }
                if let Some(target) = self.column_widths.get_mut(source_column) {
                    *target = match width {
                        crate::saved_views::ColumnWidth::Fixed(width) => width as usize,
                        crate::saved_views::ColumnWidth::Header => {
                            header_widths.get(source_column).copied().unwrap_or(1)
                        }
                        crate::saved_views::ColumnWidth::Content => {
                            content_widths.get(source_column).copied().unwrap_or(1)
                        }
                        crate::saved_views::ColumnWidth::Mode => {
                            mode_widths.get(source_column).copied().unwrap_or(1)
                        }
                        crate::saved_views::ColumnWidth::Max => {
                            max_widths.get(source_column).copied().unwrap_or(1)
                        }
                    }
                    .max(1);
                    self.column_width_modified.insert(source_column);
                }
            }

            let alignment = column_view
                .align
                .map(|align| match align {
                    crate::saved_views::ColumnAlign::Left => ColumnAlignment::Left,
                    crate::saved_views::ColumnAlign::Right => ColumnAlignment::Right,
                })
                .or_else(|| {
                    matches!(
                        column_view.column_type,
                        Some(crate::saved_views::ColumnType::Number(_))
                    )
                    .then_some(ColumnAlignment::Right)
                });
            if let Some(alignment) = alignment {
                if let Some(slot) = self.column_alignment_overrides.get_mut(source_column) {
                    *slot = Some(alignment);
                    self.column_metadata_modified.insert(source_column);
                }
            }

            if column_view.visible == Some(false) {
                self.hidden_columns.insert(source_column);
                self.column_metadata_modified.insert(source_column);
            } else if column_view.visible == Some(true) {
                self.hidden_columns.remove(&source_column);
                self.column_metadata_modified.insert(source_column);
            }
        }
        if self.hidden_columns.len() >= self.source_column_count() {
            self.hidden_columns.clear();
        }
        self.keep_cursor_visible();
    }

    #[cfg(feature = "saved-views")]
    fn resolve_saved_column_widths(&mut self) {
        if !self.saved_column_widths.iter().any(Option::is_some) {
            return;
        }
        self.ensure_custom_column_widths();
        let header_widths = self.computed_header_widths();
        let content_widths = self.computed_content_widths();
        let mode_widths = self.computed_column_widths(ColumnWidthMode::Mode);
        let max_widths = self.computed_column_widths(ColumnWidthMode::Max);
        for (source_column, width) in self.saved_column_widths.iter().enumerate() {
            let Some(width) = width else { continue };
            let resolved = match width {
                crate::saved_views::ColumnWidth::Header => &header_widths,
                crate::saved_views::ColumnWidth::Content => &content_widths,
                crate::saved_views::ColumnWidth::Mode => &mode_widths,
                crate::saved_views::ColumnWidth::Max => &max_widths,
                crate::saved_views::ColumnWidth::Fixed(_) => continue,
            }
            .get(source_column)
            .copied()
            .unwrap_or(1)
            .max(1);
            if let Some(target) = self.column_widths.get_mut(source_column) {
                *target = resolved;
                self.column_width_modified.insert(source_column);
            }
        }
    }

    #[cfg(feature = "saved-views")]
    pub fn apply_saved_sort_keys(&mut self, sort_keys: Vec<ActiveSortKey>) {
        self.sort_keys = sort_keys
            .into_iter()
            .filter(|key| key.column < self.source_column_count())
            .take(MAX_ACTIVE_SORT_KEYS)
            .collect();
        self.computed_column_widths_cache.clear();
        self.apply_query_configuration();
    }

    #[cfg(feature = "saved-views")]
    pub fn retain_pending_saved_operations(
        &mut self,
        all_sorts: Vec<crate::saved_views::SortKey>,
        unresolved_filters: Vec<crate::saved_views::SavedFilter>,
    ) {
        self.pending_saved_sorts = all_sorts;
        self.pending_saved_filters = unresolved_filters;
    }

    #[cfg(feature = "saved-views")]
    fn apply_pending_saved_operations(&mut self, completed: bool) {
        use crate::saved_views::{FilterAction, FilterKind as SavedFilterKind, SortKind};

        let Some(definition) = self.table_definition.as_ref() else {
            return;
        };
        let mut unresolved_sort = false;
        let sort_keys = self
            .pending_saved_sorts
            .iter()
            .filter_map(|sort| {
                let Some(column) = crate::saved_views::resolve_structured_column_reference(
                    definition,
                    &sort.column,
                ) else {
                    unresolved_sort = true;
                    return None;
                };
                Some(ActiveSortKey {
                    column,
                    mode: match sort.kind {
                        SortKind::Lexical => SortMode::Lexical,
                        SortKind::Natural => SortMode::Natural,
                        SortKind::Numeric => SortMode::Numeric,
                        SortKind::Type => self.type_sort_mode_for_source(column),
                    },
                    direction: match sort.direction {
                        crate::saved_views::SortDirection::Asc => SortDirection::Ascending,
                        crate::saved_views::SortDirection::Desc => SortDirection::Descending,
                    },
                    nulls: self.resolved_null_placement(column),
                })
            })
            .collect::<Vec<_>>();
        if !self.pending_saved_sorts.is_empty() {
            self.apply_saved_sort_keys(sort_keys);
        }

        let pending_filters = std::mem::take(&mut self.pending_saved_filters);
        let mut still_pending = Vec::new();
        for filter in pending_filters {
            let Some(column) = self.table_definition.as_ref().and_then(|definition| {
                crate::saved_views::resolve_structured_column_reference(definition, &filter.column)
            }) else {
                still_pending.push(filter);
                continue;
            };
            let mode = match filter.action {
                FilterAction::In => FilterMode::In,
                FilterAction::Out => FilterMode::Out,
            };
            let kind = match filter.kind {
                SavedFilterKind::Text => FilterKind::Text,
                SavedFilterKind::Regex => FilterKind::Regex,
                SavedFilterKind::Numeric => FilterKind::Numeric,
            };
            if self
                .apply_source_filter(column, mode, kind, filter.condition.clone())
                .is_err()
            {
                still_pending.push(filter);
            }
        }
        self.pending_saved_filters = still_pending;

        if completed {
            let mut missing = self
                .pending_saved_filters
                .iter()
                .map(|filter| filter.column.clone())
                .collect::<Vec<_>>();
            if unresolved_sort {
                missing.extend(
                    self.pending_saved_sorts
                        .iter()
                        .filter(|sort| {
                            self.table_definition.as_ref().is_none_or(|definition| {
                                crate::saved_views::resolve_structured_column_reference(
                                    definition,
                                    &sort.column,
                                )
                                .is_none()
                            })
                        })
                        .map(|sort| sort.column.clone()),
                );
            }
            if !missing.is_empty() {
                missing.sort();
                missing.dedup();
                self.source_status = Some(format!(
                    "Saved view operations reference missing columns: {}",
                    missing.join(", ")
                ));
            }
            self.pending_saved_sorts.clear();
            self.pending_saved_filters.clear();
        } else if !unresolved_sort {
            self.pending_saved_sorts.clear();
        }
    }

    #[cfg(feature = "saved-views")]
    pub(crate) fn apply_source_filter(
        &mut self,
        source_column: usize,
        mode: FilterMode,
        kind: FilterKind,
        input: String,
    ) -> Result<(), FilterParseError> {
        if source_column >= self.source_column_count() {
            return Ok(());
        }
        let typed_numeric = self
            .table_definition
            .as_ref()
            .and_then(|definition| definition.columns.get(source_column))
            .is_some_and(|column| {
                matches!(
                    column.source_type,
                    crate::table::LogicalType::Integer | crate::table::LogicalType::Float
                )
            });
        if kind == FilterKind::Numeric
            && !self.preview_preparing
            && !typed_numeric
            && !self.columns.is_numeric(ColumnIndex::new(source_column))
        {
            return Err(FilterParseError::NumericUnavailable);
        }
        let condition = FilterCondition::parse(
            kind,
            &input,
            self.source_numeric_column_profile(source_column),
        )?;
        self.filters.push(ActiveFilter::new(
            source_column,
            mode,
            kind,
            input,
            condition,
        ));
        self.computed_column_widths_cache.clear();
        self.apply_query_configuration();
        Ok(())
    }

    #[cfg(feature = "saved-views")]
    pub fn to_saved_view_yaml(
        &self,
        name: &str,
        input_filename: &str,
        locale: Option<&str>,
    ) -> String {
        self.to_saved_view_yaml_inner(name, input_filename, locale, None)
    }

    #[cfg(feature = "saved-views")]
    pub fn to_saved_view_yaml_with_source_options(
        &self,
        name: &str,
        input_filename: &str,
        locale: Option<&str>,
        source_options: &crate::ingest::OpenOptions,
    ) -> String {
        self.to_saved_view_yaml_inner(name, input_filename, locale, Some(source_options))
    }

    #[cfg(feature = "saved-views")]
    fn to_saved_view_yaml_inner(
        &self,
        name: &str,
        input_filename: &str,
        locale: Option<&str>,
        source_options: Option<&crate::ingest::OpenOptions>,
    ) -> String {
        let mut yaml = String::new();
        yaml.push_str(&format!("name: {}\n", yaml_scalar(name)));
        yaml.push_str("filenames:\n");
        yaml.push_str(&format!("  - {}\n", yaml_scalar(input_filename)));

        let mut source_yaml = String::new();
        if let Some(options) = source_options {
            if let Some(provenance) = &self.source_query_provenance {
                source_yaml.push_str(&format!(
                    "format: {}\n",
                    match provenance.language {
                        crate::table::NativeQueryLanguage::Sql => "sqlite",
                        crate::table::NativeQueryLanguage::Esql => "elasticsearch",
                    }
                ));
            } else if options.format != crate::ingest::InputFormat::Auto {
                source_yaml.push_str(&format!("format: {}\n", options.format));
            }
            if let Some(json_path) = &options.json_path {
                source_yaml.push_str(&format!("json_path: {}\n", yaml_scalar(json_path.as_str())));
            }
            if options.schema_scan == crate::ingest::SchemaScan::Full {
                source_yaml.push_str("schema_scan: full\n");
            }
            if let Some(query) = &options.native_query {
                source_yaml.push_str(&format!("query: {}\n", yaml_scalar(query)));
            } else if let Some(table) = options.table.as_ref().or_else(|| {
                self.active_source_query
                    .as_ref()
                    .filter(|query| query.native_query.is_none())
                    .and(self.table_definition.as_ref())
                    .map(|definition| &definition.relation.name)
            }) {
                source_yaml.push_str(&format!("table: {}\n", yaml_scalar(table)));
            }
        }
        if let Some(object_mode) = self.object_mode {
            source_yaml.push_str(&format!("object_mode: {}\n", object_mode.resolved));
        }
        if let Some(query) = &self.active_source_query {
            if query.limit != std::num::NonZeroUsize::MAX {
                source_yaml.push_str(&format!("limit: {}\n", query.limit));
            }
            if !query.filters.is_empty() {
                source_yaml.push_str("filters:\n");
                for filter in &query.filters {
                    let column = match filter.scope {
                        crate::table::SourceFilterScope::WholeRecord => "*".to_owned(),
                        crate::table::SourceFilterScope::Column(column) => self
                            .source_column_name_for_id(column)
                            .unwrap_or_else(|| format!("column_{}", column.ordinal + 1)),
                    };
                    source_yaml.push_str(&format!("  - column: {}\n", yaml_scalar(&column)));
                    source_yaml.push_str(&format!(
                        "    operator: {}\n",
                        source_filter_operator_name(filter.operator)
                    ));
                    if let Some(value) = &filter.operand {
                        source_yaml
                            .push_str(&format!("    value: {}\n", source_operand_yaml(value)));
                    }
                }
            }
            if !query.order_by.is_empty() {
                source_yaml.push_str("sort:\n");
                for sort in &query.order_by {
                    let column = self
                        .source_column_name_for_id(sort.column)
                        .unwrap_or_else(|| format!("column_{}", sort.column.ordinal + 1));
                    source_yaml.push_str(&format!("  - column: {}\n", yaml_scalar(&column)));
                    source_yaml.push_str(&format!(
                        "    direction: {}\n",
                        match sort.direction {
                            crate::table::SortDirection::Ascending => "asc",
                            crate::table::SortDirection::Descending => "desc",
                        }
                    ));
                }
            }
        }
        if source_yaml.is_empty() {
            yaml.push_str("source: {}\n");
        } else {
            yaml.push_str("source:\n");
            push_indented_yaml(&mut yaml, &source_yaml, 2);
        }

        let mut view_yaml = String::new();
        if let Some(locale) = locale {
            view_yaml.push_str(&format!("locale: {}\n", yaml_scalar(locale)));
        }
        if let Some(nulls) = self.view_nulls {
            view_yaml.push_str(&format!(
                "nulls: {}\n",
                match nulls {
                    NullPlacement::First => "first",
                    NullPlacement::Last => "last",
                }
            ));
        }

        let mut column_blocks = Vec::new();
        for source_column in 0..self.source_column_count() {
            let include_column = self.column_width_modified.contains(&source_column)
                || self.hidden_columns.contains(&source_column)
                || self
                    .column_alignment_overrides
                    .get(source_column)
                    .is_some_and(Option::is_some)
                || self.column_metadata_modified.contains(&source_column)
                || self
                    .column_nulls
                    .get(source_column)
                    .is_some_and(Option::is_some)
                || self
                    .column_color_rules
                    .get(source_column)
                    .is_some_and(|rules| !rules.is_empty());
            if !include_column {
                continue;
            }
            let key = self.source_column_name(source_column);
            let mut block = format!("  {}:\n", yaml_key(&key));
            if let Some(label) = self
                .column_label_overrides
                .get(source_column)
                .and_then(Option::as_ref)
            {
                block.push_str(&format!("    label: {}\n", yaml_scalar(label)));
            }
            if let Some(nulls) = self.column_nulls.get(source_column).copied().flatten() {
                block.push_str(&format!(
                    "    nulls: {}\n",
                    match nulls {
                        NullPlacement::First => "first",
                        NullPlacement::Last => "last",
                    }
                ));
            }
            if self.column_metadata_modified.contains(&source_column) {
                let metadata = self
                    .column_display
                    .get(source_column)
                    .copied()
                    .unwrap_or_default();
                if metadata.column_type != ColumnTypeMetadata::Text {
                    block.push_str(&format!(
                        "    type: {}\n",
                        column_type_metadata_name(metadata.column_type)
                    ));
                }
                if metadata.format != DisplayFormatMetadata::Plain {
                    block.push_str(&format!(
                        "    format: {}\n",
                        display_format_metadata_name(metadata.format)
                    ));
                }
                if let Some(mask) = metadata.mask {
                    block.push_str(&format!("    mask: {}\n", yaml_scalar(&mask.to_mask())));
                }
            }
            if self.column_width_modified.contains(&source_column) {
                let width = self
                    .column_widths
                    .get(source_column)
                    .copied()
                    .unwrap_or(1)
                    .max(1);
                block.push_str(&format!("    width: {width}\n"));
            }
            if self.hidden_columns.contains(&source_column) {
                block.push_str("    visible: false\n");
            }
            if let Some(alignment) = self
                .column_alignment_overrides
                .get(source_column)
                .copied()
                .flatten()
            {
                let align = match alignment {
                    ColumnAlignment::Left => "left",
                    ColumnAlignment::Right => "right",
                };
                block.push_str(&format!("    align: {align}\n"));
            }
            if let Some(rules) = self.column_color_rules.get(source_column) {
                if !rules.is_empty() {
                    block.push_str("    colors:\n");
                    for rule in rules {
                        push_color_rule_yaml(&mut block, rule);
                    }
                }
            }
            column_blocks.push(block);
        }
        if !column_blocks.is_empty() {
            view_yaml.push_str("columns:\n");
            for block in column_blocks {
                view_yaml.push_str(&block);
            }
        }

        if !self.sort_keys.is_empty() {
            view_yaml.push_str("sort:\n");
            for key in &self.sort_keys {
                view_yaml.push_str(&format!(
                    "  - column: {}\n",
                    yaml_scalar(&self.source_column_name(key.column))
                ));
                view_yaml.push_str(&format!(
                    "    direction: {}\n",
                    match key.direction {
                        SortDirection::Ascending => "asc",
                        SortDirection::Descending => "desc",
                    }
                ));
                view_yaml.push_str(&format!("    kind: {}\n", sort_mode_name(key.mode)));
            }
        }

        if !self.filters.is_empty() {
            view_yaml.push_str("filters:\n");
            for filter in &self.filters {
                view_yaml.push_str(&format!(
                    "  - column: {}\n",
                    yaml_scalar(&self.source_column_name(filter.column))
                ));
                view_yaml.push_str(&format!(
                    "    action: {}\n",
                    match filter.mode {
                        FilterMode::In => "in",
                        FilterMode::Out => "out",
                    }
                ));
                view_yaml.push_str(&format!("    kind: {}\n", filter_kind_name(filter.kind)));
                view_yaml.push_str(&format!("    condition: {}\n", yaml_scalar(&filter.input)));
            }
        }
        if view_yaml.is_empty() {
            yaml.push_str("view: {}\n");
        } else {
            yaml.push_str("view:\n");
            push_indented_yaml(&mut yaml, &view_yaml, 2);
        }
        yaml
    }

    #[cfg(feature = "saved-views")]
    fn source_column_name(&self, source_column: usize) -> String {
        if let Some(key) = self
            .table_definition
            .as_ref()
            .and_then(|definition| definition.canonical_column_key(source_column))
        {
            return key;
        }
        self.header
            .as_ref()
            .and_then(|header| header.get(source_column))
            .cloned()
            .unwrap_or_else(|| format!("column_{}", source_column + 1))
    }

    fn keep_cursor_visible(&mut self) {
        self.cursor.row = self
            .cursor
            .row
            .min(self.visible_rows.len().saturating_sub(1));
        self.cursor.column = self
            .cursor
            .column
            .min(self.column_count().saturating_sub(1));
        if self.cursor.row < self.viewport.origin.row {
            self.viewport.origin.row = self.cursor.row;
        } else if self.cursor.row >= self.viewport.origin.row + self.viewport.height {
            self.viewport.origin.row = self.cursor.row + 1 - self.viewport.height;
        }

        if self.cursor.column < self.viewport.origin.column {
            self.viewport.origin.column = self.cursor.column;
        } else if self.cursor.column >= self.viewport.origin.column + self.viewport.width {
            self.viewport.origin.column = self.cursor.column + 1 - self.viewport.width;
        }
    }

    fn ensure_custom_column_widths(&mut self) {
        if self.column_widths.len() != self.source_column_count() {
            self.column_widths = self.effective_column_widths();
        }
        self.computed_column_widths_cache.clear();
    }

    fn automatic_column_width_cap(&self) -> usize {
        if self.terminal_width == 0 {
            usize::MAX
        } else {
            (self.terminal_width.saturating_mul(4) / 5).max(1)
        }
    }

    fn cap_automatic_widths(&self, widths: &mut [usize]) {
        let cap = self.automatic_column_width_cap();
        for width in widths {
            *width = (*width).min(cap);
        }
    }

    fn computed_column_widths(&self, mode: ColumnWidthMode) -> Vec<usize> {
        if let ColumnWidthMode::Fixed(width) = mode {
            return vec![width as usize; self.source_column_count()];
        }

        let mut rows = Vec::new();
        if let Some(header) = self.rendered_source_header() {
            rows.push(header);
        }
        rows.extend(self.rendered_source_rows());
        column_widths(&rows, mode, self.column_gap)
    }

    #[cfg(feature = "saved-views")]
    fn computed_header_widths(&self) -> Vec<usize> {
        if let Some(header) = self.rendered_source_header() {
            header
                .iter()
                .map(|cell| UnicodeWidthStr::width(cell.as_str()).max(1))
                .collect()
        } else {
            vec![1; self.source_column_count()]
        }
    }

    #[cfg(feature = "saved-views")]
    fn computed_content_widths(&self) -> Vec<usize> {
        let mut rows = Vec::new();
        if let Some(header) = self.rendered_source_header() {
            rows.push(header);
        }
        rows.extend(self.rendered_source_rows());
        column_widths(&rows, ColumnWidthMode::Max, self.column_gap)
    }

    fn rendered_source_rows(&self) -> Vec<Vec<String>> {
        self.rows
            .iter()
            .map(|row| {
                (0..self.source_column_count())
                    .map(|source_column| {
                        self.render_source_cell(
                            source_column,
                            row.get(source_column).map(String::as_str),
                        )
                    })
                    .collect()
            })
            .collect()
    }

    fn cached_rendered_value_width(&self, source_column: usize) -> usize {
        self.rows
            .iter()
            .map(|row| {
                self.render_source_cell(source_column, row.get(source_column).map(String::as_str))
            })
            .map(|value| UnicodeWidthStr::width(value.as_str()))
            .max()
            .unwrap_or(1)
            .max(1)
    }

    fn rendered_source_header_width(&self, source_column: usize) -> usize {
        self.rendered_source_header()
            .and_then(|header| {
                header
                    .get(source_column)
                    .map(|value| UnicodeWidthStr::width(value.as_str()))
            })
            .unwrap_or(1)
            .max(1)
    }

    fn row_passes_filters(&self, row: &[String]) -> bool {
        self.filters.iter().all(|filter| {
            let raw = row
                .get(filter.column)
                .map(String::as_str)
                .unwrap_or_default();
            let rendered = self.render_source_cell(filter.column, Some(raw));
            filter.accepts_values(
                raw,
                &rendered,
                self.source_numeric_column_profile(filter.column),
            )
        })
    }

    fn source_column_visible(&self, source_column: usize) -> bool {
        !self.hidden_columns.contains(&source_column)
    }

    fn visible_source_columns(&self) -> Vec<usize> {
        self.visible_source_columns_iter().collect()
    }

    pub(crate) fn visible_source_columns_vec(&self) -> Vec<usize> {
        self.visible_source_columns()
    }

    fn visible_source_columns_iter(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.source_column_count())
            .filter(|source_column| self.source_column_visible(*source_column))
    }

    fn source_column_for_visible(&self, column: usize) -> Option<usize> {
        self.visible_source_columns_iter().nth(column)
    }

    fn visible_column_for_source(&self, source_column: usize) -> Option<usize> {
        self.visible_source_columns_iter()
            .position(|candidate| candidate == source_column)
    }

    fn render_source_cell(&self, source_column: usize, raw: Option<&str>) -> String {
        let raw = raw.unwrap_or_default();
        let metadata = self
            .column_display
            .get(source_column)
            .copied()
            .unwrap_or_default();
        match metadata.format {
            DisplayFormatMetadata::Plain => raw.to_owned(),
            DisplayFormatMetadata::Uppercase => raw.to_uppercase(),
            DisplayFormatMetadata::Lowercase => raw.to_lowercase(),
            DisplayFormatMetadata::Locale => self
                .format_locale_number(raw, source_column, metadata.locale)
                .unwrap_or_else(|| raw.to_owned()),
            DisplayFormatMetadata::Mask => metadata
                .mask
                .and_then(|mask| self.format_masked_number(raw, source_column, mask))
                .unwrap_or_else(|| raw.to_owned()),
            DisplayFormatMetadata::BooleanChar => parse_bool_key(raw)
                .map(|value| if value { "y" } else { "n" }.to_owned())
                .unwrap_or_else(|| raw.to_owned()),
            DisplayFormatMetadata::BooleanBit => parse_bool_key(raw)
                .map(|value| if value { "1" } else { "0" }.to_owned())
                .unwrap_or_else(|| raw.to_owned()),
            DisplayFormatMetadata::BooleanWord => parse_bool_key(raw)
                .map(|value| if value { "true" } else { "false" }.to_owned())
                .unwrap_or_else(|| raw.to_owned()),
        }
    }

    fn format_locale_number(
        &self,
        raw: &str,
        source_column: usize,
        locale: LocaleMetadata,
    ) -> Option<String> {
        let value = parse_numeric_scalar(raw, self.source_numeric_column_profile(source_column))?;
        let decimal_places = raw
            .split_once('.')
            .map(|(_, fraction)| {
                fraction
                    .chars()
                    .take_while(|ch| ch.is_ascii_digit())
                    .count()
            })
            .unwrap_or(0);
        Some(format_number_parts(value, decimal_places, true, locale))
    }

    fn format_masked_number(
        &self,
        raw: &str,
        source_column: usize,
        mask: NumberMaskMetadata,
    ) -> Option<String> {
        let value = parse_numeric_scalar(raw, self.source_numeric_column_profile(source_column))?;
        Some(format_number_parts(
            value,
            mask.decimal_places,
            mask.grouped,
            LocaleMetadata::en_us(),
        ))
    }

    fn rebuild_column_color_metadata(&mut self) {
        if self.preview_preparing {
            return;
        }
        let exact_profiles = self.exact_reduction_profiles().ok().flatten();
        self.column_color_metadata = (0..self.source_column_count())
            .map(|source_column| {
                let rules = self
                    .column_color_rules
                    .get(source_column)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                self.build_column_color_metadata(
                    source_column,
                    rules,
                    exact_profiles
                        .as_ref()
                        .and_then(|profiles| profiles.get(source_column)),
                )
            })
            .collect();
    }

    fn rebuild_column_color_metadata_for(&mut self, source_column: usize) {
        if self.preview_preparing {
            return;
        }
        let exact_profile = self
            .exact_reduction_profiles()
            .ok()
            .flatten()
            .and_then(|profiles| profiles.get(source_column).cloned());
        let metadata = {
            let rules = self
                .column_color_rules
                .get(source_column)
                .map(Vec::as_slice)
                .unwrap_or_default();
            self.build_column_color_metadata(source_column, rules, exact_profile.as_ref())
        };
        if let Some(slot) = self.column_color_metadata.get_mut(source_column) {
            *slot = metadata;
        }
    }

    fn build_column_color_metadata(
        &self,
        source_column: usize,
        rules: &[ConditionalColorRule],
        exact_profile: Option<&crate::table::ColumnReductionProfile>,
    ) -> ColumnColorMetadata {
        let numeric_min_max = rules
            .iter()
            .any(|rule| matches!(rule, ConditionalColorRule::AutoGradient { .. }))
            .then(|| {
                exact_profile
                    .and_then(|profile| profile.numeric_min_max)
                    .or_else(|| self.numeric_min_max(source_column))
            })
            .flatten();
        let identifier_indexes = if rules
            .iter()
            .any(|rule| matches!(rule, ConditionalColorRule::Identifiers { .. }))
        {
            exact_profile
                .map(|profile| {
                    profile
                        .identifiers
                        .iter()
                        .cloned()
                        .enumerate()
                        .map(|(index, value)| (value, index))
                        .collect()
                })
                .unwrap_or_else(|| self.identifier_indexes(source_column))
        } else {
            BTreeMap::new()
        };
        let identifier_color_refs = rules
            .iter()
            .enumerate()
            .filter_map(|(rule_idx, rule)| {
                let ConditionalColorRule::Identifiers { colors } = rule else {
                    return None;
                };
                let color_refs = identifier_indexes
                    .iter()
                    .map(|(value, index)| (value.clone(), identifier_color_ref(*index, colors)))
                    .collect::<BTreeMap<_, _>>();
                Some((rule_idx, color_refs))
            })
            .collect();
        let gradient_color_refs = rules
            .iter()
            .enumerate()
            .filter_map(|(rule_idx, rule)| {
                let ConditionalColorRule::AutoGradient { colors, steps } = rule else {
                    return None;
                };
                let steps = (*steps).max(1);
                let color_refs = (0..steps)
                    .map(|bucket| gradient_color_ref(colors, bucket, steps))
                    .collect::<Vec<_>>();
                Some((rule_idx, color_refs))
            })
            .collect();
        ColumnColorMetadata {
            numeric_min_max,
            identifier_color_refs,
            gradient_color_refs,
        }
    }

    fn exact_reduction_profiles(
        &mut self,
    ) -> anyhow::Result<Option<Vec<crate::table::ColumnReductionProfile>>> {
        if self.preview_preparing {
            return Ok(None);
        }
        let shared = if self.view_transform_is_active() {
            self.query_store.clone()
        } else {
            self.incremental_store.clone()
        };
        let Some(shared) = shared else {
            return Ok(None);
        };

        let progress = shared
            .0
            .borrow_mut()
            .ensure_indexed_through(RowIndex(usize::MAX))?;
        self.apply_source_schema_delta(progress.schema_delta)?;
        let header = self.rendered_source_header();
        let result = crate::table::reduce_column_profiles(
            &mut **shared.0.borrow_mut(),
            crate::table::ReductionScope::Exact,
            header.as_deref(),
        )?;
        self.source_status = Some(format!(
            "Profiled {} rows{}",
            result.rows_scanned,
            if result.complete { "" } else { " (partial)" }
        ));
        Ok(Some(result.value))
    }

    fn numeric_min_max(&self, source_column: usize) -> Option<(f64, f64)> {
        let profile = self.source_numeric_column_profile(source_column);
        let mut values = self
            .rows
            .iter()
            .filter_map(|row| row.get(source_column))
            .filter_map(|value| parse_numeric_scalar(value, profile))
            .filter(|value| value.is_finite());
        let first = values.next()?;
        Some(values.fold((first, first), |(min, max), value| {
            (min.min(value), max.max(value))
        }))
    }

    fn identifier_indexes(&self, source_column: usize) -> BTreeMap<String, usize> {
        let values = self
            .rows
            .iter()
            .filter_map(|row| row.get(source_column).map(String::as_str))
            .map(|raw| self.render_source_cell(source_column, Some(raw)))
            .filter(|value| !value.is_empty())
            .collect::<BTreeSet<_>>();
        values
            .into_iter()
            .enumerate()
            .map(|(index, value)| (value, index))
            .collect()
    }
}

fn format_number_parts(
    value: f64,
    decimal_places: usize,
    grouped: bool,
    locale: LocaleMetadata,
) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    let negative = value.is_sign_negative();
    let value = value.abs();
    let fixed = format!("{value:.decimal_places$}");
    let (integer, fraction) = fixed.split_once('.').unwrap_or((fixed.as_str(), ""));
    let mut rendered = String::new();
    if negative {
        rendered.push('-');
    }
    if grouped {
        rendered.push_str(&group_integer(integer, locale.grouping_separator));
    } else {
        rendered.push_str(integer);
    }
    if decimal_places > 0 {
        rendered.push(locale.decimal_separator);
        rendered.push_str(fraction);
    }
    rendered
}

fn group_integer(value: &str, separator: char) -> String {
    let mut grouped = String::new();
    for (idx, ch) in value.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            grouped.push(separator);
        }
        grouped.push(ch);
    }
    grouped.chars().rev().collect()
}

fn column_type_choice(metadata: ColumnTypeMetadata) -> ColumnTypeChoice {
    match metadata {
        ColumnTypeMetadata::Text => ColumnTypeChoice::Text,
        ColumnTypeMetadata::Date => ColumnTypeChoice::Date,
        ColumnTypeMetadata::Ip => ColumnTypeChoice::Ip,
        ColumnTypeMetadata::Float => ColumnTypeChoice::Float,
        ColumnTypeMetadata::Int => ColumnTypeChoice::Integer,
        ColumnTypeMetadata::SemVer => ColumnTypeChoice::SemVer,
        ColumnTypeMetadata::BooleanWord
        | ColumnTypeMetadata::BooleanChar
        | ColumnTypeMetadata::BooleanBit => ColumnTypeChoice::Boolean,
    }
}

fn column_type_metadata_from_choice(choice: ColumnTypeChoice) -> ColumnTypeMetadata {
    match choice {
        ColumnTypeChoice::Text => ColumnTypeMetadata::Text,
        ColumnTypeChoice::Date => ColumnTypeMetadata::Date,
        ColumnTypeChoice::Ip => ColumnTypeMetadata::Ip,
        ColumnTypeChoice::Float => ColumnTypeMetadata::Float,
        ColumnTypeChoice::Integer => ColumnTypeMetadata::Int,
        ColumnTypeChoice::SemVer => ColumnTypeMetadata::SemVer,
        ColumnTypeChoice::Boolean => ColumnTypeMetadata::BooleanWord,
    }
}

fn column_format_choice(metadata: DisplayFormatMetadata) -> ColumnFormatChoice {
    match metadata {
        DisplayFormatMetadata::Plain | DisplayFormatMetadata::Mask => ColumnFormatChoice::Plain,
        DisplayFormatMetadata::Locale => ColumnFormatChoice::Locale,
        DisplayFormatMetadata::Uppercase => ColumnFormatChoice::Uppercase,
        DisplayFormatMetadata::Lowercase => ColumnFormatChoice::Lowercase,
        DisplayFormatMetadata::BooleanChar => ColumnFormatChoice::Char,
        DisplayFormatMetadata::BooleanBit => ColumnFormatChoice::Bit,
        DisplayFormatMetadata::BooleanWord => ColumnFormatChoice::Word,
    }
}

fn display_format_metadata_from_choice(choice: ColumnFormatChoice) -> DisplayFormatMetadata {
    match choice {
        ColumnFormatChoice::Plain => DisplayFormatMetadata::Plain,
        ColumnFormatChoice::Locale => DisplayFormatMetadata::Locale,
        ColumnFormatChoice::Uppercase => DisplayFormatMetadata::Uppercase,
        ColumnFormatChoice::Lowercase => DisplayFormatMetadata::Lowercase,
        ColumnFormatChoice::Char => DisplayFormatMetadata::BooleanChar,
        ColumnFormatChoice::Bit => DisplayFormatMetadata::BooleanBit,
        ColumnFormatChoice::Word => DisplayFormatMetadata::BooleanWord,
    }
}

#[cfg(feature = "saved-views")]
fn system_locale() -> Option<String> {
    ["LC_ALL", "LC_NUMERIC", "LANG"]
        .into_iter()
        .find_map(|key| {
            let value = std::env::var(key).ok()?;
            (!value.is_empty() && value != "C" && value != "POSIX").then_some(value)
        })
}

#[cfg(feature = "saved-views")]
fn column_type_metadata(column_type: crate::saved_views::ColumnType) -> ColumnTypeMetadata {
    match column_type {
        crate::saved_views::ColumnType::String(kind) => match kind {
            crate::saved_views::StringKind::Text => ColumnTypeMetadata::Text,
            crate::saved_views::StringKind::Date => ColumnTypeMetadata::Date,
            crate::saved_views::StringKind::Ip => ColumnTypeMetadata::Ip,
        },
        crate::saved_views::ColumnType::Number(kind) => match kind {
            crate::saved_views::NumberKind::Float => ColumnTypeMetadata::Float,
            crate::saved_views::NumberKind::Int => ColumnTypeMetadata::Int,
            crate::saved_views::NumberKind::SemVer => ColumnTypeMetadata::SemVer,
        },
        crate::saved_views::ColumnType::Boolean(kind) => match kind {
            crate::saved_views::BooleanKind::Char => ColumnTypeMetadata::BooleanChar,
            crate::saved_views::BooleanKind::Bit => ColumnTypeMetadata::BooleanBit,
            crate::saved_views::BooleanKind::Word => ColumnTypeMetadata::BooleanWord,
        },
    }
}

#[cfg(feature = "saved-views")]
fn display_format_metadata(format: crate::saved_views::DisplayFormat) -> DisplayFormatMetadata {
    match format {
        crate::saved_views::DisplayFormat::Plain => DisplayFormatMetadata::Plain,
        crate::saved_views::DisplayFormat::Locale => DisplayFormatMetadata::Locale,
        crate::saved_views::DisplayFormat::Mask => DisplayFormatMetadata::Mask,
        crate::saved_views::DisplayFormat::Uppercase => DisplayFormatMetadata::Uppercase,
        crate::saved_views::DisplayFormat::Lowercase => DisplayFormatMetadata::Lowercase,
        crate::saved_views::DisplayFormat::Char => DisplayFormatMetadata::BooleanChar,
        crate::saved_views::DisplayFormat::Bit => DisplayFormatMetadata::BooleanBit,
        crate::saved_views::DisplayFormat::Word => DisplayFormatMetadata::BooleanWord,
    }
}

#[cfg(feature = "saved-views")]
fn column_type_metadata_name(column_type: ColumnTypeMetadata) -> &'static str {
    match column_type {
        ColumnTypeMetadata::Text => "text",
        ColumnTypeMetadata::Date => "date",
        ColumnTypeMetadata::Ip => "ip",
        ColumnTypeMetadata::Float => "float",
        ColumnTypeMetadata::Int => "integer",
        ColumnTypeMetadata::SemVer => "semver",
        ColumnTypeMetadata::BooleanWord => "word",
        ColumnTypeMetadata::BooleanChar => "char",
        ColumnTypeMetadata::BooleanBit => "bit",
    }
}

#[cfg(feature = "saved-views")]
fn display_format_metadata_name(format: DisplayFormatMetadata) -> &'static str {
    match format {
        DisplayFormatMetadata::Plain => "plain",
        DisplayFormatMetadata::Locale => "locale",
        DisplayFormatMetadata::Mask => "mask",
        DisplayFormatMetadata::Uppercase => "uppercase",
        DisplayFormatMetadata::Lowercase => "lowercase",
        DisplayFormatMetadata::BooleanChar => "char",
        DisplayFormatMetadata::BooleanBit => "bit",
        DisplayFormatMetadata::BooleanWord => "word",
    }
}

fn table_sort_mode(mode: SortMode) -> crate::table::ViewSortMode {
    match mode {
        SortMode::Lexical => crate::table::ViewSortMode::Lexical,
        SortMode::Natural => crate::table::ViewSortMode::Natural,
        SortMode::Numeric => crate::table::ViewSortMode::Numeric,
        #[cfg(feature = "saved-views")]
        SortMode::Date => crate::table::ViewSortMode::Date,
        #[cfg(feature = "saved-views")]
        SortMode::SemVer => crate::table::ViewSortMode::SemanticVersion,
        #[cfg(feature = "saved-views")]
        SortMode::Ip => crate::table::ViewSortMode::Ip,
        #[cfg(feature = "saved-views")]
        SortMode::Boolean => crate::table::ViewSortMode::Boolean,
    }
}

#[cfg(feature = "saved-views")]
fn sort_mode_name(mode: SortMode) -> &'static str {
    match mode {
        SortMode::Lexical => "lexical",
        SortMode::Natural => "natural",
        SortMode::Numeric => "numeric",
        SortMode::Date | SortMode::SemVer | SortMode::Ip | SortMode::Boolean => "type",
    }
}

#[cfg(feature = "saved-views")]
fn filter_kind_name(kind: FilterKind) -> &'static str {
    match kind {
        FilterKind::Text => "text",
        FilterKind::Regex => "regex",
        FilterKind::Numeric => "numeric",
    }
}

#[cfg(feature = "saved-views")]
fn push_color_rule_yaml(block: &mut String, rule: &ConditionalColorRule) {
    match rule {
        ConditionalColorRule::Match { entries } => {
            block.push_str("      - match:\n");
            for entry in entries {
                block.push_str(&format!(
                    "          {}: {}\n",
                    conditional_value_yaml(&entry.value),
                    yaml_scalar(&entry.color)
                ));
            }
        }
        ConditionalColorRule::Range { entries } => {
            block.push_str("      - range:\n");
            for entry in entries {
                block.push_str(&format!(
                    "          {}: {}\n",
                    yaml_scalar(&range_entry_expression(entry)),
                    yaml_scalar(&entry.color)
                ));
            }
        }
        ConditionalColorRule::FixedGradient { stops } => {
            block.push_str("      - gradient:\n");
            block.push_str("          mode: fixed\n");
            block.push_str("          stops:\n");
            for stop in stops {
                block.push_str(&format!(
                    "            {}: {}\n",
                    yaml_scalar(&stop.value.to_string()),
                    yaml_scalar(&stop.color)
                ));
            }
        }
        ConditionalColorRule::AutoGradient { colors, steps } => {
            block.push_str("      - gradient:\n");
            block.push_str("          mode: auto\n");
            block.push_str(&format!("          steps: {steps}\n"));
            block.push_str("          colors:\n");
            for color in colors {
                block.push_str(&format!("            - {}\n", yaml_scalar(color)));
            }
        }
        ConditionalColorRule::Identifiers { colors } => {
            block.push_str("      - identifiers:\n");
            match colors {
                crate::theme::IdentifierColors::Auto => {
                    block.push_str("          colors: auto\n");
                }
                crate::theme::IdentifierColors::Colors(colors) => {
                    block.push_str("          colors:\n");
                    for color in colors {
                        block.push_str(&format!("            - {}\n", yaml_scalar(color)));
                    }
                }
            }
        }
    }
}

#[cfg(feature = "saved-views")]
fn range_entry_expression(entry: &crate::theme::RangeEntry) -> String {
    let mut parts = Vec::new();
    if let Some(value) = entry.gte {
        parts.push(format!(">={value}"));
    }
    if let Some(value) = entry.gt {
        parts.push(format!(">{value}"));
    }
    if let Some(value) = entry.lte {
        parts.push(format!("<={value}"));
    }
    if let Some(value) = entry.lt {
        parts.push(format!("<{value}"));
    }
    parts.join(" ")
}

#[cfg(feature = "saved-views")]
fn conditional_value_yaml(value: &ConditionalValue) -> String {
    match value {
        ConditionalValue::Bool(value) => value.to_string(),
        ConditionalValue::Number(value) => value.to_string(),
        ConditionalValue::String(value) => yaml_quoted_scalar(value),
    }
}

#[cfg(feature = "saved-views")]
fn yaml_key(value: &str) -> String {
    yaml_scalar(value)
}

#[cfg(feature = "saved-views")]
fn push_indented_yaml(output: &mut String, yaml: &str, spaces: usize) {
    let indent = " ".repeat(spaces);
    for line in yaml.lines() {
        output.push_str(&indent);
        output.push_str(line);
        output.push('\n');
    }
}

#[cfg(feature = "saved-views")]
fn yaml_scalar(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/'))
        && !matches!(value, "true" | "false" | "yes" | "no" | "null")
    {
        value.to_owned()
    } else {
        yaml_quoted_scalar(value)
    }
}

#[cfg(feature = "saved-views")]
fn source_filter_operator_name(operator: crate::table::SourceFilterOperator) -> &'static str {
    use crate::table::SourceFilterOperator as Operator;
    match operator {
        Operator::Equal => "equal",
        Operator::NotEqual => "not_equal",
        Operator::LessThan => "less_than",
        Operator::LessThanOrEqual => "less_or_equal",
        Operator::GreaterThan => "greater_than",
        Operator::GreaterThanOrEqual => "greater_or_equal",
        Operator::Contains => "contains",
        Operator::Prefix => "prefix",
        Operator::IsNull => "is_null",
        Operator::IsNotNull => "is_not_null",
    }
}

#[cfg(feature = "saved-views")]
fn source_operand_yaml(value: &crate::table::SourceOperand) -> String {
    match value {
        crate::table::SourceOperand::Null => "null".to_owned(),
        crate::table::SourceOperand::Boolean(value) => value.to_string(),
        crate::table::SourceOperand::Integer(value) => value.to_string(),
        crate::table::SourceOperand::Float(value) => value.to_string(),
        crate::table::SourceOperand::Text(value) => yaml_scalar(value),
        crate::table::SourceOperand::Binary(value) => yaml_scalar(&String::from_utf8_lossy(value)),
    }
}

#[cfg(feature = "saved-views")]
fn yaml_quoted_scalar(value: &str) -> String {
    let mut quoted = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            ch if ch.is_control() => quoted.push_str(&format!("\\x{:02X}", ch as u32)),
            ch => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

pub fn column_widths(rows: &[Vec<String>], mode: ColumnWidthMode, gap: usize) -> Vec<usize> {
    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    match mode {
        ColumnWidthMode::Fixed(width) => vec![width as usize; column_count],
        ColumnWidthMode::Max => (0..column_count)
            .map(|column| {
                rows.iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| UnicodeWidthStr::width(cell.as_str()))
                    .max()
                    .unwrap_or(1)
                    .clamp(1, 250)
            })
            .collect(),
        ColumnWidthMode::Mode => (0..column_count)
            .map(|column| mode_width(rows, column, gap))
            .collect(),
    }
}

fn mode_width(rows: &[Vec<String>], column: usize, _gap: usize) -> usize {
    rows.iter()
        .filter_map(|row| row.get(column))
        .map(|cell| UnicodeWidthStr::width(cell.as_str()))
        .max()
        .unwrap_or(1)
        .clamp(1, 250)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::SourceAdapter;
    use crate::ops::filter::{FilterKind, FilterMode};
    use crate::table::{
        CellValue, ColumnDefinition, ColumnId, ColumnSourceIdentity, IndexProgress, LogicalType,
        RelationMetadata, Row, RowId, ScanDirection, ScanProgress, ScanRequest, SchemaDelta,
        SchemaState, TypeOrigin,
    };

    fn rows(values: &[&[&str]]) -> Vec<Vec<String>> {
        values
            .iter()
            .map(|row| row.iter().map(|cell| (*cell).to_owned()).collect())
            .collect()
    }

    #[derive(Clone)]
    struct QueryTestStore {
        generation: SourceGeneration,
        rows: Vec<Row>,
        indexed: usize,
        fail_materialize: bool,
    }

    impl TableStore for QueryTestStore {
        fn generation(&self) -> SourceGeneration {
            self.generation
        }

        fn row_count(&self) -> RowCount {
            if self.indexed >= self.rows.len() {
                RowCount::Exact(self.rows.len())
            } else if self.indexed == 0 {
                RowCount::Unknown
            } else {
                RowCount::AtLeast(self.indexed)
            }
        }

        fn column_count(&self) -> usize {
            1
        }

        fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
            self.ensure_indexed_through(index)?;
            Ok(self.rows.get(index.0).cloned())
        }

        fn ensure_indexed_through(&mut self, index: RowIndex) -> anyhow::Result<IndexProgress> {
            self.indexed = self
                .indexed
                .max(index.0.saturating_add(1).min(self.rows.len()));
            Ok(IndexProgress {
                row_count: self.row_count(),
                schema_delta: SchemaDelta::default(),
                bytes_scanned: self.indexed as u64,
            })
        }

        fn scan_rows(
            &mut self,
            request: ScanRequest,
            visitor: &mut dyn crate::table::RowVisitor,
        ) -> anyhow::Result<ScanProgress> {
            if request.max_rows == 0 {
                return Ok(ScanProgress {
                    visited: 0,
                    next: Some(request.start),
                    reached_end: false,
                });
            }
            if request.direction == ScanDirection::Forward {
                self.ensure_indexed_through(RowIndex(
                    request
                        .start
                        .0
                        .saturating_add(request.max_rows.saturating_sub(1)),
                ))?;
            }
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
            if self.fail_materialize {
                anyhow::bail!("injected materialization failure");
            }
            InMemoryTable::from_rows(self.generation, self.rows.clone())
        }
    }

    fn query_test_table(values: &[&str], fail_materialize: bool) -> OpenedTable {
        let generation = SourceGeneration::new();
        let make_rows = |values: &[&str]| {
            values
                .iter()
                .enumerate()
                .map(|(ordinal, value)| {
                    Row::new(
                        RowId {
                            generation,
                            ordinal: ordinal as u64,
                        },
                        vec![CellValue::Text((*value).to_owned())],
                    )
                })
                .collect::<Vec<_>>()
        };
        let source_rows = make_rows(values);
        OpenedTable {
            generation,
            definition: TableDefinition {
                generation,
                columns: vec![ColumnDefinition {
                    id: ColumnId {
                        generation,
                        ordinal: 0,
                    },
                    source_identity: ColumnSourceIdentity::Delimited {
                        ordinal: 0,
                        name: Some("value".to_owned()),
                    },
                    display_name: "value".to_owned(),
                    source_declared_type: None,
                    source_type: LogicalType::Text,
                    type_origin: TypeOrigin::Declared,
                }],
                schema_state: SchemaState::Complete,
                relation: RelationMetadata::implicit("query-test", true),
            },
            store: Box::new(QueryTestStore {
                generation,
                rows: source_rows,
                indexed: 0,
                fail_materialize,
            }),
            object_mode: None,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn source_column_name_for_id_preserves_duplicate_relation_occurrences() {
        let generation = SourceGeneration::new();
        let mut view = TableView::classify(
            rows(&[&["first", "second"], &["1", "2"]]),
            Viewport::new(10, 4),
        );
        view.table_definition = Some(TableDefinition {
            generation,
            columns: (0..2)
                .map(|ordinal| ColumnDefinition {
                    id: ColumnId {
                        generation,
                        ordinal,
                    },
                    source_identity: ColumnSourceIdentity::RelationColumn {
                        relation: "duplicates".to_owned(),
                        name: "value".to_owned(),
                        ordinal: ordinal as usize,
                    },
                    display_name: "value".to_owned(),
                    source_declared_type: None,
                    source_type: LogicalType::Text,
                    type_origin: TypeOrigin::Declared,
                })
                .collect(),
            schema_state: SchemaState::Complete,
            relation: RelationMetadata::implicit("duplicates", true),
        });

        assert_eq!(
            view.source_column_name_for_id(ColumnId {
                generation,
                ordinal: 0
            })
            .as_deref(),
            Some("value#1")
        );
        assert_eq!(
            view.source_column_name_for_id(ColumnId {
                generation,
                ordinal: 1
            })
            .as_deref(),
            Some("value#2")
        );
    }

    #[test]
    fn classifies_non_numeric_first_row_as_header() {
        let view = TableView::classify(
            rows(&[&["Name", "Value"], &["alpha", "1"]]),
            Viewport::new(10, 4),
        );
        assert_eq!(view.header().expect("header"), ["Name", "Value"]);
        assert!(view.header_visible());
        assert_eq!(view.rows(), rows(&[&["alpha", "1"]]));
    }

    #[test]
    fn keeps_numeric_first_row_as_data() {
        let view = TableView::classify(rows(&[&["1", "2"], &["3", "4"]]), Viewport::new(10, 4));
        assert!(view.header().is_none());
        assert_eq!(view.rows(), rows(&[&["1", "2"], &["3", "4"]]));
    }

    #[test]
    fn local_view_transform_materializes_active_result_and_restores_base() {
        let opened = query_test_table(&["keep-0", "keep-1", "drop"], false);
        let mut view = TableView::from_opened_table(opened, Viewport::new(1, 1)).expect("view");

        view.apply_filter(0, FilterMode::In, FilterKind::Text, "keep".to_owned())
            .expect("filter");

        assert_eq!(view.rows(), rows(&[&["keep-0"], &["keep-1"]]));
        assert_eq!(view.row_count_state(), RowCount::Exact(2));

        view.clear_filters_for_column(0);
        assert_eq!(view.rows(), rows(&[&["keep-0"], &["keep-1"], &["drop"]]));
        assert_eq!(view.row_count_state(), RowCount::Exact(3));
        assert!(!view.view_transform_is_active());
    }

    #[test]
    fn materialization_failures_preserve_prior_view_and_configuration() {
        let expected = "injected materialization failure";
        let opened = query_test_table(&["b", "a"], true);
        let mut view = TableView::from_opened_table(opened, Viewport::new(1, 1)).expect("view");
        let prior_rows = view.rows.clone();
        let prior_cursor = view.cursor;
        let prior_viewport = view.viewport;
        let prior_query = view.active_view_transform.clone();

        view.sort_current_column(SortMode::Lexical, SortDirection::Ascending);

        assert_eq!(view.rows, prior_rows);
        assert_eq!(view.cursor, prior_cursor);
        assert_eq!(view.viewport, prior_viewport);
        assert_eq!(view.active_view_transform, prior_query);
        assert!(view.sort_keys.is_empty());
        assert!(view.filters.is_empty());
        assert!(view.take_source_status().unwrap().contains(expected));

        view.apply_filter(0, FilterMode::In, FilterKind::Text, "a".to_owned())
            .expect("filter input");
        assert_eq!(view.rows, prior_rows);
        assert_eq!(view.cursor, prior_cursor);
        assert_eq!(view.viewport, prior_viewport);
        assert_eq!(view.active_view_transform, prior_query);
        assert!(view.sort_keys.is_empty());
        assert!(view.filters.is_empty());
        assert!(view.take_source_status().unwrap().contains(expected));
    }

    #[test]
    fn toggles_header_structurally() {
        let mut view = TableView::classify(
            rows(&[&["Name", "Value"], &["Name", "Value"]]),
            Viewport::new(10, 4),
        );
        view.toggle_header();
        assert!(!view.header_visible());
        assert_eq!(view.header().expect("header"), ["Name", "Value"]);
        assert_eq!(view.rows(), rows(&[&["Name", "Value"]]));
    }

    #[test]
    fn keeps_cursor_inside_table_and_viewport() {
        let mut view = TableView::classify(
            rows(&[&["A", "B"], &["1", "2"], &["3", "4"]]),
            Viewport::new(1, 1),
        );
        view.goto(10, 10);
        assert_eq!(view.cursor(), Position { row: 1, column: 1 });
        assert_eq!(view.viewport().origin, Position { row: 1, column: 1 });
    }

    #[test]
    fn current_cell_match_uses_case_insensitive_query() {
        let view = TableView::classify(
            rows(&[&["Name", "Value"], &["alpha", "1"]]),
            Viewport::new(10, 2),
        );

        assert!(view.current_cell_matches("ALP"));
        assert!(!view.current_cell_matches(""));
    }

    #[test]
    fn computes_fixed_and_max_widths() {
        let rows = rows(&[&["a", "wide"], &["bb", "中"]]);
        assert_eq!(column_widths(&rows, ColumnWidthMode::Fixed(3), 2), [3, 3]);
        assert_eq!(column_widths(&rows, ColumnWidthMode::Max, 2), [2, 4]);
    }

    #[test]
    fn sampled_mode_width_fits_widest_observed_value() {
        let rows = rows(&[&["store"], &["0"], &["0"], &["0"], &["4279369981"]]);

        assert_eq!(column_widths(&rows, ColumnWidthMode::Mode, 2), [10]);
    }

    #[test]
    fn opened_json_and_delimited_sources_fit_widest_sampled_value() {
        let dir = tempfile::tempdir().expect("tempdir");
        let json_path = dir.path().join("rows.json");
        let csv_path = dir.path().join("rows.csv");
        let values = ["0", "0", "0", "0", "0", "0", "0", "4279369981"];
        let json = values
            .iter()
            .map(|value| format!(r#"{{"store":"{value}"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        std::fs::write(&json_path, format!("[{json}]")).expect("json");
        std::fs::write(&csv_path, format!("store\n{}\n", values.join("\n"))).expect("csv");

        let json = crate::ingest::JsonAdapter::json()
            .open(
                crate::ingest::source::InputSource::Path(json_path),
                &crate::ingest::OpenOptions {
                    format: crate::ingest::InputFormat::Json,
                    ..crate::ingest::OpenOptions::default()
                },
            )
            .expect("open json")
            .into_implicit_table()
            .expect("json table");
        let mut json_view =
            TableView::from_opened_table(json, Viewport::new(20, 2)).expect("json view");

        let csv = crate::ingest::DelimitedAdapter
            .open(
                crate::ingest::source::InputSource::Path(csv_path),
                &crate::ingest::OpenOptions::default(),
            )
            .expect("open csv")
            .into_implicit_table()
            .expect("csv table");
        let mut csv_view =
            TableView::from_opened_table(csv, Viewport::new(20, 2)).expect("csv view");

        assert_eq!(json_view.effective_column_widths_cached(), [10]);
        assert_eq!(csv_view.effective_column_widths_cached(), [10]);
    }

    #[test]
    fn incremental_indexing_keeps_cached_sampled_width_stable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rows.json");
        std::fs::write(&path, r#"[{"store":"0"},{"store":"4279369981"}]"#).expect("json");
        let opened = crate::ingest::JsonAdapter::json()
            .open(
                crate::ingest::source::InputSource::Path(path),
                &crate::ingest::OpenOptions {
                    format: crate::ingest::InputFormat::Json,
                    lazy_threshold_bytes: 1,
                    schema_scan_bytes: 1,
                    ..crate::ingest::OpenOptions::default()
                },
            )
            .expect("open json")
            .into_implicit_table()
            .expect("json table");
        let mut view =
            TableView::from_opened_table(opened, Viewport::new(1, 1)).expect("json view");

        assert_eq!(view.effective_column_widths_cached(), [5]);
        view.goto(1, 0);
        assert_eq!(view.effective_column_widths_cached(), [5]);
    }

    #[test]
    fn automatic_width_is_capped_at_eighty_percent_but_explicit_width_is_not() {
        let mut view = TableView::classify(rows(&[&["12345678901234567890"]]), Viewport::new(1, 1));
        view.set_terminal_width(20);

        assert_eq!(view.effective_column_widths_cached(), [16]);
        view.set_current_column_width(24);
        assert_eq!(view.effective_column_widths_cached(), [24]);
    }

    #[test]
    fn captures_and_applies_reload_state() {
        let mut view = TableView::classify(
            rows(&[&["Name", "Value"], &["alpha", "1"], &["beta", "2"]]),
            Viewport::new(1, 1),
        );
        view.goto(1, 1);
        let state = ReloadState::capture(
            &view,
            ColumnWidthMode::Mode,
            2,
            vec![5, 6],
            Some("beta".to_owned()),
        );
        let mut reloaded = TableView::classify(
            rows(&[&["Name", "Value"], &["alpha", "1"], &["beta", "2"]]),
            Viewport::new(1, 1),
        );
        state.apply_to(&mut reloaded);
        assert_eq!(reloaded.cursor(), Position { row: 1, column: 1 });
        assert_eq!(state.search.as_deref(), Some("beta"));
    }

    #[test]
    fn reload_settings_follow_structured_source_identity_after_reordering() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data.json");
        std::fs::write(&path, r#"[{"a":1,"b":2}]"#).expect("first source");
        let options = crate::ingest::OpenOptions {
            format: crate::ingest::InputFormat::Json,
            ..crate::ingest::OpenOptions::default()
        };
        let opened = crate::ingest::JsonAdapter::json()
            .open(
                crate::ingest::source::InputSource::Path(path.clone()),
                &options,
            )
            .expect("open first")
            .into_implicit_table()
            .expect("table");
        let mut previous =
            TableView::from_opened_table(opened, Viewport::new(5, 2)).expect("first view");
        previous.goto(0, 1);
        previous.set_current_column_width(17);
        previous.set_mark();
        let old_generation = previous.source_generation();

        std::fs::write(&path, r#"[{"b":2,"a":1}]"#).expect("replacement source");
        let opened = crate::ingest::JsonAdapter::json()
            .open(
                crate::ingest::source::InputSource::Path(path.clone()),
                &options,
            )
            .expect("open replacement")
            .into_implicit_table()
            .expect("table");
        let mut reloaded =
            TableView::from_opened_table(opened, Viewport::new(5, 2)).expect("replacement view");
        reloaded.restore_view_settings_from(&previous);

        assert_ne!(reloaded.source_generation(), old_generation);
        assert_eq!(reloaded.cursor().column, 0, "cursor follows canonical /b");
        assert_eq!(reloaded.column_widths[0], 17, "width follows canonical /b");
        assert!(
            reloaded.mark().is_none(),
            "row identity is generation-scoped"
        );
    }

    #[test]
    fn resizes_viewport_and_keeps_cursor_visible() {
        let mut view = TableView::classify(
            rows(&[
                &["A", "B", "C"],
                &["1", "2", "3"],
                &["4", "5", "6"],
                &["7", "8", "9"],
            ]),
            Viewport::new(3, 3),
        );
        view.goto(2, 2);
        view.resize_viewport(1, 1);
        assert_eq!(view.viewport().origin, Position { row: 2, column: 2 });
    }

    #[test]
    fn sorts_data_rows_by_current_column() {
        let mut view = TableView::classify(
            rows(&[&["Name", "Value"], &["b", "10"], &["a", "2"]]),
            Viewport::new(10, 2),
        );
        view.goto(0, 0);
        view.sort_current_column(SortMode::Lexical, SortDirection::Ascending);
        assert_eq!(view.rows(), rows(&[&["a", "2"], &["b", "10"]]));
    }

    #[test]
    fn page_motion_uses_viewport_size() {
        let mut view = TableView::classify(
            rows(&[
                &["A"],
                &["1"],
                &["2"],
                &["3"],
                &["4"],
                &["5"],
                &["6"],
                &["7"],
            ]),
            Viewport::new(3, 1),
        );
        view.page_by(1, 0, 2);
        assert_eq!(view.cursor().row, 6);
        view.page_by(-1, 0, 1);
        assert_eq!(view.cursor().row, 3);
    }

    #[test]
    fn mark_round_trips_cursor_position() {
        let mut view = TableView::classify(
            rows(&[&["A", "B"], &["1", "2"], &["3", "4"]]),
            Viewport::new(3, 2),
        );
        view.goto(1, 1);
        view.set_mark();
        view.goto(0, 0);
        view.goto_mark();
        assert_eq!(view.cursor(), Position { row: 1, column: 1 });
    }

    #[test]
    fn column_width_controls_update_effective_widths() {
        let mut view = TableView::classify(
            rows(&[&["Name", "Value"], &["alpha", "1"]]),
            Viewport::new(3, 2),
        );
        view.set_all_column_widths(4);
        assert_eq!(view.effective_column_widths(), [4, 4]);
        view.set_current_column_width(8);
        assert_eq!(view.effective_column_widths()[0], 8);
        view.adjust_current_column_width(-20);
        assert_eq!(view.effective_column_widths()[0], 1);
        view.adjust_column_gap(3);
        assert_eq!(view.column_gap(), 5);
    }

    #[test]
    fn single_column_resize_scales_and_caps_at_content_or_header_width() {
        let header = "A header wider than every value";
        let header_width = UnicodeWidthStr::width(header);
        let mut view = TableView::classify(
            rows(&[&[header], &["1"], &["1234567890"]]),
            Viewport::new(3, 1),
        );

        view.set_current_column_width(5);
        view.adjust_current_column_width(1);
        assert_eq!(view.effective_column_widths(), [6]);
        view.adjust_current_column_width(2);
        assert_eq!(view.effective_column_widths(), [8]);
        view.adjust_current_column_width(20);
        assert_eq!(view.effective_column_widths(), [header_width]);

        view.adjust_current_column_width(-1);
        assert_eq!(
            view.effective_column_widths(),
            [header_width.saturating_sub((header_width / 5).max(1))]
        );
        view.adjust_current_column_width(-20);
        assert_eq!(view.effective_column_widths(), [1]);
        view.adjust_current_column_width(-1);
        assert_eq!(view.effective_column_widths(), [1]);

        view.set_current_column_width(20);
        view.adjust_current_column_width(1);
        let widened = 24.min(header_width);
        assert_eq!(view.effective_column_widths(), [widened]);
        view.adjust_current_column_width(-1);
        assert_eq!(
            view.effective_column_widths(),
            [widened.saturating_sub((widened / 5).max(1))]
        );
    }

    #[test]
    fn cached_widths_do_not_become_explicit_widths() {
        let mut view = TableView::classify(
            rows(&[&["Name", "Value"], &["alpha", "1000"]]),
            Viewport::new(3, 2),
        );

        assert!(view.column_widths.is_empty());
        assert!(view.computed_column_widths_cache.is_empty());

        assert_eq!(view.effective_column_widths_cached(), vec![5, 5]);

        assert!(view.column_widths.is_empty());
        assert_eq!(view.computed_column_widths_cache, vec![5, 5]);

        view.set_current_column_width(8);
        assert_eq!(view.column_widths[0], 8);
        assert!(view.computed_column_widths_cache.is_empty());
    }

    #[test]
    fn filter_in_keeps_matching_visible_rows_without_mutating_backing_rows() {
        let mut view = TableView::classify(
            rows(&[
                &["Name", "Size"],
                &["alpha", "1gb"],
                &["beta", "2gb"],
                &["gamma", "3gb"],
            ]),
            Viewport::new(10, 2),
        );
        view.goto(0, 0);
        view.apply_filter(0, FilterMode::In, FilterKind::Text, "a".to_owned())
            .expect("apply filter");

        assert_eq!(view.row_count(), 3);
        assert_eq!(view.total_row_count(), 3);
        assert_eq!(
            view.rows(),
            rows(&[&["alpha", "1gb"], &["beta", "2gb"], &["gamma", "3gb"]])
        );
        assert_eq!(
            view.visible_rows_vec(),
            rows(&[&["alpha", "1gb"], &["beta", "2gb"], &["gamma", "3gb"]])
        );
    }

    #[test]
    fn filter_out_and_multiple_filters_reduce_visible_rows() {
        let mut view = TableView::classify(
            rows(&[
                &["Name", "Size"],
                &["alpha", "1gb"],
                &["beta", "2gb"],
                &["gamma", "3gb"],
            ]),
            Viewport::new(10, 2),
        );
        view.apply_filter(0, FilterMode::Out, FilterKind::Text, "beta".to_owned())
            .expect("apply text filter");
        view.apply_filter(1, FilterMode::In, FilterKind::Numeric, ">=2gb".to_owned())
            .expect("apply numeric filter");

        assert_eq!(view.visible_rows_vec(), rows(&[&["gamma", "3gb"]]));
    }

    #[test]
    fn filters_clamp_cursor_and_can_be_cleared() {
        let mut view = TableView::classify(
            rows(&[&["Name"], &["alpha"], &["beta"], &["gamma"]]),
            Viewport::new(1, 1),
        );
        view.goto(2, 0);
        view.apply_filter(0, FilterMode::In, FilterKind::Text, "alpha".to_owned())
            .expect("apply filter");

        assert_eq!(view.cursor(), Position { row: 0, column: 0 });
        assert_eq!(view.visible_rows_vec(), rows(&[&["alpha"]]));

        view.clear_filters_for_column(0);
        assert_eq!(view.row_count(), 3);
    }

    #[test]
    fn filtered_header_indicator_participates_in_widths() {
        let mut view = TableView::classify(
            rows(&[&["Name", "Size"], &["alpha", "1gb"]]),
            Viewport::new(10, 2),
        );
        view.apply_filter(1, FilterMode::In, FilterKind::Numeric, "<2gb".to_owned())
            .expect("apply filter");

        assert_eq!(
            view.rendered_header().expect("header"),
            vec!["Name".to_owned(), "+Size".to_owned()]
        );
        assert!(view.effective_column_widths()[1] >= 5);
    }

    #[test]
    fn auto_widths_expand_only_when_indicators_need_space() {
        let mut view = TableView::classify(
            rows(&[&["Name", "Size"], &["alpha", "1gb"]]),
            Viewport::new(10, 2),
        );

        assert_eq!(view.effective_column_widths(), vec![5, 4]);
        view.goto(0, 1);
        view.sort_current_column(SortMode::Numeric, SortDirection::Ascending);
        assert_eq!(
            view.rendered_header().expect("header"),
            vec!["Name".to_owned(), "▲Size".to_owned()]
        );
        assert_eq!(view.effective_column_widths(), vec![5, 5]);

        view.apply_filter(1, FilterMode::In, FilterKind::Numeric, "<2gb".to_owned())
            .expect("apply filter");

        assert_eq!(
            view.rendered_header().expect("header"),
            vec!["Name".to_owned(), "▲+Size".to_owned()]
        );
        assert_eq!(view.effective_column_widths(), vec![5, 6]);
    }

    #[test]
    fn filter_header_indicators_show_mode_and_multiples() {
        let mut view = TableView::classify(
            rows(&[
                &["Name", "Size"],
                &["alpha", "1gb"],
                &["beta", "2gb"],
                &["gamma", "3gb"],
            ]),
            Viewport::new(10, 2),
        );

        view.apply_filter(0, FilterMode::Out, FilterKind::Text, "beta".to_owned())
            .expect("apply out filter");
        view.apply_filter(1, FilterMode::In, FilterKind::Numeric, ">=1gb".to_owned())
            .expect("apply in filter");
        assert_eq!(
            view.rendered_header().expect("header"),
            vec!["-Name".to_owned(), "+Size".to_owned()]
        );

        view.apply_filter(0, FilterMode::In, FilterKind::Text, "a".to_owned())
            .expect("apply second name filter");
        assert_eq!(
            view.rendered_header().expect("header"),
            vec!["±Name".to_owned(), "+Size".to_owned()]
        );
    }

    #[test]
    fn filtering_and_navigation_do_not_shrink_computed_widths() {
        let mut view = TableView::classify(
            rows(&[
                &["Name", "Value"],
                &["short", "ok"],
                &["very-very-long", "hidden"],
            ]),
            Viewport::new(10, 2),
        );
        view.set_column_width_mode(ColumnWidthMode::Max);
        let widths_before_filter = view.effective_column_widths();

        view.apply_filter(1, FilterMode::In, FilterKind::Text, "ok".to_owned())
            .expect("apply filter");
        let widths_after_filter = view.effective_column_widths();
        view.move_by(1, 0);
        let widths_after_navigation = view.effective_column_widths();

        assert_eq!(widths_after_filter, widths_before_filter);
        assert_eq!(widths_after_navigation, widths_before_filter);
    }

    #[test]
    fn sorting_preserves_active_filters() {
        let mut view = TableView::classify(
            rows(&[
                &["Name", "Size"],
                &["gamma", "3gb"],
                &["alpha", "1gb"],
                &["beta", "2gb"],
            ]),
            Viewport::new(10, 2),
        );
        view.apply_filter(1, FilterMode::In, FilterKind::Numeric, ">=2gb".to_owned())
            .expect("apply filter");
        view.goto(0, 0);
        view.sort_current_column(SortMode::Lexical, SortDirection::Ascending);

        assert_eq!(
            view.visible_rows_vec(),
            rows(&[&["beta", "2gb"], &["gamma", "3gb"]])
        );
    }

    #[test]
    fn multi_level_sort_tracks_primary_key_and_header_markers() {
        let mut view = TableView::classify(
            rows(&[
                &["Name", "Shard"],
                &["b", "2"],
                &["a", "2"],
                &["c", "1"],
                &["a", "1"],
            ]),
            Viewport::new(10, 2),
        );

        view.goto(0, 1);
        view.sort_current_column(SortMode::Numeric, SortDirection::Ascending);
        view.goto(0, 0);
        view.sort_current_column(SortMode::Lexical, SortDirection::Ascending);

        assert_eq!(
            view.rows(),
            rows(&[&["a", "1"], &["a", "2"], &["b", "2"], &["c", "1"]])
        );
        assert_eq!(
            view.sort_keys(),
            &[
                ActiveSortKey {
                    column: 0,
                    mode: SortMode::Lexical,
                    direction: SortDirection::Ascending,
                    nulls: NullPlacement::Last,
                },
                ActiveSortKey {
                    column: 1,
                    mode: SortMode::Numeric,
                    direction: SortDirection::Ascending,
                    nulls: NullPlacement::Last,
                },
            ]
        );
        assert_eq!(
            view.rendered_header().expect("header"),
            vec!["▲Name".to_owned(), "▲Shard".to_owned()]
        );

        view.clear_current_column_sort();
        assert_eq!(view.sort_keys().len(), 1);
        assert_eq!(
            view.rendered_header().expect("header"),
            vec!["Name".to_owned(), "▲Shard".to_owned()]
        );
    }

    #[test]
    fn repeated_sort_toggles_primary_key_and_keeps_last_three_sorts() {
        let mut view = TableView::classify(
            rows(&[
                &["A", "B", "C", "D"],
                &["b", "2", "x", "m"],
                &["a", "1", "y", "n"],
            ]),
            Viewport::new(10, 4),
        );

        for column in 0..4 {
            view.goto(0, column);
            view.sort_current_column(SortMode::Lexical, SortDirection::Ascending);
        }

        assert_eq!(
            view.sort_keys()
                .iter()
                .map(|key| key.column)
                .collect::<Vec<_>>(),
            vec![3, 2, 1]
        );

        view.sort_current_column(SortMode::Lexical, SortDirection::Ascending);
        assert_eq!(
            view.sort_keys()
                .iter()
                .map(|key| key.column)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
    }

    #[test]
    fn hidden_columns_are_omitted_from_visible_rows_and_navigation() {
        let mut view = TableView::classify(
            rows(&[&["A", "B", "C"], &["a1", "b1", "c1"]]),
            Viewport::new(10, 3),
        );
        view.goto(0, 1);
        view.hide_current_column();

        assert_eq!(view.column_count(), 2);
        assert_eq!(view.visible_rows_vec(), rows(&[&["a1", "c1"]]));
        assert_eq!(
            view.rendered_header().expect("header"),
            rows(&[&["A", "C"]])[0]
        );
        assert_eq!(view.current_cell(), Some("c1"));
        assert!(view.hidden_boundary_before(1));

        view.show_hidden_left(1);
        assert_eq!(view.column_count(), 3);
        assert_eq!(view.visible_rows_vec(), rows(&[&["a1", "b1", "c1"]]));
    }

    #[test]
    fn hide_current_column_preserves_last_visible_column() {
        let mut view = TableView::classify(rows(&[&["A"], &["a1"]]), Viewport::new(10, 1));
        view.hide_current_column();

        assert_eq!(view.column_count(), 1);
        assert_eq!(view.current_cell(), Some("a1"));
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn applies_saved_column_width_alignment_and_visibility() {
        let mut view = TableView::classify(
            rows(&[&["Name", "Count", "Hidden"], &["alpha", "1000", "x"]]),
            Viewport::new(10, 3),
        );
        let parsed = crate::saved_views::parse_saved_view_yaml(
            r#"
name: counts
filenames: [counts.csv]
source: {}
view:
  columns:
    Count:
      type: integer
      width: 12
    Hidden:
      visible: false
"#,
        )
        .expect("parse");
        let headers = view.header().expect("header").to_vec();
        let resolved = crate::saved_views::resolve_columns(&parsed.view, &headers);

        view.apply_saved_columns(&resolved, None);

        assert_eq!(view.column_count(), 2);
        assert_eq!(view.effective_column_widths(), vec![5, 12]);
        assert_eq!(
            view.column_alignment_override(1),
            Some(ColumnAlignment::Right)
        );
        assert_eq!(view.visible_rows_vec(), rows(&[&["alpha", "1000"]]));
        assert!(view.hidden_boundary_after_last());
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn applies_saved_display_formats_without_mutating_raw_rows() {
        let mut view = TableView::classify(
            rows(&[
                &["Name", "Locale", "Mask", "Flag"],
                &["alpha", "1234.50", "1234.5", "yes"],
            ]),
            Viewport::new(10, 4),
        );
        let parsed = crate::saved_views::parse_saved_view_yaml(
            r##"
name: display
filenames: [display.csv]
source: {}
view:
  locale: de_DE
  columns:
    Name:
      type: text
      format: uppercase
    Locale:
      type: float
      format: locale
    Mask:
      type: float
      format: mask
      mask: "#,##0.00"
    Flag:
      type: word
      format: bit
"##,
        )
        .expect("parse");
        let headers = view.header().expect("header").to_vec();
        let resolved = crate::saved_views::resolve_columns(&parsed.view, &headers);

        view.apply_saved_columns(&resolved, parsed.view.view.locale.as_deref());

        assert_eq!(
            view.visible_rows_vec(),
            rows(&[&["ALPHA", "1.234,50", "1,234.50", "1"]])
        );
        assert_eq!(
            view.visible_raw_rows_vec(),
            rows(&[&["alpha", "1234.50", "1234.5", "yes"]])
        );
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn search_and_text_filter_match_raw_or_rendered_values() {
        let mut view = TableView::classify(
            rows(&[&["Count"], &["1000"], &["2000"]]),
            Viewport::new(10, 1),
        );
        let parsed = crate::saved_views::parse_saved_view_yaml(
            r#"
name: counts
filenames: [counts.csv]
source: {}
view:
  columns:
    Count:
      type: integer
      format: locale
"#,
        )
        .expect("parse");
        let headers = view.header().expect("header").to_vec();
        let resolved = crate::saved_views::resolve_columns(&parsed.view, &headers);
        view.apply_saved_columns(&resolved, Some("en_US"));

        assert_eq!(
            view.search_rows_vec(),
            rows(&[&["1000\n1,000"], &["2000\n2,000"]])
        );
        assert!(view.visible_cell_matches_search_query(0, 0, "1000"));
        assert!(view.visible_cell_matches_search_query(0, 0, "1,000"));
        view.apply_filter(0, FilterMode::In, FilterKind::Text, "1,000".to_owned())
            .expect("filter");
        assert_eq!(view.visible_raw_rows_vec(), rows(&[&["1000"]]));
        assert_eq!(view.visible_rows_vec(), rows(&[&["1,000"]]));
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn conditional_colors_apply_without_changing_values() {
        let mut view = TableView::classify(
            rows(&[
                &["Status", "Percent"],
                &["active", "5%"],
                &["idle", "50%"],
                &["down", "95%"],
            ]),
            Viewport::new(10, 2),
        );
        let parsed = crate::saved_views::parse_saved_view_yaml(
            r#"
name: colors
filenames: [data.csv]
source: {}
view:
  columns:
    Status:
      colors:
        - match:
            active: green
    Percent:
      type: number
      colors:
        - range:
            "<10": red
        - gradient:
            mode: auto
            colors: [green, yellow]
"#,
        )
        .expect("parse");
        let headers = view.header().expect("header").to_vec();
        let resolved = crate::saved_views::resolve_columns(&parsed.view, &headers);
        view.apply_saved_columns(&resolved, None);

        assert_eq!(
            view.conditional_color_for_visible_cell(0, 0),
            Some("green".to_owned())
        );
        assert_eq!(
            view.conditional_color_for_visible_cell(0, 1),
            Some("red".to_owned())
        );
        assert_eq!(
            view.conditional_color_for_visible_cell(1, 1),
            Some("gradient(4;8;5:green6:yellow)".to_owned())
        );
        assert!(matches!(
            view.conditional_color_for_source_cell(1, "50%", "50%"),
            Some(Cow::Borrowed(_))
        ));
        assert_eq!(
            view.visible_rows_vec(),
            rows(&[&["active", "5%"], &["idle", "50%"], &["down", "95%"]])
        );
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn saved_view_yaml_quotes_string_match_values_that_look_numeric() {
        let mut view = TableView::classify(rows(&[&["Code"], &["10"]]), Viewport::new(10, 1));
        let parsed = crate::saved_views::parse_saved_view_yaml(
            r#"
name: colors
filenames: [data.csv]
source: {}
view:
  columns:
    Code:
      colors:
        - match:
            "10": green
"#,
        )
        .expect("parse");
        let headers = view.header().expect("header").to_vec();
        let resolved = crate::saved_views::resolve_columns(&parsed.view, &headers);
        view.apply_saved_columns(&resolved, None);

        let yaml = view.to_saved_view_yaml("colors", "data.csv", None);

        assert!(yaml.contains("          \"10\": green"));
        assert_eq!(
            conditional_value_yaml(&ConditionalValue::String("a\nb".to_owned())),
            "\"a\\nb\""
        );
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn keyed_object_view_fills_the_first_viewport_and_saves_resolved_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("repositories.json");
        let entries = (0..80)
            .map(|index| format!(r#""repo-{index}":{{"type":"fs","ordinal":{index}}}"#))
            .collect::<Vec<_>>()
            .join(",");
        std::fs::write(&path, format!("{{{entries}}}")).expect("write");
        let options = crate::ingest::OpenOptions {
            format: crate::ingest::InputFormat::Json,
            lazy_threshold_bytes: 1,
            schema_scan_bytes: 1,
            ..crate::ingest::OpenOptions::default()
        };
        let opened = crate::ingest::JsonAdapter::json()
            .open(
                crate::ingest::source::InputSource::Path(path.clone()),
                &options,
            )
            .expect("open")
            .into_implicit_table()
            .expect("table");
        let view = TableView::from_opened_table(opened, Viewport::new(8, 80)).expect("view");

        assert_eq!(view.visible_rows_vec().len(), 8);
        assert_eq!(
            view.current_column_info()
                .unwrap()
                .canonical_source
                .as_deref(),
            Some("@key")
        );
        let yaml = view.to_saved_view_yaml_with_source_options(
            "repositories",
            "repositories.json",
            None,
            &options,
        );
        assert!(yaml.contains("source:\n  format: json\n  object_mode: entries\n"));
        assert!(yaml.contains("\nview: {}\n"));

        let record_options = crate::ingest::OpenOptions {
            format: crate::ingest::InputFormat::Json,
            object_mode: crate::ingest::ObjectMode::Record,
            object_mode_origin: crate::ingest::ObjectModeOrigin::Cli,
            ..crate::ingest::OpenOptions::default()
        };
        let record = crate::ingest::JsonAdapter::json()
            .open(
                crate::ingest::source::InputSource::Path(path),
                &record_options,
            )
            .expect("record open")
            .into_implicit_table()
            .expect("record table");
        let record_view =
            TableView::from_opened_table(record, Viewport::new(8, 80)).expect("record view");
        assert!(record_view
            .to_saved_view_yaml_with_source_options(
                "record",
                "repositories.json",
                None,
                &record_options,
            )
            .contains("source:\n  format: json\n  object_mode: record\n"));

        let array_path = dir.path().join("array.json");
        std::fs::write(&array_path, r#"[{"name":"one"}]"#).expect("array write");
        let array = crate::ingest::JsonAdapter::json()
            .open(
                crate::ingest::source::InputSource::Path(array_path),
                &crate::ingest::OpenOptions {
                    format: crate::ingest::InputFormat::Json,
                    ..crate::ingest::OpenOptions::default()
                },
            )
            .expect("array open")
            .into_implicit_table()
            .expect("array table");
        let array_view =
            TableView::from_opened_table(array, Viewport::new(8, 80)).expect("array view");
        assert!(!array_view
            .to_saved_view_yaml("array", "array.json", None)
            .contains("object_mode:"));
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn column_info_updates_rebuild_identifier_color_metadata() {
        let mut view = TableView::classify(rows(&[&["Name"], &["alpha"]]), Viewport::new(10, 1));
        let parsed = crate::saved_views::parse_saved_view_yaml(
            r#"
name: identifiers
filenames: [data.csv]
source: {}
view:
  columns:
    Name:
      type: text
      format: uppercase
      colors:
        - identifiers:
            colors: auto
"#,
        )
        .expect("parse");
        let headers = view.header().expect("header").to_vec();
        let resolved = crate::saved_views::resolve_columns(&parsed.view, &headers);
        view.apply_saved_columns(&resolved, None);

        assert_eq!(view.visible_rows_vec(), rows(&[&["ALPHA"]]));
        assert_eq!(
            view.conditional_color_for_visible_cell(0, 0),
            Some("identifier(0)".to_owned())
        );

        view.apply_current_column_info(ColumnInfoUpdate {
            visible: true,
            alignment: None,
            column_type: ColumnTypeChoice::Text,
            format: ColumnFormatChoice::Lowercase,
            sort: ColumnSortChoice::None,
            nulls: ColumnNullPlacementChoice::Inherited,
            clear_filters: false,
        });

        assert_eq!(view.visible_rows_vec(), rows(&[&["alpha"]]));
        assert_eq!(
            view.conditional_color_for_visible_cell(0, 0),
            Some("identifier(0)".to_owned())
        );
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn identifier_colors_are_stable_for_unique_rendered_values() {
        let mut view = TableView::classify(
            rows(&[&["Address"], &["10.0.0.2"], &["10.0.0.1"], &["10.0.0.2"]]),
            Viewport::new(10, 1),
        );
        let parsed = crate::saved_views::parse_saved_view_yaml(
            r#"
name: identifiers
filenames: [data.csv]
source: {}
view:
  columns:
    Address:
      type: ip
      colors:
        - identifiers: {}
"#,
        )
        .expect("parse");
        let headers = view.header().expect("header").to_vec();
        let resolved = crate::saved_views::resolve_columns(&parsed.view, &headers);
        view.apply_saved_columns(&resolved, None);

        let first = view
            .conditional_color_for_visible_cell(0, 0)
            .expect("first color");
        let second = view
            .conditional_color_for_visible_cell(1, 0)
            .expect("second color");
        let repeated = view
            .conditional_color_for_visible_cell(2, 0)
            .expect("repeated color");

        assert_ne!(first, second);
        assert_eq!(first, repeated);
        assert!(first.starts_with("identifier("));
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn restored_view_settings_preserve_conditional_color_cache() {
        let mut view = TableView::classify(
            rows(&[&["Address"], &["10.0.0.2"], &["10.0.0.1"]]),
            Viewport::new(10, 1),
        );
        let parsed = crate::saved_views::parse_saved_view_yaml(
            r#"
name: identifiers
filenames: [data.csv]
source: {}
view:
  columns:
    Address:
      type: ip
      colors:
        - identifiers: {}
"#,
        )
        .expect("parse");
        let headers = view.header().expect("header").to_vec();
        let resolved = crate::saved_views::resolve_columns(&parsed.view, &headers);
        view.apply_saved_columns(&resolved, None);

        let mut restored = TableView::classify(
            rows(&[&["Address"], &["10.0.0.2"], &["10.0.0.1"]]),
            Viewport::new(10, 1),
        );
        restored.restore_view_settings_from(&view);

        assert_eq!(
            restored.conditional_color_for_visible_cell(0, 0),
            view.conditional_color_for_visible_cell(0, 0)
        );
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn saved_type_metadata_enables_date_semver_and_ip_sorting() {
        let mut date_view = TableView::classify(
            rows(&[
                &["Created"],
                &["2024-01-01T00:00:00Z"],
                &["2023-12-31T23:00:00Z"],
            ]),
            Viewport::new(10, 1),
        );
        let parsed = crate::saved_views::parse_saved_view_yaml(
            r#"
name: dates
filenames: [dates.csv]
source: {}
view:
  columns:
    Created:
      type: date
"#,
        )
        .expect("parse");
        let headers = date_view.header().expect("header").to_vec();
        let resolved = crate::saved_views::resolve_columns(&parsed.view, &headers);
        date_view.apply_saved_columns(&resolved, None);
        date_view.sort_current_column(SortMode::Lexical, SortDirection::Ascending);
        assert_eq!(
            date_view.visible_raw_rows_vec(),
            rows(&[&["2023-12-31T23:00:00Z"], &["2024-01-01T00:00:00Z"]])
        );

        let mut semver_view = TableView::classify(
            rows(&[&["Version"], &["1.10.0"], &["1.2.0"]]),
            Viewport::new(10, 1),
        );
        let parsed = crate::saved_views::parse_saved_view_yaml(
            r#"
name: versions
filenames: [versions.csv]
source: {}
view:
  columns:
    Version:
      type: semver
"#,
        )
        .expect("parse");
        let headers = semver_view.header().expect("header").to_vec();
        let resolved = crate::saved_views::resolve_columns(&parsed.view, &headers);
        semver_view.apply_saved_columns(&resolved, None);
        semver_view.sort_current_column(SortMode::Lexical, SortDirection::Ascending);
        assert_eq!(
            semver_view.visible_raw_rows_vec(),
            rows(&[&["1.2.0"], &["1.10.0"]])
        );

        let mut ip_view = TableView::classify(
            rows(&[&["Address"], &["2001:db8::1"], &["10.0.0.2"]]),
            Viewport::new(10, 1),
        );
        let parsed = crate::saved_views::parse_saved_view_yaml(
            r#"
name: ips
filenames: [ips.csv]
source: {}
view:
  columns:
    Address:
      type: ip
"#,
        )
        .expect("parse");
        let headers = ip_view.header().expect("header").to_vec();
        let resolved = crate::saved_views::resolve_columns(&parsed.view, &headers);
        ip_view.apply_saved_columns(&resolved, None);
        ip_view.sort_current_column(SortMode::Lexical, SortDirection::Ascending);
        assert_eq!(
            ip_view.visible_raw_rows_vec(),
            rows(&[&["10.0.0.2"], &["2001:db8::1"]])
        );
    }

    #[test]
    fn opened_incremental_table_loads_viewport_then_indexes_navigation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data.csv");
        std::fs::write(&path, "name,value\na,1\na,2\nc,3\n").expect("write");
        let options = crate::ingest::OpenOptions {
            lazy_threshold_bytes: 0,
            ..crate::ingest::OpenOptions::default()
        };
        let opened = crate::ingest::DelimitedAdapter
            .open(crate::ingest::source::InputSource::Path(path), &options)
            .expect("open")
            .into_implicit_table()
            .expect("table");
        let mut view = TableView::from_opened_table(opened, Viewport::new(1, 2)).expect("view");
        assert_eq!(view.total_row_count(), 1);
        assert!(matches!(view.row_count_state(), RowCount::AtLeast(_)));

        let found = view
            .progressive_search("c", crate::ops::search::SearchDirection::Forward)
            .expect("progressive match");
        assert_eq!(found, Position { row: 2, column: 0 });
        assert_eq!(view.total_row_count(), 3);
        view.goto(found.row, found.column);
        assert_eq!(view.current_raw_cell(), Some("c"));

        view.goto(0, 0);
        assert_eq!(
            view.progressive_skip_to_change(
                crate::ops::skip::Axis::Row,
                crate::ops::skip::Direction::Forward,
                1,
            ),
            Position { row: 2, column: 0 }
        );
    }

    #[test]
    fn goto_bottom_batch_loads_large_incremental_delimited_source() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("large.csv");
        let mut data = String::from("value\n");
        for value in 0..10_000 {
            data.push_str(&format!("{value}\n"));
        }
        std::fs::write(&path, data).expect("write");
        let opened = crate::ingest::DelimitedAdapter
            .open(
                crate::ingest::source::InputSource::Path(path),
                &crate::ingest::OpenOptions {
                    lazy_threshold_bytes: 0,
                    ..crate::ingest::OpenOptions::default()
                },
            )
            .expect("open")
            .into_implicit_table()
            .expect("table");
        let mut view = TableView::from_opened_table(opened, Viewport::new(1, 1)).expect("view");
        assert_eq!(view.effective_column_widths_cached(), [5]);

        view.goto_bottom();

        assert_eq!(view.row_count_state(), RowCount::Exact(10_000));
        assert_eq!(view.current_raw_cell(), Some("9999"));
        assert_eq!(view.effective_column_widths_cached(), [5]);
    }

    #[test]
    fn opened_typed_table_queries_preserve_identity_and_null_policy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data.json");
        std::fs::write(&path, r#"[{"n":2},{"n":null},{"n":1}]"#).expect("write");
        let options = crate::ingest::OpenOptions {
            format: crate::ingest::InputFormat::Json,
            ..crate::ingest::OpenOptions::default()
        };
        let opened = crate::ingest::JsonAdapter::json()
            .open(crate::ingest::source::InputSource::Path(path), &options)
            .expect("open")
            .into_implicit_table()
            .expect("table");
        let mut view = TableView::from_opened_table(opened, Viewport::new(3, 1)).expect("view");
        view.goto(2, 0);
        view.set_view_null_placement(Some(NullPlacement::First));
        view.sort_current_column(SortMode::Numeric, SortDirection::Ascending);
        assert_eq!(view.visible_raw_rows_vec(), rows(&[&[""], &["1"], &["2"]]));
        assert_eq!(view.current_raw_cell(), Some("1"));
        assert_eq!(
            view.active_view_transform().unwrap().order_by[0].nulls,
            NullPlacement::First
        );

        view.clear_current_column_sort();
        assert_eq!(view.visible_raw_rows_vec(), rows(&[&["2"], &[""], &["1"]]));
        assert_eq!(view.current_raw_cell(), Some("1"));

        view.goto(0, 0);
        view.set_mark();
        view.apply_filter(0, FilterMode::In, FilterKind::Text, "1".to_owned())
            .expect("filter");
        assert_eq!(view.visible_raw_rows_vec(), rows(&[&["1"]]));
        view.goto_mark();
        assert_eq!(view.current_raw_cell(), Some("1"));
        view.clear_filters_for_column(0);
        view.goto_mark();
        assert_eq!(view.current_raw_cell(), Some("2"));
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn pending_canonical_saved_column_applies_when_schema_delta_arrives() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("late.json");
        std::fs::write(&path, r#"[{"a":1},{"a":2,"late":3}]"#).expect("write");
        let options = crate::ingest::OpenOptions {
            format: crate::ingest::InputFormat::Json,
            schema_scan_bytes: 1,
            ..crate::ingest::OpenOptions::default()
        };
        let opened = crate::ingest::JsonAdapter::json()
            .open(crate::ingest::source::InputSource::Path(path), &options)
            .expect("open")
            .into_implicit_table()
            .expect("table");
        let mut view = TableView::from_opened_table(opened, Viewport::new(1, 2)).expect("view");
        let saved = crate::saved_views::parse_saved_view_yaml(
            r#"
name: late
filenames: [late.json]
source: {}
view:
  columns:
    /late:
      label: Later
      nulls: first
"#,
        )
        .expect("saved");
        let resolved = crate::saved_views::resolve_structured_columns(
            &saved.view,
            view.table_definition().expect("definition"),
        );
        assert!(resolved.pending.contains_key("/late"));
        view.apply_saved_columns(&resolved, None);

        view.goto(1, 0);
        assert_eq!(view.header().unwrap()[1], "Later");
        assert_eq!(view.resolved_null_placement(1), NullPlacement::First);
        assert!(matches!(
            &view.table_definition().unwrap().columns[1].source_identity,
            crate::table::ColumnSourceIdentity::StructuredPath(pointer) if pointer.as_str() == "/late"
        ));
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn pending_structured_sort_and_filter_apply_when_late_column_arrives() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("late.json");
        std::fs::write(&path, r#"[{"a":1},{"a":2,"late":3}]"#).expect("write");
        let options = crate::ingest::OpenOptions {
            format: crate::ingest::InputFormat::Json,
            schema_scan_bytes: 1,
            ..crate::ingest::OpenOptions::default()
        };
        let opened = crate::ingest::JsonAdapter::json()
            .open(crate::ingest::source::InputSource::Path(path), &options)
            .expect("open")
            .into_implicit_table()
            .expect("table");
        let mut view = TableView::from_opened_table(opened, Viewport::new(1, 2)).expect("view");
        let saved = crate::saved_views::parse_saved_view_yaml(
            r#"
name: late operations
filenames: [late.json]
source: {}
view:
  sort:
    - column: /late
      direction: desc
      kind: numeric
  filters:
    - column: /late
      action: in
      kind: numeric
      condition: "> 2"
"#,
        )
        .expect("saved");
        view.retain_pending_saved_operations(
            saved.view.view.sort.clone(),
            saved.view.view.filters.clone(),
        );

        view.goto(1, 0);

        let query = view.active_view_transform().expect("query");
        assert_eq!(query.order_by.len(), 1);
        assert_eq!(query.order_by[0].column.ordinal, 1);
        assert_eq!(query.filters.len(), 1);
        assert_eq!(query.filters[0].column.ordinal, 1);
        assert_eq!(view.visible_raw_rows_vec(), rows(&[&["2", "3"]]));
    }
}
