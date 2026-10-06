mod column;
pub(crate) mod render;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::rc::Rc;

use unicode_width::UnicodeWidthStr;

use crate::ingest::OpenedTable;
use crate::ops::filter::{ActiveFilter, FilterCondition, FilterKind, FilterMode, FilterParseError};
use crate::ops::search::CaseInsensitiveQuery;
use crate::ops::sort::{
    parse_numeric_scalar, sort_rows_by_specs, NumericColumnProfile, SortDirection, SortMode,
    SortSpec,
};
use crate::table::{
    InMemoryTable, NullPlacement, RowCount, RowIndex, SourceGeneration, TableDefinition, TableStore,
};
#[cfg(feature = "saved-views")]
use crate::theme::ConditionalValue;
use crate::theme::{
    ColorProfileDemand, ColorProfileScope, ColumnColorProfile, CompiledColumnColors,
    ConditionalColorRule, ResolvedTheme,
};
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

fn compatible_source_column(
    old: &crate::table::ColumnDefinition,
    new: &crate::table::ColumnDefinition,
) -> bool {
    use crate::table::ColumnSourceIdentity;
    let lineage = match (&old.source_identity, &new.source_identity) {
        (
            ColumnSourceIdentity::RelationColumn {
                relation: left,
                name: left_name,
                ..
            },
            ColumnSourceIdentity::RelationColumn {
                relation: right,
                name: right_name,
                ..
            },
        ) => left == right && left_name == right_name,
        (
            ColumnSourceIdentity::Delimited {
                ordinal: left,
                name: left_name,
            },
            ColumnSourceIdentity::Delimited {
                ordinal: right,
                name: right_name,
            },
        ) => left == right && left_name == right_name,
        (left, right) => left == right,
    };
    if !lineage {
        return false;
    }
    if matches!((&old.source_declared_type, &new.source_declared_type),
        (Some(left), Some(right)) if left != right)
    {
        return false;
    }
    use crate::table::LogicalType;
    matches!(
        (old.source_type, new.source_type),
        (left, right) if left == right
            || matches!(left, LogicalType::Unknown | LogicalType::Null)
            || matches!(right, LogicalType::Unknown | LogicalType::Null)
            || matches!((left, right), (LogicalType::Integer, LogicalType::Float)
                | (LogicalType::Float, LogicalType::Integer))
    )
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
            pending_projection_schema: RefCell::new(None),
            source_query_provenance: None,
            source_query_coordinator: Rc::new(RefCell::new(
                crate::table::SourceQueryCoordinator::default(),
            )),
            committed_source: None,
            pending_source: None,
            latest_activation_error: None,
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
            compiled_column_colors: vec![None; columns.len()],
            color_cache_valid: true,
            exact_color_profiles: None,
            column_metadata_modified: BTreeSet::new(),
            sort_keys: Vec::new(),
            columns,
            #[cfg(feature = "saved-views")]
            resident_source_header: None,
            #[cfg(feature = "saved-views")]
            saved_binding: None,
            #[cfg(feature = "saved-views")]
            saved_binding_warnings: Vec::new(),
            #[cfg(feature = "saved-views")]
            saved_column_widths: vec![None; column_count],
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
            pending_projection_schema: RefCell::new(None),
            source_query_provenance,
            source_query_coordinator: Rc::new(RefCell::new(
                crate::table::SourceQueryCoordinator::default(),
            )),
            committed_source: None,
            pending_source: None,
            latest_activation_error: None,
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
            compiled_column_colors: vec![None; columns.len()],
            color_cache_valid: true,
            exact_color_profiles: None,
            column_metadata_modified: BTreeSet::new(),
            sort_keys: Vec::new(),
            columns,
            #[cfg(feature = "saved-views")]
            resident_source_header: None,
            #[cfg(feature = "saved-views")]
            saved_binding: None,
            #[cfg(feature = "saved-views")]
            saved_binding_warnings: Vec::new(),
            #[cfg(feature = "saved-views")]
            saved_column_widths: vec![None; column_count],
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
    /// Snapshot only presentation and local operation facts. Source indexing
    /// may advance through the attached store, but the live viewer, cursor,
    /// viewport, and store attachment remain untouched even on failure.
    pub(crate) fn prepare_projection(
        &self,
        requirements: crate::output::OutputRequirements,
        theme: &ResolvedTheme,
    ) -> anyhow::Result<crate::projection::FrozenProjection> {
        let mut definition = self
            .table_definition
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("live output has no activated source definition"))?
            .clone();
        if let Some((generation, replay)) = self.pending_projection_schema.borrow().as_ref() {
            if *generation == definition.generation {
                definition.apply_delta(replay.clone())?;
            }
        }
        let shared = self
            .incremental_store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("live output has no attached source store"))?;
        let mut settings = crate::projection::ProjectionSettings::new(
            self.column_width_mode,
            #[cfg(feature = "saved-views")]
            None,
            #[cfg(feature = "saved-views")]
            true,
            &definition,
        );
        settings.gap = self.column_gap;
        settings.hidden = self.hidden_columns.clone();
        settings.filters = self.filters.clone();
        settings.sorts = self.sort_keys.clone();
        settings.numeric_requested = settings
            .filters
            .iter()
            .any(|filter| filter.kind == FilterKind::Numeric);
        settings.sort_requested = !settings.sorts.is_empty();
        settings
            .display
            .extend(self.column_display.iter().copied().enumerate());
        settings.formatted_filter_profile_requested = settings
            .filters
            .iter()
            .any(|filter| filter.kind != FilterKind::Numeric)
            && settings.display.values().any(|metadata| {
                matches!(
                    metadata.format,
                    DisplayFormatMetadata::Locale | DisplayFormatMetadata::Mask,
                )
            });
        settings.numeric_requested |= settings.formatted_filter_profile_requested;
        settings.colors.extend(
            self.column_color_rules
                .iter()
                .enumerate()
                .filter(|(_, rules)| !rules.is_empty())
                .map(|(column, rules)| (column, rules.clone())),
        );
        settings.labels.extend(
            self.column_label_overrides
                .iter()
                .enumerate()
                .filter_map(|(column, label)| label.clone().map(|label| (column, label))),
        );
        settings.alignment.extend(
            self.column_alignment_overrides
                .iter()
                .enumerate()
                .filter_map(|(column, alignment)| alignment.map(|alignment| (column, alignment))),
        );
        settings
            .fixed_widths
            .extend(self.column_width_modified.iter().filter_map(|&column| {
                #[cfg(feature = "saved-views")]
                if self
                    .saved_column_widths
                    .get(column)
                    .is_some_and(Option::is_some)
                {
                    return None;
                }
                self.column_widths
                    .get(column)
                    .copied()
                    .map(|width| (column, width))
            }));
        #[cfg(feature = "saved-views")]
        {
            settings.binding = self.saved_binding.clone();
            settings.numeric_requested |= settings
                .binding
                .as_ref()
                .is_some_and(|binding| binding.has_pending_numeric_filters());
            settings.saved_widths.extend(
                self.saved_column_widths
                    .iter()
                    .enumerate()
                    .filter_map(|(column, width)| width.map(|width| (column, width))),
            );
        }
        let mut delta = crate::table::SchemaDelta::default();
        let prepared = crate::projection::prepare(
            crate::projection::SourceInput {
                store: &mut **shared.0.borrow_mut(),
                definition: definition.clone(),
                partial: self.source_result_is_partial,
                replay_schema: Some(&mut delta),
            },
            settings,
            crate::projection::ProjectionPolicy::Complete,
            requirements,
            theme,
        );
        if !delta.is_empty() {
            let mut pending = self.pending_projection_schema.borrow_mut();
            match pending.as_mut() {
                Some((generation, replay)) if *generation == definition.generation => {
                    replay.added_columns.extend(delta.added_columns);
                    replay.widened_types.extend(delta.widened_types);
                    replay.completed |= delta.completed;
                }
                _ => *pending = Some((definition.generation, delta)),
            }
        }
        prepared
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
            match &definition.columns[index].source_identity {
                crate::table::ColumnSourceIdentity::Delimited {
                    name: Some(name), ..
                } => (definition
                    .columns
                    .iter()
                    .filter(|column| {
                        matches!(&column.source_identity,
                            crate::table::ColumnSourceIdentity::Delimited { name: Some(other), .. }
                            if other == name)
                    })
                    .count()
                    == 1)
                    .then(|| name.clone()),
                crate::table::ColumnSourceIdentity::Delimited {
                    ordinal,
                    name: None,
                }
                | crate::table::ColumnSourceIdentity::Positional(ordinal) => {
                    Some(format!("column_{}", ordinal + 1))
                }
                _ => None,
            }
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
        #[cfg(feature = "saved-views")]
        if let Some(binding) = self.saved_binding.as_mut() {
            binding.supersede_sorts();
            binding.supersede_all_filters();
        }
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

    /// Transfer validated opening settings only after the initial result and view bind.
    pub fn initialize_source_configuration(
        &mut self,
        options: crate::ingest::OpenOptions,
    ) -> anyhow::Result<()> {
        let committed = self.resolve_source_configuration(options)?;
        self.committed_source = Some(committed);
        Ok(())
    }

    pub fn committed_open_options(&self) -> Option<&crate::ingest::OpenOptions> {
        self.committed_source.as_ref()
    }

    fn resolve_source_configuration(
        &self,
        mut options: crate::ingest::OpenOptions,
    ) -> anyhow::Result<crate::ingest::OpenOptions> {
        if let Some(query) = self.active_source_query.as_ref() {
            options.limit = (query.limit != std::num::NonZeroUsize::MAX).then_some(query.limit);
            options.source_filters = query
                .filters
                .iter()
                .map(|filter| {
                    let column = match filter.scope {
                        crate::table::SourceFilterScope::WholeRecord => "*".to_owned(),
                        crate::table::SourceFilterScope::Column(id) => {
                            self.source_column_name_for_id(id).ok_or_else(|| {
                                anyhow::anyhow!("source filter column has no durable key")
                            })?
                        }
                    };
                    Ok(crate::ingest::SourceFilterRequest {
                        column,
                        operator: filter.operator,
                        operand: filter.operand.clone(),
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            options.source_sort = query
                .order_by
                .iter()
                .map(|sort| {
                    Ok(crate::ingest::SourceSortRequest {
                        column: self.source_column_name_for_id(sort.column).ok_or_else(|| {
                            anyhow::anyhow!("source sort column has no durable key")
                        })?,
                        direction: sort.direction,
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            if options.native_query.is_none()
                && options.table.is_none()
                && self.source_query_provenance.is_some()
            {
                options.table = self
                    .table_definition
                    .as_ref()
                    .map(|definition| definition.relation.name.clone());
            }
        }
        Ok(options)
    }

    pub fn request_source_query(
        &mut self,
        query: crate::table::SourceQuery,
        native_query_changed: bool,
    ) -> bool {
        let Some(definition) = self.table_definition.as_ref() else {
            self.source_status = Some("Source queries require a store-backed table".to_owned());
            return false;
        };
        if let Err(error) = crate::table::validate_source_query(definition, &query) {
            self.source_status = Some(format!("Source query validation failed: {error}"));
            return false;
        }
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
        let (cursor_identity, mark_identity) = match (
            self.current_stable_row_identity(),
            self.mark_stable_row_identity(),
        ) {
            (Ok(cursor), Ok(mark)) => (cursor, mark),
            (Err(error), _) | (_, Err(error)) => {
                self.source_status = Some(format!("Source identity read failed: {error}"));
                return false;
            }
        };
        let mut options = self.committed_source.clone().unwrap_or_default();
        if native_query_changed {
            options.native_query = query.native_query.clone();
            options.table = None;
        }
        let revision = self.source_query_coordinator.borrow_mut().request(task);
        self.pending_source = Some(PendingSourceResult {
            revision,
            options,
            cursor_identity,
            mark_identity,
        });
        self.latest_activation_error = None;
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
                let Some(pending) = self
                    .pending_source
                    .take()
                    .filter(|pending| pending.revision == revision)
                else {
                    return false;
                };
                match self.activate_source_replacement(
                    *result,
                    pending.options,
                    pending.cursor_identity,
                    pending.mark_identity,
                    revision,
                ) {
                    Ok(()) => {
                        self.source_status =
                            Some(format!("Source query revision {revision} completed"));
                        true
                    }
                    Err(error) => {
                        self.record_activation_failure(error.to_string());
                        false
                    }
                }
            }
            Some(crate::table::SourceQueryCoordinatorEvent::Failed { revision, error }) => {
                if self
                    .pending_source
                    .as_ref()
                    .is_some_and(|pending| pending.revision == revision)
                {
                    self.pending_source = None;
                    self.record_activation_failure(error);
                }
                false
            }
            None => false,
        }
    }

    fn record_activation_failure(&mut self, error: String) {
        self.source_status = Some(format!(
            "Source query failed; prior result retained: {error}"
        ));
        self.latest_activation_error = Some(error);
    }

    pub fn await_latest_source_query(&mut self) -> anyhow::Result<()> {
        while self.source_query_is_pending() {
            self.poll_source_query();
            if self.source_query_is_pending() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        if let Some(error) = &self.latest_activation_error {
            anyhow::bail!("latest source activation failed: {error}");
        }
        Ok(())
    }

    pub fn reload_committed_source(
        &mut self,
        source: &crate::ingest::source::InputSource,
    ) -> anyhow::Result<()> {
        if source.is_stdin() {
            return Ok(());
        }
        let options = self
            .committed_source
            .clone()
            .ok_or_else(|| anyhow::anyhow!("source configuration is not committed"))?;
        let revision = self.source_query_coordinator.borrow_mut().supersede();
        self.pending_source = None;
        let cursor_row = self.cursor.row;
        let result = (|| {
            let opened =
                crate::ingest::open_source(source.clone(), &options)?.into_implicit_table()?;
            self.activate_opened_candidate(
                opened,
                options,
                CandidateRestoration::Reload { cursor_row },
                revision,
            )
        })();
        if let Err(error) = &result {
            self.record_activation_failure(error.to_string());
        }
        result
    }

    fn activate_source_replacement(
        &mut self,
        replacement: crate::table::SourceResult,
        options: crate::ingest::OpenOptions,
        cursor_identity: Option<crate::table::StableRowIdentity>,
        mark_identity: Option<crate::table::StableRowIdentity>,
        revision: u64,
    ) -> anyhow::Result<()> {
        let opened = crate::ingest::OpenedTable {
            generation: replacement.definition.generation,
            definition: replacement.definition,
            store: replacement.store,
            object_mode: self.object_mode,
            warnings: replacement.metadata.warnings,
        };
        self.activate_opened_candidate(
            opened,
            options,
            CandidateRestoration::Stable {
                cursor_identity,
                mark_identity,
            },
            revision,
        )
    }

    fn activate_opened_candidate(
        &mut self,
        opened: crate::ingest::OpenedTable,
        options: crate::ingest::OpenOptions,
        restoration: CandidateRestoration,
        revision: u64,
    ) -> anyhow::Result<()> {
        let mut next = Self::from_opened_table(opened, self.viewport)?;
        next.restore_view_settings_from(self)?;
        if matches!(restoration, CandidateRestoration::Stable { .. }) {
            if let (Some((_, old_column)), Some(old_definition), Some(new_definition)) = (
                self.mark_identity,
                self.table_definition.as_ref(),
                next.table_definition.as_ref(),
            ) {
                if let Some(old) = old_definition
                    .columns
                    .iter()
                    .find(|column| column.id == old_column)
                {
                    let matches = new_definition
                        .columns
                        .iter()
                        .enumerate()
                        .filter(|(_, candidate)| compatible_source_column(old, candidate))
                        .collect::<Vec<_>>();
                    if matches.len() == 1
                        && old_definition
                            .columns
                            .iter()
                            .filter(|candidate| compatible_source_column(candidate, matches[0].1))
                            .count()
                            == 1
                    {
                        let (source_column, new_column) = matches[0];
                        if let Some(visible_column) = next.visible_column_for_source(source_column)
                        {
                            next.mark_identity =
                                next.row_ids.first().map(|id| (*id, new_column.id));
                            next.mark = next.mark_identity.map(|_| Position {
                                row: 0,
                                column: visible_column,
                            });
                        }
                    }
                }
            }
        }
        match restoration {
            CandidateRestoration::Stable {
                cursor_identity,
                mark_identity,
            } => next.restore_stable_identities(cursor_identity, mark_identity)?,
            CandidateRestoration::Reload { cursor_row } => {
                next.goto(cursor_row, next.cursor.column);
            }
        }
        next.committed_source = Some(next.resolve_source_configuration(options)?);
        if self.source_query_coordinator.borrow().latest_revision() != revision {
            anyhow::bail!("source replacement was superseded during preparation");
        }
        next.source_query_coordinator = self.source_query_coordinator.clone();
        next.latest_activation_error = None;
        *self = next;
        Ok(())
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
        if self.rows.len() != previous_len {
            self.refresh_resident_color_facts_on_append();
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
        let pending = self.pending_projection_schema.borrow_mut().take();
        if let Some((generation, replay)) = pending {
            if self.source_generation() == Some(generation) {
                self.apply_source_schema_delta(replay)?;
            }
        }
        if delta.is_empty() {
            #[cfg(feature = "saved-views")]
            if delta.completed {
                self.advance_saved_binding(false);
                self.advance_saved_binding(true);
            }
            return Ok(());
        }
        #[cfg(feature = "saved-views")]
        let completed = delta.completed;
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
        if let Some(header) = self.header.as_mut() {
            for (index, override_label) in self.column_label_overrides.iter().enumerate() {
                if let (Some(label), Some(name)) = (override_label, header.get_mut(index)) {
                    name.clone_from(label);
                }
            }
        }
        for row in &mut self.rows {
            row.resize(new_count, String::new());
        }
        self.column_alignment_overrides.resize(new_count, None);
        self.column_label_overrides.resize(new_count, None);
        self.column_nulls.resize(new_count, None);
        self.column_display
            .resize(new_count, ColumnDisplayMetadata::default());
        self.column_color_rules.resize(new_count, Vec::new());
        self.compiled_column_colors.resize(new_count, None);
        self.invalidate_color_source_facts();
        #[cfg(feature = "saved-views")]
        self.saved_column_widths.resize(new_count, None);
        self.columns = Columns::infer(self.header.as_deref(), &self.rows);
        self.computed_column_widths_cache.clear();
        #[cfg(feature = "saved-views")]
        {
            self.advance_saved_binding(false);
            if completed {
                self.advance_saved_binding(true);
            }
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
    ) -> VisibleCellStyleContext {
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
    ) -> VisibleCellStyleContext {
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
            conditional_color: self.conditional_foreground_for_source_cell(
                source_column,
                raw,
                rendered,
            ),
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

    fn conditional_foreground_for_source_cell(
        &self,
        source_column: usize,
        raw: &str,
        rendered: &str,
    ) -> Option<ratatui::style::Color> {
        self.compiled_column_colors
            .get(source_column)
            .and_then(Option::as_ref)
            .and_then(|compiled| compiled.evaluate(raw, rendered))
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
        let metadata = self
            .column_display
            .get(source_column)
            .copied()
            .unwrap_or_default();
        let source_type = self
            .table_definition
            .as_ref()
            .and_then(|definition| definition.columns.get(source_column))
            .map_or(crate::table::LogicalType::Unknown, |column| {
                column.source_type
            });
        render::type_sort_mode(
            metadata,
            source_type,
            self.columns.is_numeric(ColumnIndex::new(source_column)),
        )
    }

    pub fn sort_current_column(&mut self, mode: SortMode, direction: SortDirection) {
        let Some(column) = self.source_column_for_visible(self.cursor.column) else {
            return;
        };
        #[cfg(feature = "saved-views")]
        if let Some(binding) = self.saved_binding.as_mut() {
            binding.supersede_sorts();
        }
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
        #[cfg(feature = "saved-views")]
        if let Some(binding) = self.saved_binding.as_mut() {
            binding.supersede_sorts();
        }
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
        match self.refresh_view_transform() {
            QueryRefresh::Applied => {
                self.invalidate_color_source_facts();
                return true;
            }
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
            &|column, value| {
                let index = column.ordinal as usize;
                let metadata = self.column_display.get(index).copied().unwrap_or_default();
                let raw = value.display();
                if metadata.format == DisplayFormatMetadata::Plain {
                    raw.into_owned()
                } else {
                    render::cell(
                        metadata,
                        numeric_profiles.get(index).copied().unwrap_or_default(),
                        &raw,
                    )
                }
            },
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

    fn current_stable_row_identity(
        &mut self,
    ) -> anyhow::Result<Option<crate::table::StableRowIdentity>> {
        let Some(index) = self
            .visible_rows
            .get(self.cursor.row)
            .and_then(|row| self.row_ids.get(*row))
            .map(|id| id.ordinal as usize)
        else {
            return Ok(None);
        };
        match &self.incremental_store {
            Some(store) => store.0.borrow_mut().stable_row_identity(RowIndex(index)),
            None => Ok(None),
        }
    }

    fn mark_stable_row_identity(
        &mut self,
    ) -> anyhow::Result<Option<crate::table::StableRowIdentity>> {
        let Some(index) = self.mark_identity.map(|(id, _)| id.ordinal as usize) else {
            return Ok(None);
        };
        match &self.incremental_store {
            Some(store) => store.0.borrow_mut().stable_row_identity(RowIndex(index)),
            None => Ok(None),
        }
    }

    fn restore_stable_identities(
        &mut self,
        cursor_identity: Option<crate::table::StableRowIdentity>,
        mark_identity: Option<crate::table::StableRowIdentity>,
    ) -> anyhow::Result<()> {
        if cursor_identity.is_none() && mark_identity.is_none() {
            self.cursor.row = 0;
            self.mark = None;
            self.mark_identity = None;
            return Ok(());
        }
        let mut cursor_match = None;
        let mut mark_match = None;
        let mut cursor_count = 0;
        let mut mark_count = 0;
        let transformed = self.view_transform_is_active();
        let previous_len = self.rows.len();
        let visible_positions = transformed.then(|| {
            self.row_ids
                .iter()
                .copied()
                .enumerate()
                .map(|(position, id)| (id, position))
                .collect::<std::collections::HashMap<_, _>>()
        });
        if let Some(store) = &self.incremental_store {
            let mut store = store.0.borrow_mut();
            let mut index = 0;
            loop {
                let Some(source_row) = store.row(RowIndex(index))? else {
                    break;
                };
                let identity = store.stable_row_identity(RowIndex(index))?;
                if cursor_identity
                    .as_ref()
                    .is_some_and(|wanted| identity.as_ref() == Some(wanted))
                {
                    cursor_count += 1;
                    cursor_match = if transformed {
                        visible_positions
                            .as_ref()
                            .and_then(|positions| positions.get(&source_row.id).copied())
                    } else {
                        Some(index)
                    };
                }
                if mark_identity
                    .as_ref()
                    .is_some_and(|wanted| identity.as_ref() == Some(wanted))
                {
                    mark_count += 1;
                    mark_match = if transformed {
                        visible_positions
                            .as_ref()
                            .and_then(|positions| positions.get(&source_row.id).copied())
                    } else {
                        Some(index)
                    };
                }
                if !transformed && index >= self.row_ids.len() {
                    self.row_ids.push(source_row.id);
                    let mut cells = source_row.display_cells();
                    cells.resize(self.source_column_count(), String::new());
                    self.rows.push(cells);
                }
                index += 1;
            }
            if !transformed {
                self.visible_rows = (0..self.rows.len()).collect();
            }
        }
        if !transformed && self.rows.len() != previous_len {
            self.refresh_resident_color_facts_on_append();
        }
        self.cursor.row = if cursor_count == 1 {
            cursor_match.unwrap_or(0)
        } else {
            0
        };
        if mark_count == 1 {
            if let (Some(row), Some((_, column_id))) = (mark_match, self.mark_identity) {
                self.mark_identity = Some((self.row_ids[row], column_id));
                if let Some(mark) = &mut self.mark {
                    mark.row = row;
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
        Ok(())
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
        #[cfg(feature = "saved-views")]
        if let Some(binding) = self.saved_binding.as_mut() {
            binding.supersede_filters_for_column(
                source_column,
                self.table_definition.as_ref(),
                self.resident_source_header
                    .as_deref()
                    .or(self.header.as_deref())
                    .unwrap_or(&[]),
            );
        }
        self.computed_column_widths_cache.clear();
        self.apply_query_configuration();
        Ok(())
    }

    pub fn clear_filters_for_column(&mut self, column: usize) {
        if let Some(source_column) = self.source_column_for_visible(column) {
            #[cfg(feature = "saved-views")]
            if let Some(binding) = self.saved_binding.as_mut() {
                binding.supersede_filters_for_column(
                    source_column,
                    self.table_definition.as_ref(),
                    self.resident_source_header
                        .as_deref()
                        .or(self.header.as_deref())
                        .unwrap_or(&[]),
                );
            }
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
        #[cfg(feature = "saved-views")]
        if let Some(binding) = self.saved_binding.as_mut() {
            binding.supersede_column(source_column);
            binding.supersede_sorts();
            if update.clear_filters {
                binding.supersede_filters_for_column(
                    source_column,
                    self.table_definition.as_ref(),
                    self.resident_source_header
                        .as_deref()
                        .or(self.header.as_deref())
                        .unwrap_or(&[]),
                );
            }
        }

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
            display.type_explicit = true;
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
        self.invalidate_conditional_colors();

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

    pub fn restore_view_settings_from(&mut self, previous: &TableView) -> anyhow::Result<()> {
        let source_column_count = self.source_column_count();
        let previous_column_count = previous.source_column_count();
        let remap = (0..previous_column_count)
            .map(|old_index| {
                match (
                    previous.table_definition.as_ref(),
                    self.table_definition.as_ref(),
                ) {
                    (Some(old), Some(new)) => {
                        let old_column = old.columns.get(old_index)?;
                        let matches = new
                            .columns
                            .iter()
                            .enumerate()
                            .filter(|(_, column)| compatible_source_column(old_column, column))
                            .collect::<Vec<_>>();
                        let (_, candidate) = matches.first().copied()?;
                        let unique_old = old
                            .columns
                            .iter()
                            .filter(|column| compatible_source_column(column, candidate))
                            .count()
                            == 1;
                        (matches.len() == 1 && unique_old).then_some(matches[0].0)
                    }
                    (None, None) => (old_index < source_column_count).then_some(old_index),
                    _ => None,
                }
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
        #[cfg(feature = "saved-views")]
        {
            // Resident views lack a typed definition: keep the new result's raw
            // header before restored presentation labels replace it.
            self.resident_source_header = (self.table_definition.is_none()
                && previous.saved_binding.is_some()
                && previous.column_label_overrides.iter().any(Option::is_some))
            .then(|| self.header.clone())
            .flatten();
        }
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
            if let Some(label) = self.column_label_overrides[new_index].as_ref() {
                if let Some(header) = self
                    .header
                    .as_mut()
                    .and_then(|header| header.get_mut(new_index))
                {
                    header.clone_from(label);
                }
            }
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
        self.compiled_column_colors = vec![None; source_column_count];
        self.invalidate_color_source_facts();
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
            self.saved_binding = previous.saved_binding.clone();
            if let Some(binding) = self.saved_binding.as_mut() {
                binding.remap_columns(&remap);
            }
            self.saved_binding_warnings = previous.saved_binding_warnings.clone();
            self.saved_column_widths = vec![None; source_column_count];
            for (old_index, width) in previous.saved_column_widths.iter().copied().enumerate() {
                if let Some(new_index) = remap.get(old_index).copied().flatten() {
                    self.saved_column_widths[new_index] = width;
                }
            }
        }
        if !self.apply_query_configuration() {
            anyhow::bail!(
                "cannot reconstruct local view: {}",
                self.source_status
                    .as_deref()
                    .unwrap_or("source transform failed")
            );
        }
        #[cfg(feature = "saved-views")]
        self.advance_saved_binding(self.table_definition.as_ref().is_none_or(|definition| {
            definition.schema_state == crate::table::SchemaState::Complete
        }));
        if let Some(new_source_column) = previous_cursor_source
            .and_then(|old| remap.get(old).copied().flatten())
            .and_then(|source| self.visible_column_for_source(source))
        {
            self.cursor.column = new_source_column;
        }
        Ok(())
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
        resolved: &[crate::saved_views::ResolvedColumnView],
        locale: Option<&str>,
    ) {
        if self.table_definition.is_none()
            && self.resident_source_header.is_none()
            && resolved.iter().any(|column| column.view.label.is_some())
        {
            self.resident_source_header = self.header.clone();
        }
        self.saved_column_widths
            .resize(self.source_column_count(), None);
        for resolved_column in resolved {
            let source_column = resolved_column.column_index;
            let Some(label) = resolved_column.view.label.as_ref() else {
                continue;
            };
            if let Some(header) = self.header.as_mut() {
                if let Some(name) = header.get_mut(source_column) {
                    *name = label.clone();
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

        for resolved_column in resolved {
            let source_column = resolved_column.column_index;
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
                display.type_explicit = column_view.column_type.is_some();
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
            if let Some(slot) = self.column_color_rules.get_mut(source_column) {
                *slot = colors;
                if !slot.is_empty() {
                    self.column_metadata_modified.insert(source_column);
                }
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
        self.invalidate_conditional_colors();
        self.keep_cursor_visible();
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

    /// Install the invocation's saved presentation once. The binding owner alone
    /// interprets references on this and all later schema/profile updates.
    #[cfg(feature = "saved-views")]
    pub fn install_saved_binding(
        &mut self,
        config: crate::saved_views::SavedViewConfig,
        sorts_enabled: bool,
    ) {
        let binding = crate::saved_views::binding::SavedViewBinding::new(config, sorts_enabled);
        self.set_view_null_placement(binding.nulls());
        self.saved_binding = Some(binding);
        self.advance_saved_binding(false);
        if self
            .table_definition
            .as_ref()
            .is_none_or(|definition| definition.schema_state == crate::table::SchemaState::Complete)
        {
            self.advance_saved_binding(true);
        }
    }

    #[cfg(feature = "saved-views")]
    fn saved_binding_headers(&self) -> &[String] {
        self.resident_source_header
            .as_deref()
            .or(self.header.as_deref())
            .unwrap_or(&[])
    }

    #[cfg(feature = "saved-views")]
    pub fn take_saved_binding_warnings(&mut self) -> Vec<crate::saved_views::SavedViewWarning> {
        std::mem::take(&mut self.saved_binding_warnings)
    }

    #[cfg(feature = "saved-views")]
    fn advance_saved_binding(&mut self, profile_definitive: bool) {
        use crate::saved_views::binding::FilterBindingOutcome;

        let Some(mut binding) = self.saved_binding.take() else {
            return;
        };
        let complete = self.table_definition.as_ref().is_none_or(|definition| {
            definition.schema_state == crate::table::SchemaState::Complete
        });
        let update = binding.advance(
            self.table_definition.as_ref(),
            self.saved_binding_headers(),
            complete,
        );
        if !update.columns.is_empty() {
            self.apply_saved_columns(&update.columns, binding.locale());
        }
        self.saved_binding_warnings.extend(update.warnings);

        if let Some(sorts) = update.sorts {
            let keys = sorts
                .into_iter()
                .map(|sort| {
                    sort.to_active(
                        self.type_sort_mode_for_source(sort.column),
                        self.resolved_null_placement(sort.column),
                    )
                })
                .collect();
            self.apply_saved_sort_keys(keys);
        }

        for filter in update.filters {
            let index = filter.index;
            let column = filter.column;
            let typed_numeric = self
                .table_definition
                .as_ref()
                .and_then(|definition| definition.columns.get(column))
                .is_some_and(|source| {
                    matches!(
                        source.source_type,
                        crate::table::LogicalType::Integer | crate::table::LogicalType::Float
                    )
                });
            let numeric_available =
                typed_numeric || self.columns.is_numeric(ColumnIndex::new(column));
            let outcome = match filter.parse_active(
                self.source_numeric_column_profile(column),
                numeric_available,
            ) {
                Ok(active) => {
                    self.filters.push(active);
                    self.computed_column_widths_cache.clear();
                    self.apply_query_configuration();
                    FilterBindingOutcome::Installed
                }
                Err(outcome) => outcome,
            };
            if let Some(warning) = binding.filter_feedback(index, outcome, profile_definitive) {
                self.saved_binding_warnings.push(warning);
            }
        }
        self.saved_binding = Some(binding);
    }

    #[cfg(feature = "saved-views")]
    pub fn to_saved_view_yaml(
        &self,
        name: &str,
        input_filename: &str,
        locale: Option<&str>,
    ) -> String {
        let mut yaml = String::new();
        yaml.push_str(&format!("name: {}\n", yaml_scalar(name)));
        yaml.push_str("filenames:\n");
        yaml.push_str(&format!("  - {}\n", yaml_scalar(input_filename)));

        let mut source_yaml = String::new();
        if let Some(options) = self.committed_source.as_ref() {
            if options.format != crate::ingest::InputFormat::Auto {
                source_yaml.push_str(&format!("format: {}\n", options.format));
            } else if let Some(provenance) = &self.source_query_provenance {
                source_yaml.push_str(&format!(
                    "format: {}\n",
                    match provenance.language {
                        crate::table::NativeQueryLanguage::Sql => "sqlite",
                        crate::table::NativeQueryLanguage::Esql => "elasticsearch",
                    }
                ));
            }
            if let Some(json_path) = &options.json_path {
                source_yaml.push_str(&format!("json_path: {}\n", yaml_scalar(json_path.as_str())));
            }
            if options.schema_scan == crate::ingest::SchemaScan::Full {
                source_yaml.push_str("schema_scan: full\n");
            }
            if let Some(query) = &options.native_query {
                source_yaml.push_str(&format!("query: {}\n", yaml_scalar(query)));
            } else if let Some(table) = &options.table {
                source_yaml.push_str(&format!("table: {}\n", yaml_scalar(table)));
            }
            if let Some(limit) = options.limit {
                source_yaml.push_str(&format!("limit: {}\n", limit));
            }
            if !options.source_filters.is_empty() {
                source_yaml.push_str("filters:\n");
                for filter in &options.source_filters {
                    source_yaml.push_str(&format!("  - column: {}\n", yaml_scalar(&filter.column)));
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
            if !options.source_sort.is_empty() {
                source_yaml.push_str("sort:\n");
                for sort in &options.source_sort {
                    source_yaml.push_str(&format!("  - column: {}\n", yaml_scalar(&sort.column)));
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
        if let Some(object_mode) = self.object_mode {
            source_yaml.push_str(&format!("object_mode: {}\n", object_mode.resolved));
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
        render::cell(
            self.column_display
                .get(source_column)
                .copied()
                .unwrap_or_default(),
            self.source_numeric_column_profile(source_column),
            raw.unwrap_or_default(),
        )
    }

    fn invalidate_conditional_colors(&mut self) {
        self.color_cache_valid = false;
    }

    fn invalidate_color_source_facts(&mut self) {
        self.exact_color_profiles = None;
        self.invalidate_conditional_colors();
    }

    fn refresh_resident_color_facts_on_append(&mut self) {
        if self.color_cache_valid
            && self
                .column_color_rules
                .iter()
                .enumerate()
                .any(|(column, rules)| {
                    let demand = ColorProfileDemand::for_rules(rules);
                    let exact = self
                        .exact_color_profiles
                        .as_ref()
                        .and_then(|profiles| profiles.get(column));
                    (demand.extrema && exact.and_then(|profile| profile.numeric_min_max).is_none())
                        || (demand.identifiers && exact.is_none())
                })
        {
            self.invalidate_conditional_colors();
        }
    }

    /// Prepare only when a renderer demands conditional foregrounds. Binding and
    /// plain output retain configured rules without inspecting a color-only profile.
    pub(crate) fn prepare_conditional_colors(&mut self, theme: &ResolvedTheme) {
        if self.color_cache_valid {
            debug_assert!(
                self.compiled_column_colors
                    .iter()
                    .flatten()
                    .all(|compiled| compiled.scope() == ColorProfileScope::CompleteResult),
                "live rendering cannot reuse an emitted-preview profile"
            );
            return;
        }
        let needs_profile = self
            .column_color_rules
            .iter()
            .any(|rules| ColorProfileDemand::for_rules(rules).needs_profile());
        if needs_profile && self.exact_color_profiles.is_none() {
            self.exact_color_profiles = self.exact_reduction_profiles().ok().flatten();
        }
        self.compiled_column_colors = (0..self.source_column_count())
            .map(|source_column| {
                let rules = self.column_color_rules.get(source_column)?;
                if rules.is_empty() {
                    return None;
                }
                let exact_profile = self
                    .exact_color_profiles
                    .as_ref()
                    .and_then(|profiles| profiles.get(source_column));
                let demand = ColorProfileDemand::for_rules(rules);
                let numeric_min_max = if demand.extrema {
                    exact_profile
                        .and_then(|profile| profile.numeric_min_max)
                        .or_else(|| self.numeric_min_max(source_column))
                } else {
                    None
                };
                let identifier_indexes = if demand.identifiers {
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
                Some(CompiledColumnColors::prepare(
                    theme,
                    rules,
                    ColumnColorProfile {
                        scope: ColorProfileScope::CompleteResult,
                        numeric_profile: self.source_numeric_column_profile(source_column),
                        numeric_min_max,
                        identifier_indexes,
                    },
                ))
            })
            .collect();
        self.color_cache_valid = true;
    }

    fn exact_reduction_profiles(
        &mut self,
    ) -> anyhow::Result<Option<Vec<crate::table::ColumnReductionProfile>>> {
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
mod lifecycle_tests;

#[cfg(all(test, feature = "saved-views"))]
mod color_profile_tests;

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

    #[cfg(feature = "saved-views")]
    fn color_at(view: &mut TableView, row: usize, column: usize) -> Option<ratatui::style::Color> {
        view.prepare_conditional_colors(&crate::theme::default_theme());
        let rendered = view.rendered_visible_row(row)?;
        let source_column = view.source_column_for_visible(column)?;
        view.source_cell_style_context(row, source_column, rendered.get(column)?, None)
            .conditional_color
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
    fn failed_projection_preserves_discovered_schema_for_live_indexing() {
        use std::sync::atomic::{AtomicU8, Ordering};
        use std::sync::Arc;

        struct SchemaThenError {
            inner: InMemoryTable,
            late: ColumnDefinition,
            phase: Arc<AtomicU8>,
        }

        impl TableStore for SchemaThenError {
            fn generation(&self) -> SourceGeneration {
                self.inner.generation()
            }

            fn row_count(&self) -> RowCount {
                self.inner.row_count()
            }

            fn column_count(&self) -> usize {
                if self.phase.load(Ordering::Relaxed) >= 2 {
                    2
                } else {
                    1
                }
            }

            fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
                self.inner.row(index)
            }

            fn ensure_indexed_through(&mut self, index: RowIndex) -> anyhow::Result<IndexProgress> {
                if self.phase.load(Ordering::Relaxed) == 2 && index.0 == usize::MAX {
                    self.phase.store(3, Ordering::Relaxed);
                    anyhow::bail!("injected read failure after schema discovery");
                }
                let mut progress = self.inner.ensure_indexed_through(index)?;
                if self.phase.load(Ordering::Relaxed) == 1 {
                    self.phase.store(2, Ordering::Relaxed);
                    progress.schema_delta.added_columns.push(self.late.clone());
                }
                Ok(progress)
            }

            fn scan_rows(
                &mut self,
                request: ScanRequest,
                visitor: &mut dyn crate::table::RowVisitor,
            ) -> anyhow::Result<ScanProgress> {
                self.inner.scan_rows(request, visitor)
            }

            fn materialize(&mut self) -> anyhow::Result<InMemoryTable> {
                self.inner.materialize()
            }
        }

        let mut opened = query_test_table(&["alpha"], false);
        let generation = opened.generation;
        let phase = Arc::new(AtomicU8::new(0));
        let mut late = opened.definition.columns[0].clone();
        late.id.ordinal = 1;
        late.source_identity = ColumnSourceIdentity::Positional(1);
        late.display_name = "late".to_owned();
        late.source_type = LogicalType::Integer;
        opened.store = Box::new(SchemaThenError {
            inner: InMemoryTable::from_rows(
                generation,
                vec![Row::new(
                    RowId {
                        generation,
                        ordinal: 0,
                    },
                    vec![CellValue::Text("alpha".to_owned()), CellValue::Integer(7)],
                )],
            )
            .unwrap(),
            late,
            phase: phase.clone(),
        });
        let mut view = TableView::from_opened_table(opened, Viewport::new(1, 1)).unwrap();
        phase.store(1, Ordering::Relaxed);

        let error = view
            .prepare_projection(
                crate::output::requirements(
                    crate::output::OutputFormat::Json,
                    crate::output::ColorOutput::Never,
                )
                .unwrap(),
                &crate::theme::default_theme(),
            )
            .err()
            .expect("projection must report the read failure");
        assert!(error.to_string().contains("injected read failure"));

        view.ensure_source_indexed_through(0).unwrap();
        assert_eq!(view.header().unwrap(), ["value", "late"]);
        assert_eq!(view.rendered_visible_row(0).unwrap(), ["alpha", "7"]);
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
        reloaded
            .restore_view_settings_from(&previous)
            .expect("compatible view");

        assert_ne!(reloaded.source_generation(), old_generation);
        assert_eq!(reloaded.cursor().column, 0, "cursor follows canonical /b");
        assert_eq!(reloaded.column_widths[0], 17, "width follows canonical /b");
        assert!(
            reloaded.mark().is_none(),
            "row identity is generation-scoped"
        );
    }

    #[test]
    fn unrelated_structured_column_does_not_inherit_hidden_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fields.json");
        std::fs::write(&path, r#"[{"old":1,"stable":2}]"#).expect("source");
        let options = crate::ingest::OpenOptions {
            format: crate::ingest::InputFormat::Json,
            ..crate::ingest::OpenOptions::default()
        };
        let source = crate::ingest::source::InputSource::Path(path.clone());
        let opened = crate::ingest::open_source(source.clone(), &options)
            .expect("open")
            .into_implicit_table()
            .expect("table");
        let mut previous = TableView::from_opened_table(opened, Viewport::new(5, 2)).expect("view");
        previous.goto(0, 0);
        previous.hide_current_column();
        previous.set_current_column_width(17);

        std::fs::write(&path, r#"[{"new":3,"stable":2}]"#).expect("replacement");
        let opened = crate::ingest::open_source(source, &options)
            .expect("reopen")
            .into_implicit_table()
            .expect("table");
        let mut replacement =
            TableView::from_opened_table(opened, Viewport::new(5, 2)).expect("replacement view");
        replacement
            .restore_view_settings_from(&previous)
            .expect("compatible state");
        assert_eq!(replacement.visible_rows_vec(), rows(&[&["3", "2"]]));
        assert_eq!(replacement.column_count(), 2);
        assert_ne!(replacement.effective_column_widths()[0], 17);
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
        view.install_saved_binding(parsed.view.view, true);

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
        view.install_saved_binding(parsed.view.view, true);

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
        let mut config = parsed.view.view;
        config.locale = Some("en_US".to_owned());
        view.install_saved_binding(config, true);

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
        view.install_saved_binding(parsed.view.view, true);

        assert_eq!(
            color_at(&mut view, 0, 0),
            Some(ratatui::style::Color::Rgb(0, 192, 0))
        );
        assert_eq!(
            color_at(&mut view, 0, 1),
            Some(ratatui::style::Color::Rgb(255, 0, 0))
        );
        assert_eq!(
            color_at(&mut view, 1, 1),
            Some(ratatui::style::Color::Rgb(146, 228, 0))
        );
        assert_eq!(
            view.visible_rows_vec(),
            rows(&[&["active", "5%"], &["idle", "50%"], &["down", "95%"]])
        );
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn color_demand_profiles_only_dependent_rules_and_uses_offscreen_typed_extrema() {
        let mut opened = query_test_table(&["50", "1", "100"], false);
        let generation = opened.generation;
        opened.definition.columns[0].source_type = LogicalType::Integer;
        opened.store = Box::new(QueryTestStore {
            generation,
            rows: [50, 1, 100]
                .into_iter()
                .enumerate()
                .map(|(ordinal, value)| {
                    Row::new(
                        RowId {
                            generation,
                            ordinal: ordinal as u64,
                        },
                        vec![CellValue::Integer(value)],
                    )
                })
                .collect(),
            indexed: 0,
            fail_materialize: false,
        });
        let mut view = TableView::from_opened_table(opened, Viewport::new(1, 1)).expect("view");
        let fixed = crate::saved_views::parse_saved_view_yaml(
            "name: fixed\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    value:\n      colors:\n        - match:\n            '50': green\n",
        )
        .expect("fixed saved view");
        view.install_saved_binding(fixed.view.view, true);
        view.take_source_status();
        view.prepare_conditional_colors(&crate::theme::default_theme());
        assert!(
            view.take_source_status().is_none(),
            "match rules need no exact color profile"
        );
        assert_eq!(view.rows.len(), 1);
        let gradient = crate::saved_views::parse_saved_view_yaml(
            "name: gradient\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    value:\n      colors:\n        - gradient:\n            mode: auto\n            steps: 4\n            colors: [black, white]\n",
        )
        .expect("gradient saved view");
        view.install_saved_binding(gradient.view.view, true);
        view.take_source_status();
        assert_eq!(
            view.rows.len(),
            1,
            "binding cannot profile for color demand"
        );
        assert_eq!(
            color_at(&mut view, 0, 0),
            Some(ratatui::style::Color::Rgb(85, 85, 85))
        );
        assert_eq!(
            view.take_source_status().as_deref(),
            Some("Profiled 3 rows")
        );
        assert_eq!(
            view.rows.len(),
            1,
            "exact color profile does not widen resident viewport"
        );
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn exact_store_identifier_keys_keep_raw_display_misses_without_rendered_fallback() {
        let saved = crate::saved_views::parse_saved_view_yaml(
            "name: identifiers\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    value:\n      type: text\n      format: uppercase\n      colors:\n        - identifiers:\n            colors: [green, red]\n",
        )
        .expect("saved view");
        let mut store = TableView::from_opened_table(
            query_test_table(&["alpha", "beta"], false),
            Viewport::new(1, 1),
        )
        .expect("store view");
        store.install_saved_binding(saved.view.view.clone(), true);
        assert_eq!(
            store.rendered_visible_row(0),
            Some(vec!["ALPHA".to_owned()])
        );
        assert_eq!(
            color_at(&mut store, 0, 0),
            None,
            "raw store key does not match rendered uppercase text"
        );
        assert_eq!(
            store.take_source_status().as_deref(),
            Some("Profiled 2 rows")
        );

        let mut resident = TableView::classify(
            rows(&[&["value"], &["alpha"], &["ALPHA"], &["beta"]]),
            Viewport::new(1, 1),
        );
        resident.install_saved_binding(saved.view.view, true);
        let first = color_at(&mut resident, 0, 0);
        assert!(first.is_some(), "resident keys use rendered values");
        assert_eq!(
            color_at(&mut resident, 1, 0),
            first,
            "equal rendered identifiers share a key"
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
        view.install_saved_binding(parsed.view.view, true);

        let yaml = view.to_saved_view_yaml("colors", "data.csv", None);

        assert!(yaml.contains("          \"10\": green"));
        assert_eq!(
            conditional_value_yaml(&ConditionalValue::String("a\nb".to_owned())),
            "\"a\\nb\""
        );
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn rendering_keeps_original_conditional_yaml_values_for_save_and_reopen() {
        let original = crate::saved_views::parse_saved_view_yaml(
            r##"
name: configured
filenames: ["*"]
source: {}
view:
  columns:
    Code:
      colors:
        - match:
            "10": "a,b:c(β)"
            10: green
    Family:
      colors:
        - identifiers:
            colors: ["a,b:c(β)", "#25A39AFF"]
    Scale:
      colors:
        - gradient:
            mode: auto
            steps: 3
            colors: ["a,b:c(β)", green]
"##,
        )
        .expect("original saved view");
        let mut view = TableView::classify(
            rows(&[
                &["Code", "Family", "Scale"],
                &["10", "alpha", "5"],
                &["20", "beta", "10"],
            ]),
            Viewport::new(10, 3),
        );
        view.install_saved_binding(original.view.view.clone(), true);
        let fixture = include_str!("../../examples/data/config/themes/cmdzro.yml")
            .replace("  hex_teal:", "  \"a,b:c(β)\": \"#25A39AFF\"\n  hex_teal:");
        let theme =
            crate::theme::parse_theme_yaml(&fixture, crate::theme::TerminalColorMode::TrueColor)
                .expect("selected theme");
        view.prepare_conditional_colors(&theme);
        let rendered = view.rendered_visible_row(0).expect("rendered row");
        let foregrounds = rendered
            .iter()
            .enumerate()
            .map(|(column, cell)| {
                let source_column = view
                    .source_column_for_visible(column)
                    .expect("source column");
                view.source_cell_style_context(0, source_column, cell, None)
                    .conditional_color
            })
            .collect::<Vec<_>>();
        assert_eq!(
            foregrounds,
            vec![
                Some(ratatui::style::Color::Rgb(37, 163, 154)),
                Some(ratatui::style::Color::Rgb(19, 100, 100)),
                Some(ratatui::style::Color::Rgb(37, 163, 154)),
            ]
        );

        let saved = view.to_saved_view_yaml("configured", "data.csv", None);
        let reopened = crate::saved_views::parse_saved_view_yaml(&saved).expect("reopened view");
        for (name, configured) in &original.view.view.columns {
            assert_eq!(
                reopened.view.view.columns[name].colors, configured.colors,
                "configured conditional rules must survive rendering and saving"
            );
        }
        let code_rules = &reopened.view.view.columns["Code"].colors;
        assert!(
            matches!(&code_rules[0], ConditionalColorRule::Match { entries }
            if matches!((&entries[0].value, &entries[1].value),
                (ConditionalValue::String(string), ConditionalValue::Number(number))
                if string == "10" && *number == 10.0))
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
        let mut view = view;
        view.initialize_source_configuration(options)
            .expect("committed source");
        let yaml = view.to_saved_view_yaml("repositories", "repositories.json", None);
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
        let mut record_view =
            TableView::from_opened_table(record, Viewport::new(8, 80)).expect("record view");
        record_view
            .initialize_source_configuration(record_options)
            .expect("committed source");
        assert!(record_view
            .to_saved_view_yaml("record", "repositories.json", None)
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
    fn column_info_format_changes_keep_identifier_foreground_on_rendered_value() {
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
        view.install_saved_binding(parsed.view.view, true);

        assert_eq!(view.visible_rows_vec(), rows(&[&["ALPHA"]]));
        let first = color_at(&mut view, 0, 0);
        assert!(first.is_some());

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
        assert_eq!(color_at(&mut view, 0, 0), first);
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
        view.install_saved_binding(parsed.view.view, true);

        let first = color_at(&mut view, 0, 0).expect("first foreground");
        let second = color_at(&mut view, 1, 0).expect("second foreground");
        let repeated = color_at(&mut view, 2, 0).expect("repeated foreground");
        assert_ne!(first, second);
        assert_eq!(first, repeated);
        view.sort_current_column(SortMode::Lexical, SortDirection::Descending);
        assert_eq!(
            view.visible_raw_rows_vec(),
            rows(&[&["10.0.0.2"], &["10.0.0.2"], &["10.0.0.1"]])
        );
        assert_eq!(color_at(&mut view, 0, 0), Some(first));
        assert_eq!(color_at(&mut view, 1, 0), Some(first));
        assert_eq!(color_at(&mut view, 2, 0), Some(second));
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn restored_view_settings_keep_identifier_foregrounds() {
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
        view.install_saved_binding(parsed.view.view, true);

        let mut restored = TableView::classify(
            rows(&[&["Address"], &["10.0.0.2"], &["10.0.0.1"]]),
            Viewport::new(10, 1),
        );
        restored
            .restore_view_settings_from(&view)
            .expect("compatible view");

        assert_eq!(color_at(&mut restored, 0, 0), color_at(&mut view, 0, 0));
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn resident_source_labels_survive_override_and_restoration_without_aliasing() {
        let saved = crate::saved_views::parse_saved_view_yaml(
            "name: count\nfilenames: []\nsource: {}\nview:\n  columns:\n    count: {label: Total}\n  sort:\n    - {column: count, direction: asc, kind: numeric}\n",
        )
        .expect("saved view");
        let source = rows(&[&["count"], &["10"], &["2"], &["1"]]);
        let mut view = TableView::classify(source.clone(), Viewport::new(10, 1));
        view.install_saved_binding(saved.view.view, true);
        assert_eq!(view.header().expect("header"), ["Total"]);
        assert_eq!(
            view.visible_raw_rows_vec(),
            rows(&[&["1"], &["2"], &["10"]])
        );
        assert!(view.take_saved_binding_warnings().is_empty());

        let mut restored = TableView::classify(source.clone(), Viewport::new(10, 1));
        restored
            .restore_view_settings_from(&view)
            .expect("compatible resident view");
        assert_eq!(restored.header().expect("header"), ["Total"]);
        assert_eq!(
            restored.visible_raw_rows_vec(),
            rows(&[&["1"], &["2"], &["10"]])
        );

        let filter = crate::saved_views::parse_saved_view_yaml(
            "name: source\nfilenames: []\nsource: {}\nview:\n  filters:\n    - {column: count, action: in, kind: numeric, condition: '>2'}\n",
        )
        .expect("source-label filter");
        let mut alias = TableView::classify(source, Viewport::new(10, 1));
        alias
            .restore_view_settings_from(&view)
            .expect("compatible resident view");
        restored.install_saved_binding(filter.view.view, true);
        assert_eq!(restored.visible_raw_rows_vec(), rows(&[&["10"]]));
        assert!(restored.take_saved_binding_warnings().is_empty());

        let filter = crate::saved_views::parse_saved_view_yaml(
            "name: alias\nfilenames: []\nsource: {}\nview:\n  filters:\n    - {column: Total, action: in, kind: numeric, condition: '>2'}\n",
        )
        .expect("alias filter");
        alias.install_saved_binding(filter.view.view, true);
        assert_eq!(
            alias.visible_raw_rows_vec(),
            rows(&[&["1"], &["2"], &["10"]])
        );
        let warnings = alias.take_saved_binding_warnings();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].field, "view.filters[0].column");
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
        date_view.install_saved_binding(parsed.view.view, true);
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
        semver_view.install_saved_binding(parsed.view.view, true);
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
        ip_view.install_saved_binding(parsed.view.view, true);
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
        view.install_saved_binding(saved.view.view, true);

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
        view.install_saved_binding(saved.view.view, true);

        view.goto(1, 0);

        assert_eq!(view.visible_rows_vec(), rows(&[&["2", "3"]]));
        assert_eq!(view.visible_raw_rows_vec(), rows(&[&["2", "3"]]));
    }
    #[test]
    fn failed_frozen_export_keeps_live_source_and_screen_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("broken.ndjson");
        std::fs::write(&path, "{\"id\":1}\ninvalid\n").expect("write");
        let options = crate::ingest::OpenOptions {
            format: crate::ingest::InputFormat::Ndjson,
            lazy_threshold_bytes: 1,
            schema_scan_bytes: 1,
            ..crate::ingest::OpenOptions::default()
        };
        let opened = crate::ingest::JsonAdapter::ndjson()
            .open(crate::ingest::source::InputSource::Path(path), &options)
            .expect("open")
            .into_implicit_table()
            .expect("table");
        let view = TableView::from_opened_table(opened, Viewport::new(1, 1)).expect("view");
        let generation = view.source_generation();
        let cursor = view.cursor();
        let viewport = view.viewport();
        let header = view.header().map(<[String]>::to_vec);
        let rows = view.visible_rows_vec();
        assert!(view
            .prepare_projection(
                crate::output::requirements(
                    crate::output::OutputFormat::Jsonl,
                    crate::output::ColorOutput::Never,
                )
                .expect("requirements"),
                &crate::theme::default_theme(),
            )
            .is_err());
        assert_eq!(view.source_generation(), generation);
        assert_eq!(view.cursor(), cursor);
        assert_eq!(view.viewport(), viewport);
        assert_eq!(view.header().map(<[String]>::to_vec), header);
        assert_eq!(view.visible_rows_vec(), rows);
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn frozen_export_replays_late_schema_on_next_live_progress() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("late.json");
        std::fs::write(&path, r#"[{"id":1},{"id":2,"late":"text"}]"#).expect("write");
        let options = crate::ingest::OpenOptions {
            format: crate::ingest::InputFormat::Json,
            schema_scan_bytes: 1,
            lazy_threshold_bytes: 1,
            ..crate::ingest::OpenOptions::default()
        };
        let opened = crate::ingest::JsonAdapter::json()
            .open(crate::ingest::source::InputSource::Path(path), &options)
            .expect("open")
            .into_implicit_table()
            .expect("table");
        let mut view = TableView::from_opened_table(opened, Viewport::new(1, 1)).expect("view");
        assert_eq!(view.header(), Some(&["id".to_owned()][..]));
        let saved = crate::saved_views::parse_saved_view_yaml(
            "name: late presentation\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    /late: {format: uppercase}\n",
        )
        .expect("saved");
        view.install_saved_binding(saved.view.view, true);
        let cursor = view.cursor();
        let viewport = view.viewport();
        let frozen = view
            .prepare_projection(
                crate::output::requirements(
                    crate::output::OutputFormat::Jsonl,
                    crate::output::ColorOutput::Never,
                )
                .expect("requirements"),
                &crate::theme::default_theme(),
            )
            .expect("frozen");
        assert_eq!(
            frozen
                .output
                .header
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<Vec<_>>(),
            ["id", "late"]
        );
        assert_eq!(frozen.output.rows[1][1].text, "TEXT");
        assert_eq!(view.header(), Some(&["id".to_owned()][..]));
        assert_eq!(view.cursor(), cursor);
        assert_eq!(view.viewport(), viewport);
        let mut before = Vec::new();
        crate::output::write_prepared(
            crate::output::OutputFormat::Jsonl,
            crate::output::ColorOutput::Never,
            &frozen.output,
            None,
            &mut before,
        )
        .expect("serialize frozen output");
        let repeated = view
            .prepare_projection(
                crate::output::requirements(
                    crate::output::OutputFormat::Jsonl,
                    crate::output::ColorOutput::Never,
                )
                .expect("requirements"),
                &crate::theme::default_theme(),
            )
            .expect("repeated frozen export");
        let mut repeated_output = Vec::new();
        crate::output::write_prepared(
            crate::output::OutputFormat::Jsonl,
            crate::output::ColorOutput::Never,
            &repeated.output,
            None,
            &mut repeated_output,
        )
        .expect("serialize repeated export");
        assert_eq!(repeated_output, before);

        view.ensure_source_indexed_through(2)
            .expect("continue viewer");
        assert_eq!(
            view.header(),
            Some(&["id".to_owned(), "late".to_owned()][..])
        );
        assert_eq!(
            view.rendered_visible_row(1).as_deref(),
            Some(&["2".to_owned(), "TEXT".to_owned()][..])
        );
        view.set_current_column_width(3);
        let mut after = Vec::new();
        crate::output::write_prepared(
            crate::output::OutputFormat::Jsonl,
            crate::output::ColorOutput::Never,
            &frozen.output,
            None,
            &mut after,
        )
        .expect("serialize frozen output again");
        assert_eq!(before, after);
    }
}
