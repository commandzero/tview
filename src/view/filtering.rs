use super::column::ColumnIndex;
use super::TableView;
use crate::ops::filter::{ActiveFilter, FilterKind, FilterMode};
use crate::ops::sort::NumericColumnProfile;

impl TableView {
    pub(crate) fn is_numeric_column(&self, column: usize) -> bool {
        self.source_column_for_visible(column)
            .is_some_and(|source_column| self.columns.is_numeric(ColumnIndex::new(source_column)))
    }

    pub(super) fn source_numeric_column_profile(
        &self,
        source_column: usize,
    ) -> NumericColumnProfile {
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

    pub(super) fn source_column_filter_indicator(
        &self,
        source_column: usize,
    ) -> Option<&'static str> {
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

    pub(super) fn install_filter(&mut self, filter: ActiveFilter) {
        #[cfg(feature = "saved-views")]
        let source_column = filter.column;
        self.filters.push(filter);
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

    pub(super) fn row_passes_filters(&self, row: &[String]) -> bool {
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
}
