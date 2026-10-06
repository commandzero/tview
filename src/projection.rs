//! Prepare an activated result for serialization without granting the serializer
//! access to its source. Indexing shared store caches may advance; settings and
//! live viewer state never belong to this selection workspace.

use std::collections::BTreeSet;

use crate::table::{Row, RowCount, RowIndex, SchemaDelta, TableDefinition, TableStore};

/// A complete export or an effective-result prefix (after source and view operations).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectionPolicy {
    Complete,
    Prefix { limit: usize, full_schema: bool },
}

/// The narrow binding/evaluation interface used while the active source is read.
/// Its implementor owns a projection-local copy of settings and binding progress,
/// never a `TableView` or mutable live binding state.
pub(crate) trait SelectionBehavior {
    /// Schema observed for interpretation, including rejected and lookahead rows.
    fn observe(&mut self, definition: &TableDefinition) -> anyhow::Result<()>;
    fn pending_filters(&self) -> bool;
    fn numeric_filter(&self) -> bool;
    fn sort_enabled(&self) -> bool;
    fn filters_active(&self) -> bool;
    fn accepts(&self, row: &Row) -> bool;
    /// Resolve numeric filters against the whole activated result before selection.
    fn finish_full_scan(
        &mut self,
        rows: &[Row],
        definition: &TableDefinition,
    ) -> anyhow::Result<()>;
    fn sort_accepted(&self, rows: &mut Vec<Row>);
}

/// Owned typed rows, accepted source-column identities, and frozen remainder.
/// No store handle or callback capable of fetching another row is retained.
#[derive(Debug)]
pub(crate) struct SelectedRows {
    pub definition: TableDefinition,
    pub rows: Vec<Row>,
    pub accepted_columns: BTreeSet<usize>,
    pub remaining: Option<RowCount>,
}

/// Sequential selection stops at one *matching* lookahead; full traversal is
/// reserved for complete exports, sorting, numeric filtering, and full schema.
/// Deferred rows remain in the existing indexed store and are replayed by index
/// when a late binding resolves, rather than copying cell vectors or a viewer.
pub(crate) fn select_rows(
    store: &mut dyn TableStore,
    mut definition: TableDefinition,
    policy: ProjectionPolicy,
    behavior: &mut impl SelectionBehavior,
    partial: bool,
) -> anyhow::Result<SelectedRows> {
    if store.generation() != definition.generation {
        anyhow::bail!("projection source belongs to a different generation");
    }
    let initial_columns = store.initial_schema_column_count();
    let mut first_progress = store.ensure_indexed_through(RowIndex(0))?;
    definition.apply_delta(std::mem::take(&mut first_progress.schema_delta))?;
    // Bind the source's first available schema, including declared headers of
    // empty sources, before choosing traversal demand.
    behavior.observe(&definition)?;
    let mut first_progress = Some(first_progress);
    let (limit, full_schema) = match policy {
        ProjectionPolicy::Complete => (usize::MAX, true),
        ProjectionPolicy::Prefix { limit, full_schema } => (limit, full_schema),
    };
    let materialize = matches!(policy, ProjectionPolicy::Complete)
        || behavior.sort_enabled()
        || behavior.numeric_filter();
    let full_traversal = materialize || full_schema;
    if full_traversal {
        // Interactive streaming stores index available bytes without waiting
        // for a requested row. A complete traversal must first await effective
        // EOF; temporary absence is not EOF, and late errors must propagate.
        let mut progress = store.ensure_indexed_through(RowIndex(usize::MAX))?;
        definition.apply_delta(std::mem::take(&mut progress.schema_delta))?;
        behavior.observe(&definition)?;
        first_progress = Some(progress);
    }
    let source_filters = store.has_source_filters()
        || store
            .active_source_query()
            .is_some_and(|query| !query.filters.is_empty());
    let mut rows = Vec::new();
    let mut all_rows = Vec::new();
    let mut accepted_columns = BTreeSet::new();
    let mut deferred = Vec::new();
    let mut matches = 0usize;
    let mut index = 0usize;
    let mut more = false;
    let mut eof = false;
    let mut replay_truncated = false;

    loop {
        let progress = if let Some(first) = first_progress.take() {
            first
        } else {
            store.ensure_indexed_through(RowIndex(index))?
        };
        let SchemaDelta {
            added_columns,
            widened_types,
            completed,
        } = progress.schema_delta;
        if !added_columns.is_empty() || !widened_types.is_empty() || completed {
            definition.apply_delta(SchemaDelta {
                added_columns,
                widened_types,
                completed,
            })?;
            // The lookahead is evidence of another effective match, not a
            // second opportunity to change frozen presentation or warnings.
            if full_traversal || rows.len() < limit || behavior.pending_filters() {
                behavior.observe(&definition)?;
            }
        }
        if !behavior.pending_filters() && !deferred.is_empty() {
            for deferred_index in deferred.drain(..) {
                let row = store
                    .row(RowIndex(deferred_index))?
                    .ok_or_else(|| anyhow::anyhow!("indexed deferred row is unavailable"))?;
                if behavior.accepts(&row) {
                    more |= record_match(
                        store,
                        row,
                        limit,
                        full_schema,
                        &mut rows,
                        &mut accepted_columns,
                        &mut matches,
                    );
                    if more && !full_traversal {
                        replay_truncated = true;
                        break;
                    }
                }
            }
            if more && !full_traversal {
                break;
            }
        }
        if !materialize && behavior.pending_filters() {
            let indexed = match progress.row_count {
                RowCount::Exact(count) | RowCount::AtLeast(count) => count > index,
                RowCount::Unknown => store.row(RowIndex(index))?.is_some(),
            };
            if !indexed {
                eof = true;
                break;
            }
            deferred.push(index);
            index += 1;
            continue;
        }
        let Some(row) = store.row(RowIndex(index))? else {
            eof = true;
            break;
        };
        if materialize {
            all_rows.push(row);
        } else if behavior.accepts(&row) {
            more |= record_match(
                store,
                row,
                limit,
                full_schema,
                &mut rows,
                &mut accepted_columns,
                &mut matches,
            );
            if more && !full_traversal {
                break;
            }
        }
        index += 1;
    }

    if eof && !deferred.is_empty() {
        behavior.observe(&definition)?;
        if behavior.pending_filters() {
            anyhow::bail!("view filters remain unresolved after schema completion");
        }
        for deferred_index in deferred {
            let row = store
                .row(RowIndex(deferred_index))?
                .ok_or_else(|| anyhow::anyhow!("indexed deferred row is unavailable"))?;
            if behavior.accepts(&row) {
                more |= record_match(
                    store,
                    row,
                    limit,
                    full_schema,
                    &mut rows,
                    &mut accepted_columns,
                    &mut matches,
                );
                if more && !full_traversal {
                    break;
                }
            }
        }
    }
    if materialize {
        behavior.finish_full_scan(&all_rows, &definition)?;
        all_rows.retain(|row| behavior.accepts(row));
        matches = all_rows.len();
        behavior.sort_accepted(&mut all_rows);
        // Local filtering precedes exact sorting, then prefix selection.
        // Full-schema prefixes include omitted matches, but never rejected rows.
        for row in all_rows {
            if rows.len() < limit || full_schema {
                observe_presence(store, &row, &mut accepted_columns);
            }
            if rows.len() < limit {
                rows.push(row);
            } else {
                more = true;
            }
        }
    }
    let empty_source = eof && index == 0;
    if rows.is_empty() && !empty_source && behavior.filters_active() && !source_filters {
        accepted_columns.clear();
    } else if matches!(policy, ProjectionPolicy::Complete) || empty_source {
        // Complete export retains source fields rejected by local filters.
        // An empty configured source still has its declared header/schema.
        accepted_columns.extend(0..definition.columns.len());
    } else if !full_schema {
        // Initial structured fields can belong only to rejected or omitted
        // rows. Only declared delimited/native columns are schema hints.
        accepted_columns.extend((0..initial_columns).filter(|&column| {
            !matches!(
                &definition.columns[column].source_identity,
                crate::table::ColumnSourceIdentity::StructuredPath(_)
                    | crate::table::ColumnSourceIdentity::ObjectKey
                    | crate::table::ColumnSourceIdentity::Positional(_)
            )
        }));
    }
    let remaining = more.then(|| {
        if partial {
            RowCount::Unknown
        } else if eof || full_traversal {
            RowCount::Exact(matches.saturating_sub(rows.len()))
        } else {
            match store.row_count() {
                RowCount::Exact(total) if !behavior.filters_active() => {
                    RowCount::Exact(total.saturating_sub(rows.len()))
                }
                RowCount::Exact(total) if !replay_truncated && index.saturating_add(1) >= total => {
                    RowCount::Exact(matches.saturating_sub(rows.len()))
                }
                _ => RowCount::Unknown,
            }
        }
    });
    Ok(SelectedRows {
        definition,
        rows,
        accepted_columns,
        remaining,
    })
}

