#[cfg(feature = "saved-views")]
mod binding;
mod cells;
mod colors;
mod column;
mod column_info;
mod export;
mod filtering;
mod identity;
mod indexing;
mod navigation;
mod open;
pub(crate) mod render;
mod restore;
#[cfg(feature = "saved-views")]
mod saved_yaml;
mod search;
mod sorting;
mod source_activation;
mod source_details;
mod transform;
mod visibility;
mod width;

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fmt;
use std::rc::Rc;

use crate::ops::filter::{ActiveFilter, FilterCondition, FilterKind, FilterMode, FilterParseError};
use crate::ops::search::CaseInsensitiveQuery;
use crate::ops::sort::{SortDirection, SortMode};
use crate::table::{InMemoryTable, NullPlacement, SourceGeneration, TableDefinition, TableStore};
use crate::theme::{CompiledColumnColors, ConditionalColorRule};
use column::Columns;

pub use width::column_widths;

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

enum CandidateRestoration {
    Stable {
        cursor_identity: Option<crate::table::StableRowIdentity>,
        mark_identity: Option<crate::table::StableRowIdentity>,
    },
    Reload {
        cursor_row: usize,
    },
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
pub(crate) struct VisibleCellStyleContext {
    pub(crate) conditional_color: Option<ratatui::style::Color>,
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
pub(crate) enum ColumnTypeMetadata {
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
pub(crate) enum DisplayFormatMetadata {
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
            .or_else(binding::system_locale)
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
pub(crate) struct ColumnDisplayMetadata {
    pub(crate) column_type: ColumnTypeMetadata,
    pub(crate) type_explicit: bool,
    pub(crate) format: DisplayFormatMetadata,
    mask: Option<NumberMaskMetadata>,
    locale: LocaleMetadata,
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
struct PendingSourceResult {
    revision: u64,
    options: crate::ingest::OpenOptions,
    cursor_identity: Option<crate::table::StableRowIdentity>,
    mark_identity: Option<crate::table::StableRowIdentity>,
}

#[derive(Debug, Clone)]
pub struct TableView {
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
    /// Schema consumed during projection preparation, including failed attempts,
    /// is replayed on ordinary viewer progress without changing export screen state.
    pending_projection_schema: RefCell<Option<(SourceGeneration, crate::table::SchemaDelta)>>,
    source_query_provenance: Option<crate::table::NativeQueryArtifact>,
    source_query_coordinator: Rc<RefCell<crate::table::SourceQueryCoordinator>>,
    committed_source: Option<crate::ingest::OpenOptions>,
    pending_source: Option<PendingSourceResult>,
    latest_activation_error: Option<String>,
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
    compiled_column_colors: Vec<Option<CompiledColumnColors>>,
    color_cache_valid: bool,
    exact_color_profiles: Option<Vec<crate::table::ColumnReductionProfile>>,
    column_metadata_modified: BTreeSet<usize>,
    sort_keys: Vec<ActiveSortKey>,
    columns: Columns,
    #[cfg(feature = "saved-views")]
    resident_source_header: Option<Vec<String>>,
    #[cfg(feature = "saved-views")]
    saved_binding: Option<crate::saved_views::binding::SavedViewBinding>,
    #[cfg(feature = "saved-views")]
    saved_binding_warnings: Vec<crate::saved_views::SavedViewWarning>,
    #[cfg(feature = "saved-views")]
    saved_column_widths: Vec<Option<crate::saved_views::ColumnWidth>>,
}

impl TableView {
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
        self.install_filter(ActiveFilter::new(
            source_column,
            mode,
            kind,
            input,
            condition,
        ));
        Ok(())
    }

    pub(crate) fn source_cell_style_context(
        &self,
        row: usize,
        source_column: usize,
        rendered: &str,
        query: Option<&CaseInsensitiveQuery<'_>>,
    ) -> VisibleCellStyleContext {
        let Some(source_row) = self.source_row_for_visible_row(row) else {
            return VisibleCellStyleContext {
                conditional_color: None,
                search_match: false,
            };
        };
        self.style_indexed_cell(source_row, (source_column, rendered), query)
    }
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

#[cfg(test)]
mod lifecycle_tests;

#[cfg(all(test, feature = "saved-views"))]
mod color_profile_tests;

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

#[cfg(test)]
mod tests;
