use super::column::{ColumnIndex, Columns};
use super::render;
use super::{DisplayFormatMetadata, QueryRefresh, TableView};
use crate::ops::filter::{ActiveFilter, FilterCondition, FilterMode, NumericOperator};
use crate::ops::sort::{sort_rows_by_specs, SortDirection, SortMode, SortSpec};
use crate::table::{
    InMemoryTable, NullPlacement, TableDefinition, ViewFilter, ViewFilterPredicate, ViewTransform,
};

impl TableView {
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

    pub fn toggle_header(&mut self) {
        if self.header.is_some() {
            self.header_visible = !self.header_visible;
        }
    }

    pub(super) fn apply_query_configuration(&mut self) -> bool {
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
        let query = self.build_view_transform(&definition);
        if let Err(error) = crate::table::validate_view_transform(&definition, &query) {
            self.source_status = Some(format!("Query validation failed: {error}"));
            return QueryRefresh::Failed;
        }

        if query.filters.is_empty() && query.order_by.is_empty() {
            self.restore_base_rows();
        } else {
            match self.refresh_transformed_rows(&definition, &query) {
                QueryRefresh::Applied => {}
                outcome => return outcome,
            }
        }
        self.active_view_transform = Some(query);
        self.committed_filters = self.filters.clone();
        self.committed_sort_keys = self.sort_keys.clone();
        self.restore_selected_row(selected_id);
        QueryRefresh::Applied
    }

    fn build_view_transform(&self, definition: &TableDefinition) -> ViewTransform {
        ViewTransform {
            generation: definition.generation,
            filters: self
                .filters
                .iter()
                .filter_map(|filter| view_filter(filter, definition))
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
        }
    }

    fn restore_base_rows(&mut self) {
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
    }

    fn refresh_transformed_rows(
        &mut self,
        definition: &TableDefinition,
        query: &ViewTransform,
    ) -> QueryRefresh {
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
        let result = match self.execute_view_transform(&base, definition, query) {
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

        QueryRefresh::Applied
    }

    fn execute_view_transform(
        &self,
        base: &InMemoryTable,
        definition: &TableDefinition,
        query: &ViewTransform,
    ) -> anyhow::Result<InMemoryTable> {
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
        crate::table::execute_local_view_transform_with_profiles(
            base,
            definition,
            query,
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
        )
    }

    fn restore_selected_row(&mut self, selected_id: Option<crate::table::RowId>) {
        if let Some(selected_id) = selected_id {
            if let Some(position) = self.row_ids.iter().position(|id| *id == selected_id) {
                self.cursor.row = position;
            }
        }
        self.cursor.row = self.cursor.row.min(self.rows.len().saturating_sub(1));
    }

    pub fn active_view_transform(&self) -> Option<&crate::table::ViewTransform> {
        self.active_view_transform.as_ref()
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

fn view_filter(filter: &ActiveFilter, definition: &TableDefinition) -> Option<ViewFilter> {
    let column = definition.columns.get(filter.column)?.id;
    let mode = match filter.mode {
        FilterMode::In => crate::table::FilterMode::In,
        FilterMode::Out => crate::table::FilterMode::Out,
    };
    let predicate = match &filter.condition {
        FilterCondition::Text(value) => ViewFilterPredicate::Text {
            value: value.clone(),
            domain: crate::table::ValueDomain::RawOrRendered,
        },
        FilterCondition::Regex(regex) => ViewFilterPredicate::Regex {
            pattern: regex.as_str().to_owned(),
            domain: crate::table::ValueDomain::RawOrRendered,
        },
        FilterCondition::Numeric { operator, operand } => ViewFilterPredicate::Numeric {
            operator: view_numeric_operator(*operator),
            operand: *operand,
        },
    };
    Some(ViewFilter {
        column,
        mode,
        predicate,
    })
}

fn view_numeric_operator(operator: NumericOperator) -> crate::table::NumericOperator {
    use crate::table::NumericOperator as ViewOperator;
    match operator {
        NumericOperator::LessThan => ViewOperator::LessThan,
        NumericOperator::LessThanOrEqual => ViewOperator::LessThanOrEqual,
        NumericOperator::GreaterThan => ViewOperator::GreaterThan,
        NumericOperator::GreaterThanOrEqual => ViewOperator::GreaterThanOrEqual,
        NumericOperator::Equal => ViewOperator::Equal,
    }
}