/// Returns true when this accepted row confirms an omitted match.
fn record_match(
    store: &dyn TableStore,
    row: Row,
    limit: usize,
    full_schema: bool,
    rows: &mut Vec<Row>,
    accepted_columns: &mut BTreeSet<usize>,
    matches: &mut usize,
) -> bool {
    *matches += 1;
    if rows.len() < limit {
        observe_presence(store, &row, accepted_columns);
        rows.push(row);
        false
    } else {
        if full_schema {
            observe_presence(store, &row, accepted_columns);
        }
        true
    }
}

fn observe_presence(store: &dyn TableStore, row: &Row, columns: &mut BTreeSet<usize>) {
    if let Some(present) = store.present_columns(row.id) {
        columns.extend(present);
    } else {
        columns.extend(0..row.cells.len());
    }
}

/// Projection-local saved binding and row evaluation; never a copied viewer.
pub(crate) struct ProjectionSettings {
    pub width: crate::view::ColumnWidthMode,
    pub headers: Vec<String>,
    pub diagnostics: Vec<String>,
    pub(crate) gap: usize,
    pub(crate) hidden: BTreeSet<usize>,
    pub(crate) labels: std::collections::BTreeMap<usize, String>,
    pub(crate) alignment: std::collections::BTreeMap<usize, crate::view::ColumnAlignment>,
    pub(crate) fixed_widths: std::collections::BTreeMap<usize, usize>,
    #[cfg(feature = "saved-views")]
    pub(crate) saved_widths: std::collections::BTreeMap<usize, crate::saved_views::ColumnWidth>,
    pub(crate) display: std::collections::BTreeMap<usize, crate::view::ColumnDisplayMetadata>,
    pub(crate) colors: std::collections::BTreeMap<usize, Vec<crate::theme::ConditionalColorRule>>,
    #[cfg(feature = "saved-views")]
    pub columns: std::collections::BTreeMap<usize, crate::saved_views::ColumnView>,
    #[cfg(feature = "saved-views")]
    pub(crate) binding: Option<crate::saved_views::binding::SavedViewBinding>,
    #[cfg(feature = "saved-views")]
    bound_sorts: Option<Vec<crate::saved_views::binding::BoundSort>>,
    pub(crate) filters: Vec<crate::ops::filter::ActiveFilter>,
    pub(crate) sorts: Vec<crate::view::ActiveSortKey>,
    profiles: Vec<crate::ops::sort::NumericColumnProfile>,
    numeric_columns: BTreeSet<usize>,
    numeric_definitive: bool,
    pub(crate) formatted_filter_profile_requested: bool,
    pub(crate) numeric_requested: bool,
    pub(crate) sort_requested: bool,
}

impl ProjectionSettings {
    pub(crate) fn new(
        width: crate::view::ColumnWidthMode,
        #[cfg(feature = "saved-views")] saved: Option<crate::saved_views::SavedViewConfig>,
        #[cfg(feature = "saved-views")] sorts_enabled: bool,
        definition: &TableDefinition,
    ) -> Self {
        #[cfg(feature = "saved-views")]
        let formatted_filter_profile_requested = saved.as_ref().is_some_and(|saved| {
            saved
                .filters
                .iter()
                .any(|filter| filter.kind != crate::saved_views::FilterKind::Numeric)
                && saved.columns.values().any(|column| {
                    column.mask.is_some()
                        || matches!(
                            column.format,
                            Some(
                                crate::saved_views::DisplayFormat::Locale
                                    | crate::saved_views::DisplayFormat::Mask
                            )
                        )
                })
        });
        #[cfg(feature = "saved-views")]
        let numeric_requested = saved.as_ref().is_some_and(|saved| {
            saved
                .filters
                .iter()
                .any(|filter| filter.kind == crate::saved_views::FilterKind::Numeric)
        }) || formatted_filter_profile_requested;
        #[cfg(feature = "saved-views")]
        let sort_requested =
            sorts_enabled && saved.as_ref().is_some_and(|saved| !saved.sort.is_empty());
        Self {
            width,
            headers: definition
                .columns
                .iter()
                .map(|column| column.display_name.clone())
                .collect(),
            diagnostics: Vec::new(),
            gap: 2,
            hidden: BTreeSet::new(),
            labels: std::collections::BTreeMap::new(),
            alignment: std::collections::BTreeMap::new(),
            fixed_widths: std::collections::BTreeMap::new(),
            #[cfg(feature = "saved-views")]
            saved_widths: std::collections::BTreeMap::new(),
            display: std::collections::BTreeMap::new(),
            colors: std::collections::BTreeMap::new(),
            #[cfg(feature = "saved-views")]
            columns: std::collections::BTreeMap::new(),
            #[cfg(feature = "saved-views")]
            binding: saved.map(|saved| {
                crate::saved_views::binding::SavedViewBinding::new(saved, sorts_enabled)
            }),
            #[cfg(feature = "saved-views")]
            bound_sorts: None,
            filters: Vec::new(),
            sorts: Vec::new(),
            profiles: Vec::new(),
            numeric_definitive: false,
            numeric_columns: BTreeSet::new(),
            #[cfg(feature = "saved-views")]
            formatted_filter_profile_requested,
            #[cfg(not(feature = "saved-views"))]
            formatted_filter_profile_requested: false,
            #[cfg(feature = "saved-views")]
            numeric_requested,
            #[cfg(not(feature = "saved-views"))]
            numeric_requested: false,
            #[cfg(feature = "saved-views")]
            sort_requested,
            #[cfg(not(feature = "saved-views"))]
            sort_requested: false,
        }
    }

    fn profile(&self, column: usize) -> crate::ops::sort::NumericColumnProfile {
        self.profiles.get(column).copied().unwrap_or_default()
    }

    #[cfg(feature = "saved-views")]
    fn apply_binding(&mut self, definition: &TableDefinition) {
        use crate::saved_views::binding::FilterBindingOutcome;
        use crate::saved_views::FilterKind;
        let Some(mut binding) = self.binding.take() else {
            return;
        };
        let update = binding.advance(
            Some(definition),
            &self.headers,
            definition.schema_state == crate::table::SchemaState::Complete,
        );
        for resolved in update.columns {
            let column = resolved.column_index;
            self.display.insert(
                column,
                crate::view::render::saved_column(
                    &resolved.view,
                    binding.locale(),
                    self.numeric_columns.contains(&column),
                ),
            );
            self.columns.insert(column, resolved.view);
        }
        if let Some(sorts) = update.sorts {
            self.bound_sorts = Some(sorts);
        }
        if self.numeric_definitive {
            if let Some(sorts) = self.bound_sorts.take() {
                self.sorts = sorts
                    .into_iter()
                    .map(|sort| {
                        let column = self.columns.get(&sort.column);
                        let metadata = column.map_or_else(
                            crate::view::ColumnDisplayMetadata::default,
                            |column| {
                                crate::view::render::saved_column(
                                    column,
                                    binding.locale(),
                                    self.numeric_columns.contains(&sort.column),
                                )
                            },
                        );
                        let source_type = definition
                            .columns
                            .get(sort.column)
                            .map_or(crate::table::LogicalType::Unknown, |column| {
                                column.source_type
                            });
                        let type_mode = crate::view::render::type_sort_mode(
                            metadata,
                            source_type,
                            self.numeric_columns.contains(&sort.column),
                        );
                        let nulls = column
                            .and_then(|column| column.nulls)
                            .or_else(|| binding.nulls())
                            .unwrap_or(crate::table::NullPlacement::Last);
                        sort.to_active(type_mode, nulls)
                    })
                    .collect();
            }
        }
        for filter in update.filters {
            if filter.kind == FilterKind::Numeric && !self.numeric_definitive {
                use crate::ops::filter::{
                    FilterCondition, FilterKind as ActiveKind, FilterParseError,
                };
                let parsed = FilterCondition::parse(
                    ActiveKind::Numeric,
                    &filter.condition,
                    crate::ops::sort::NumericColumnProfile::default(),
                )
                .or_else(|error| match error {
                    FilterParseError::InvalidNumericOperand => FilterCondition::parse(
                        ActiveKind::Numeric,
                        &filter.condition,
                        crate::ops::sort::NumericColumnProfile::time(),
                    ),
                    other => Err(other),
                });
                if let Err(error) = parsed {
                    if let Some(warning) = binding.filter_feedback(
                        filter.index,
                        FilterBindingOutcome::Invalid(error.to_string()),
                        true,
                    ) {
                        self.diagnostics.push(format!(
                            "saved view: {}: {}",
                            warning.field, warning.message
                        ));
                    }
                }
                continue;
            }
            let index = filter.index;
            let profile = self.profile(filter.column);
            let available =
                filter.kind != FilterKind::Numeric || self.numeric_columns.contains(&filter.column);
            let outcome = match filter.parse_active(profile, available) {
                Ok(active) => {
                    self.filters.push(active);
                    FilterBindingOutcome::Installed
                }
                Err(outcome) => outcome,
            };
            if let Some(warning) = binding.filter_feedback(index, outcome, self.numeric_definitive)
            {
                self.diagnostics.push(format!(
                    "saved view: {}: {}",
                    warning.field, warning.message
                ));
            }
        }
        self.diagnostics.extend(
            update
                .warnings
                .into_iter()
                .map(|warning| format!("saved view: {}: {}", warning.field, warning.message)),
        );
        self.numeric_requested = (self.formatted_filter_profile_requested
            && (!self.filters.is_empty() || binding.has_pending_filters()))
            || self
                .filters
                .iter()
                .any(|filter| filter.kind == crate::ops::filter::FilterKind::Numeric)
            || binding.has_pending_numeric_filters();
        self.sort_requested = !self.sorts.is_empty()
            || self
                .bound_sorts
                .as_ref()
                .is_some_and(|sorts| !sorts.is_empty())
            || binding.has_pending_sorts();
        self.binding = Some(binding);
    }
}

impl SelectionBehavior for ProjectionSettings {
    fn observe(&mut self, definition: &TableDefinition) -> anyhow::Result<()> {
        self.headers.extend(
            definition
                .columns
                .iter()
                .skip(self.headers.len())
                .map(|column| column.display_name.clone()),
        );
        #[cfg(feature = "saved-views")]
        self.apply_binding(definition);
        Ok(())
    }

    fn pending_filters(&self) -> bool {
        #[cfg(feature = "saved-views")]
        if self
            .binding
            .as_ref()
            .is_some_and(|binding| binding.has_pending_filters())
        {
            return true;
        }
        false
    }

    fn numeric_filter(&self) -> bool {
        self.numeric_requested
    }
    fn sort_enabled(&self) -> bool {
        self.sort_requested || !self.sorts.is_empty()
    }
    fn filters_active(&self) -> bool {
        !self.filters.is_empty() || self.pending_filters()
    }

    fn accepts(&self, row: &Row) -> bool {
        self.filters.iter().all(|filter| {
            let raw = row
                .cells
                .get(filter.column)
                .map(|cell| cell.display())
                .unwrap_or_default();
            let rendered = self
                .display
                .get(&filter.column)
                .filter(|metadata| metadata.format != crate::view::DisplayFormatMetadata::Plain)
                .map(|metadata| {
                    crate::view::render::cell(*metadata, self.profile(filter.column), &raw)
                });
            filter.accepts_values(
                &raw,
                rendered.as_deref().unwrap_or(&raw),
                self.profile(filter.column),
            )
        })
    }

    fn finish_full_scan(
        &mut self,
        rows: &[Row],
        definition: &TableDefinition,
    ) -> anyhow::Result<()> {
        use crate::ops::sort::infer_numeric_profile_from_values;
        if self.numeric_requested
            || self.sort_requested
            || self
                .sorts
                .iter()
                .any(|sort| sort.mode == crate::ops::sort::SortMode::Numeric)
        {
            self.profiles = definition
                .columns
                .iter()
                .enumerate()
                .map(|(column, info)| {
                    infer_numeric_profile_from_values(
                        Some(&info.display_name),
                        rows.iter().map(|row| match row.cells.get(column) {
                            Some(
                                crate::table::CellValue::Text(text)
                                | crate::table::CellValue::Json(text),
                            ) => text.as_str(),
                            _ => "",
                        }),
                    )
                })
                .collect();
            for filter in &mut self.filters {
                if filter.kind == crate::ops::filter::FilterKind::Numeric {
                    let profile = self
                        .profiles
                        .get(filter.column)
                        .copied()
                        .unwrap_or_default();
                    filter.condition = crate::ops::filter::FilterCondition::parse(
                        filter.kind,
                        &filter.input,
                        profile,
                    )?;
                }
            }
            self.numeric_columns = self
                .profiles
                .iter()
                .enumerate()
                .filter_map(|(column, profile)| {
                    let mut has_number = false;
                    for row in rows.iter() {
                        let Some(cell) = row.cells.get(column) else {
                            continue;
                        };
                        match cell {
                            crate::table::CellValue::Integer(_)
                            | crate::table::CellValue::Float(_) => {
                                has_number = true;
                            }
                            crate::table::CellValue::Null => {}
                            value => {
                                let display = value.display();
                                let text = display.trim();
                                if !text.is_empty()
                                    && !crate::ops::sort::is_numeric_placeholder(text)
                                {
                                    if !crate::ops::sort::is_numeric_cell(text, *profile) {
                                        return None;
                                    }
                                    has_number = true;
                                }
                            }
                        }
                    }
                    (has_number || profile.is_time()).then_some(column)
                })
                .collect();
        }
        self.numeric_definitive = true;
        self.observe(definition)?;
        Ok(())
    }

    fn sort_accepted(&self, rows: &mut Vec<Row>) {
        if !self.sorts.is_empty() {
            static NULL: crate::table::CellValue = crate::table::CellValue::Null;
            rows.sort_by(|left, right| {
                for sort in &self.sorts {
                    let ordering = crate::table::compare_typed_cells(
                        left.cells.get(sort.column).unwrap_or(&NULL),
                        right.cells.get(sort.column).unwrap_or(&NULL),
                        table_sort_mode(sort.mode),
                        match sort.direction {
                            crate::ops::sort::SortDirection::Ascending => {
                                crate::table::SortDirection::Ascending
                            }
                            crate::ops::sort::SortDirection::Descending => {
                                crate::table::SortDirection::Descending
                            }
                        },
                        sort.nulls,
                        self.profile(sort.column),
                    );
                    if ordering != std::cmp::Ordering::Equal {
                        return ordering;
                    }
                }
                std::cmp::Ordering::Equal
            });
        }
    }
}

fn table_sort_mode(mode: crate::ops::sort::SortMode) -> crate::table::ViewSortMode {
    use crate::ops::sort::SortMode;
    use crate::table::ViewSortMode;
    match mode {
        SortMode::Lexical => ViewSortMode::Lexical,
        SortMode::Natural => ViewSortMode::Natural,
        SortMode::Numeric => ViewSortMode::Numeric,
        #[cfg(feature = "saved-views")]
        SortMode::Date => ViewSortMode::Date,
        #[cfg(feature = "saved-views")]
        SortMode::SemVer => ViewSortMode::SemanticVersion,
        #[cfg(feature = "saved-views")]
        SortMode::Ip => ViewSortMode::Ip,
        #[cfg(feature = "saved-views")]
        SortMode::Boolean => ViewSortMode::Boolean,
    }
}

pub(crate) struct FrozenProjection {
    pub output: crate::output::PreparedOutput,
    pub remaining: Option<RowCount>,
    pub diagnostics: Vec<String>,
    pub(crate) schema_replay: Option<SchemaDelta>,
}

pub(crate) struct SourceInput<'a> {
    pub store: &'a mut dyn TableStore,
    pub definition: TableDefinition,
    pub partial: bool,
    pub replay_schema: bool,
}

/// The only source-reading output preparation operation. All selection,
/// binding, profile and presentation work completes before returning an owned
/// result to a format adapter.
pub(crate) fn prepare(
    source: SourceInput<'_>,
    mut settings: ProjectionSettings,
    policy: ProjectionPolicy,
    requirements: crate::output::OutputRequirements,
    theme: &crate::theme::ResolvedTheme,
) -> anyhow::Result<FrozenProjection> {
    use crate::ops::sort::{infer_numeric_profile_from_values, parse_numeric_scalar};
    use crate::output::{
        display_width, freeze_table_cell, PreparedCell, PreparedColumn, PreparedOutput,
    };
    use crate::theme::{
        ColorProfileDemand, ColorProfileScope, ColumnColorProfile, CompiledColumnColors,
    };
    let SourceInput {
        store,
        definition,
        partial,
        replay_schema,
    } = source;

    let original_schema =
        replay_schema.then(|| (definition.columns.clone(), definition.schema_state));
    let selected = select_rows(store, definition, policy, &mut settings, partial)?;
    let schema_replay = original_schema.and_then(|(original, state)| {
        let delta = SchemaDelta {
            added_columns: selected
                .definition
                .columns
                .iter()
                .skip(original.len())
                .cloned()
                .collect(),
            widened_types: original
                .iter()
                .zip(&selected.definition.columns)
                .filter_map(|(before, after)| {
                    (before.source_type != after.source_type).then_some(
                        crate::table::TypeWidening {
                            column: after.id,
                            source_type: after.source_type,
                        },
                    )
                })
                .collect(),
            completed: state != crate::table::SchemaState::Complete
                && selected.definition.schema_state == crate::table::SchemaState::Complete,
        };
        (!delta.is_empty()).then_some(delta)
    });
    let header_visible = selected.definition.relation.header_visible;
    let table_layout = requirements.stable_widths;
    #[cfg(feature = "saved-views")]
    for (&index, config) in &settings.columns {
        if let Some(label) = &config.label {
            settings.labels.insert(index, label.clone());
        }
        if let Some(alignment) = config.align {
            settings.alignment.insert(
                index,
                match alignment {
                    crate::saved_views::ColumnAlign::Left => crate::view::ColumnAlignment::Left,
                    crate::saved_views::ColumnAlign::Right => crate::view::ColumnAlignment::Right,
                },
            );
        } else if matches!(
            config.column_type,
            Some(crate::saved_views::ColumnType::Number(_))
        ) {
            settings
                .alignment
                .insert(index, crate::view::ColumnAlignment::Right);
        }
        match config.visible {
            Some(false) => {
                settings.hidden.insert(index);
            }
            Some(true) => {
                settings.hidden.remove(&index);
            }
            None => {}
        }
        if !config.colors.is_empty() {
            settings.colors.insert(index, config.colors.clone());
        }
    }
    if !selected.accepted_columns.is_empty()
        && selected
            .accepted_columns
            .iter()
            .all(|index| settings.hidden.contains(index))
    {
        settings.hidden.clear();
    }
    let visible = selected
        .accepted_columns
        .iter()
        .copied()
        .filter(|index| !settings.hidden.contains(index))
        .collect::<Vec<_>>();
    let profile = |column: usize| {
        settings.profiles.get(column).copied().unwrap_or_else(|| {
            infer_numeric_profile_from_values(
                selected
                    .definition
                    .columns
                    .get(column)
                    .map(|column| column.display_name.as_str()),
                selected.rows.iter().map(|row| match row.cells.get(column) {
                    Some(
                        crate::table::CellValue::Text(text) | crate::table::CellValue::Json(text),
                    ) => text.as_str(),
                    _ => "",
                }),
            )
        })
    };
    let profiles = visible
        .iter()
        .map(|&column| {
            let numeric_format = settings.display.get(&column).is_some_and(|metadata| {
                matches!(
                    metadata.format,
                    crate::view::DisplayFormatMetadata::Locale
                        | crate::view::DisplayFormatMetadata::Mask
                )
            });
            if table_layout || requirements.conditional_styles || numeric_format {
                profile(column)
            } else {
                settings.profiles.get(column).copied().unwrap_or_default()
            }
        })
        .collect::<Vec<_>>();
    // Complete local transforms historically profile their resident, rendered
    // result. An untransformed complete source uses typed extrema and raw keys.
    let exact_color_domain = matches!(policy, ProjectionPolicy::Complete)
        && settings.filters.is_empty()
        && settings.sorts.is_empty();
    let header_style = theme.style("table.header");
    let cell_style = theme.style("table.cell");
    let header = if header_visible {
        visible
            .iter()
            .map(|&column| {
                let text = settings.labels.get(&column).cloned().unwrap_or_else(|| {
                    selected.definition.columns.get(column).map_or_else(
                        || format!("Column {}", column + 1),
                        |source| source.display_name.clone(),
                    )
                });
                PreparedCell {
                    text: if table_layout {
                        freeze_table_cell(text)
                    } else {
                        text
                    },
                    foreground: None,
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let numeric_align = if table_layout {
        visible
            .iter()
            .enumerate()
            .map(|(position, &column)| {
                let mut has_number = false;
                selected.rows.iter().all(|row| {
                    let Some(cell) = row.cells.get(column) else {
                        return true;
                    };
                    match cell {
                        crate::table::CellValue::Integer(_) | crate::table::CellValue::Float(_) => {
                            has_number = true;
                            true
                        }
                        crate::table::CellValue::Null => true,
                        other => {
                            let raw = other.display();
                            let text = raw.trim();
                            if text.is_empty() || crate::ops::sort::is_numeric_placeholder(text) {
                                true
                            } else if crate::ops::sort::is_numeric_cell(text, profiles[position]) {
                                has_number = true;
                                true
                            } else {
                                false
                            }
                        }
                    }
                }) && (has_number || profiles[position].is_time())
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let typed_extrema = if requirements.conditional_styles
        && settings
            .colors
            .values()
            .any(|rules| ColorProfileDemand::for_rules(rules).extrema)
    {
        visible
            .iter()
            .map(|&column| {
                if !settings
                    .colors
                    .get(&column)
                    .is_some_and(|rules| ColorProfileDemand::for_rules(rules).extrema)
                {
                    return None;
                }
                selected
                    .rows
                    .iter()
                    .filter_map(|row| match row.cells.get(column) {
                        Some(crate::table::CellValue::Integer(value)) => Some(*value as f64),
                        Some(crate::table::CellValue::Float(value)) => Some(*value),
                        _ => None,
                    })
                    .filter(|value| value.is_finite())
                    .fold(None::<(f64, f64)>, |range, value| {
                        Some(match range {
                            Some((min, max)) => (min.min(value), max.max(value)),
                            None => (value, value),
                        })
                    })
            })
            .collect::<Vec<Option<(f64, f64)>>>()
    } else {
        Vec::new()
    };
    let mut raw_slot_count = 0;
    let raw_color_slots = if requirements.conditional_styles {
        visible
            .iter()
            .map(|column| {
                if settings.colors.contains_key(column)
                    && settings.display.get(column).is_some_and(|metadata| {
                        metadata.format != crate::view::DisplayFormatMetadata::Plain
                    })
                {
                    let slot = raw_slot_count;
                    raw_slot_count += 1;
                    Some(slot)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let mut raw_for_colors = if requirements.conditional_styles {
        Vec::with_capacity(selected.rows.len())
    } else {
        Vec::new()
    };
    let mut rows: Vec<Vec<PreparedCell>> = Vec::with_capacity(selected.rows.len());
    for source_row in selected.rows {
        let mut source_cells = source_row.cells;
        let mut raw_overrides = vec![None; raw_slot_count];
        let cells: Vec<PreparedCell> =
            visible
                .iter()
                .zip(&profiles)
                .enumerate()
                .map(|(position, (&column, &profile))| {
                    let value = if column < source_cells.len() {
                        std::mem::replace(&mut source_cells[column], crate::table::CellValue::Null)
                    } else {
                        crate::table::CellValue::Null
                    };
                    let raw = match value {
                        crate::table::CellValue::Text(value)
                        | crate::table::CellValue::Json(value) => value,
                        other => other.display().into_owned(),
                    };
                    let text = match settings.display.get(&column).filter(|metadata| {
                        metadata.format != crate::view::DisplayFormatMetadata::Plain
                    }) {
                        Some(metadata) => {
                            let rendered = crate::view::render::cell(*metadata, profile, &raw);
                            if rendered == raw {
                                raw
                            } else {
                                if let Some(slot) = raw_color_slots.get(position).copied().flatten()
                                {
                                    raw_overrides[slot] = Some(raw);
                                }
                                rendered
                            }
                        }
                        None => raw,
                    };
                    PreparedCell {
                        text,
                        foreground: None,
                    }
                })
                .collect();
        rows.push(cells);
        if requirements.conditional_styles {
            raw_for_colors.push(raw_overrides);
        }
    }
    if requirements.conditional_styles {
        for (visible_column, (&column, &numeric_profile)) in
            visible.iter().zip(&profiles).enumerate()
        {
            let Some(rules) = settings.colors.get(&column) else {
                continue;
            };
            if rules.is_empty() {
                continue;
            }
            let demand = ColorProfileDemand::for_rules(rules);
            let mut extrema = if exact_color_domain && demand.extrema {
                typed_extrema[visible_column]
            } else {
                None
            };
            let mut identifiers = BTreeSet::new();
            let needs_resident_extrema =
                demand.extrema && (!exact_color_domain || typed_extrema[visible_column].is_none());
            if needs_resident_extrema || demand.identifiers {
                for (index, rendered_row) in rows.iter().enumerate() {
                    let rendered = &rendered_row[visible_column].text;
                    let raw = raw_color_slots[visible_column]
                        .and_then(|slot| raw_for_colors[index][slot].as_deref())
                        .unwrap_or(rendered);
                    if needs_resident_extrema {
                        if let Some(value) = parse_numeric_scalar(raw, numeric_profile)
                            .filter(|value| value.is_finite())
                        {
                            extrema = Some(match extrema {
                                Some((min, max)) => (min.min(value), max.max(value)),
                                None => (value, value),
                            });
                        }
                    }
                    if demand.identifiers {
                        let key = if exact_color_domain { raw } else { rendered };
                        if !key.is_empty() {
                            identifiers.insert(key.to_owned());
                        }
                    }
                }
            }
            if demand.extrema && extrema.is_none() {
                extrema = typed_extrema[visible_column];
            }
            let evaluator = CompiledColumnColors::prepare(
                theme,
                rules,
                ColumnColorProfile {
                    scope: if matches!(policy, ProjectionPolicy::Complete) {
                        ColorProfileScope::CompleteResult
                    } else {
                        ColorProfileScope::EmittedPreview
                    },
                    numeric_profile,
                    numeric_min_max: extrema,
                    identifier_indexes: identifiers
                        .into_iter()
                        .enumerate()
                        .map(|(index, key)| (key, index))
                        .collect(),
                },
            );
            for (index, rendered_row) in rows.iter_mut().enumerate() {
                let rendered = &mut rendered_row[visible_column];
                let raw = raw_color_slots[visible_column]
                    .and_then(|slot| raw_for_colors[index][slot].as_deref())
                    .unwrap_or(&rendered.text);
                rendered.foreground = evaluator.evaluate(raw, &rendered.text);
            }
        }
    }
    if table_layout {
        for row in &mut rows {
            for cell in row {
                cell.text = freeze_table_cell(std::mem::take(&mut cell.text));
            }
        }
    }

    let columns = if table_layout {
        visible
            .iter()
            .enumerate()
            .map(|(position, &column)| {
                let header_width = header.get(position).map_or_else(
                    || display_width(&selected.definition.columns[column].display_name).max(1),
                    |cell: &PreparedCell| display_width(&cell.text).max(1),
                );
                let content_width = rows
                    .iter()
                    .map(|row| display_width(&row[position].text))
                    .max()
                    .unwrap_or(1)
                    .max(1);
                #[cfg(feature = "saved-views")]
                let saved_width = settings
                    .saved_widths
                    .get(&column)
                    .copied()
                    .or_else(|| {
                        settings
                            .columns
                            .get(&column)
                            .and_then(|config| config.width)
                    })
                    .map(|width| match width {
                        crate::saved_views::ColumnWidth::Fixed(value) => value as usize,
                        crate::saved_views::ColumnWidth::Header => header_width,
                        crate::saved_views::ColumnWidth::Content => content_width,
                        crate::saved_views::ColumnWidth::Mode => content_width,
                        crate::saved_views::ColumnWidth::Max => {
                            header_width.max(content_width).min(250)
                        }
                    });
                #[cfg(not(feature = "saved-views"))]
                let saved_width: Option<usize> = None;
                let width = settings
                    .fixed_widths
                    .get(&column)
                    .copied()
                    .or(match settings.width {
                        crate::view::ColumnWidthMode::Fixed(width) => Some(width as usize),
                        _ => None,
                    })
                    .or(saved_width)
                    .unwrap_or(header_width.max(content_width))
                    .max(1);
                PreparedColumn {
                    alignment: settings.alignment.get(&column).copied().unwrap_or(
                        if numeric_align[position] {
                            crate::view::ColumnAlignment::Right
                        } else {
                            crate::view::ColumnAlignment::Left
                        },
                    ),
                    width,
                }
            })
            .collect()
    } else {
        vec![
            PreparedColumn {
                alignment: crate::view::ColumnAlignment::Left,
                width: 1
            };
            visible.len()
        ]
    };
    Ok(FrozenProjection {
        output: PreparedOutput {
            header_visible,
            header,
            rows,
            columns,
            gap: settings.gap,
            header_style,
            cell_style,
        },
        remaining: selected.remaining,
        diagnostics: settings.diagnostics,
        schema_replay,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{
        CellValue, ColumnDefinition, ColumnId, ColumnSourceIdentity, InMemoryTable, LogicalType,
        RelationMetadata, SchemaState, SourceGeneration, TypeOrigin,
    };

    struct NoViewOperations;

    impl SelectionBehavior for NoViewOperations {
        fn observe(&mut self, _: &TableDefinition) -> anyhow::Result<()> {
            Ok(())
        }
        fn pending_filters(&self) -> bool {
            false
        }
        fn numeric_filter(&self) -> bool {
            false
        }
        fn sort_enabled(&self) -> bool {
            false
        }
        fn filters_active(&self) -> bool {
            false
        }
        fn accepts(&self, _: &Row) -> bool {
            true
        }
        fn finish_full_scan(&mut self, _: &[Row], _: &TableDefinition) -> anyhow::Result<()> {
            Ok(())
        }
        fn sort_accepted(&self, _: &mut Vec<Row>) {}
    }

    fn typed_result() -> (TableDefinition, InMemoryTable) {
        let generation = SourceGeneration::new();
        let definition = TableDefinition {
            generation,
            columns: vec![ColumnDefinition {
                id: ColumnId {
                    generation,
                    ordinal: 0,
                },
                source_identity: ColumnSourceIdentity::Positional(0),
                display_name: "count".to_owned(),
                source_declared_type: Some("integer".to_owned()),
                source_type: LogicalType::Integer,
                type_origin: TypeOrigin::Declared,
            }],
            schema_state: SchemaState::Complete,
            relation: RelationMetadata::implicit("numbers", true),
        };
        let rows = (0..3)
            .map(|ordinal| {
                Row::new(
                    crate::table::RowId {
                        generation,
                        ordinal,
                    },
                    vec![CellValue::Integer(ordinal as i64)],
                )
            })
            .collect();
        (
            definition,
            InMemoryTable::from_rows(generation, rows).unwrap(),
        )
    }

    #[test]
    fn in_memory_complete_and_prefix_share_typed_result_contract() {
        let (definition, mut store) = typed_result();
        let prefix = select_rows(
            &mut store,
            definition.clone(),
            ProjectionPolicy::Prefix {
                limit: 1,
                full_schema: false,
            },
            &mut NoViewOperations,
            false,
        )
        .unwrap();
        assert_eq!(prefix.rows.len(), 1);
        assert_eq!(prefix.rows[0].cells, vec![CellValue::Integer(0)]);
        assert_eq!(prefix.remaining, Some(RowCount::Exact(2)));

        let complete = select_rows(
            &mut store,
            definition,
            ProjectionPolicy::Complete,
            &mut NoViewOperations,
            false,
        )
        .unwrap();
        assert_eq!(complete.rows.len(), 3);
        assert_eq!(complete.rows[2].cells, vec![CellValue::Integer(2)]);
        assert_eq!(complete.remaining, None);
    }
    #[test]
    fn sequential_ndjson_prefix_ignores_malformed_unread_suffix() {
        use crate::ingest::{JsonAdapter, OpenOptions, SourceAdapter};
        let file = tempfile::Builder::new()
            .suffix(".ndjson")
            .tempfile()
            .unwrap();
        std::fs::write(
            file.path(),
            "{\"id\":1}\n{\"id\":2,\"lookahead\":null}\ninvalid\n",
        )
        .unwrap();
        let opened = JsonAdapter::ndjson()
            .open(
                crate::ingest::source::InputSource::Path(file.path().to_owned()),
                &OpenOptions {
                    preview: true,
                    ..OpenOptions::default()
                },
            )
            .unwrap()
            .into_implicit_table()
            .unwrap();
        let mut store = opened.store;
        let result = select_rows(
            store.as_mut(),
            opened.definition,
            ProjectionPolicy::Prefix {
                limit: 1,
                full_schema: false,
            },
            &mut NoViewOperations,
            false,
        )
        .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].cells[0], CellValue::Integer(1));
        assert_eq!(result.remaining, Some(RowCount::Unknown));
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn typed_saved_filter_and_exact_sort_precede_prefix() {
        use crate::saved_views::{
            FilterAction, FilterKind, SavedFilter, SavedViewConfig, SortDirection, SortKey,
            SortKind,
        };
        let (definition, mut store) = typed_result();
        let saved = SavedViewConfig {
            sort: vec![SortKey {
                column: "count".to_owned(),
                direction: SortDirection::Desc,
                kind: SortKind::Numeric,
            }],
            filters: vec![SavedFilter {
                column: "count".to_owned(),
                action: FilterAction::In,
                kind: FilterKind::Numeric,
                condition: ">0".to_owned(),
            }],
            ..SavedViewConfig::default()
        };
        let mut settings = ProjectionSettings::new(
            crate::view::ColumnWidthMode::Mode,
            Some(saved),
            true,
            &definition,
        );
        let selected = select_rows(
            &mut store,
            definition,
            ProjectionPolicy::Prefix {
                limit: 1,
                full_schema: false,
            },
            &mut settings,
            false,
        )
        .unwrap();
        assert_eq!(selected.rows.len(), 1);
        assert_eq!(selected.rows[0].cells, vec![CellValue::Integer(2)]);
        assert_eq!(selected.remaining, Some(RowCount::Exact(1)));
        assert!(settings.diagnostics.is_empty());
    }
}
