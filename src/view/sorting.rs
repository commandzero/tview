#[cfg(feature = "saved-views")]
use super::{column::ColumnIndex, render, ColumnTypeMetadata};
use super::{ActiveSortKey, TableView, MAX_ACTIVE_SORT_KEYS};
use crate::ops::sort::{SortDirection, SortMode};
use crate::table::NullPlacement;

impl TableView {
    pub fn toggle_view_null_placement(&mut self) {
        let next = match self.view_nulls.unwrap_or(NullPlacement::Last) {
            NullPlacement::First => Some(NullPlacement::Last),
            NullPlacement::Last => Some(NullPlacement::First),
        };
        self.set_view_null_placement(next);
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

    pub(super) fn source_column_sort_indicator(
        &self,
        source_column: usize,
    ) -> Option<&'static str> {
        self.sort_keys
            .iter()
            .find(|key| key.column == source_column)
            .map(|key| match key.direction {
                SortDirection::Ascending => "▲",
                SortDirection::Descending => "▼",
            })
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
        self.change_current_sort(Some((mode, direction)));
    }

    pub fn clear_current_column_sort(&mut self) {
        self.change_current_sort(None);
    }

    fn change_current_sort(&mut self, sort: Option<(SortMode, SortDirection)>) {
        let Some(column) = self.source_column_for_visible(self.cursor.column) else {
            return;
        };
        #[cfg(feature = "saved-views")]
        if let Some(binding) = self.saved_binding.as_mut() {
            binding.supersede_sorts();
        }
        if let Some((mode, direction)) = sort {
            let mode = self.sort_mode_for_source(column, mode);
            self.activate_sort_key(column, mode, direction);
        } else {
            self.sort_keys.retain(|key| key.column != column);
        }
        self.computed_column_widths_cache.clear();
        self.apply_query_configuration();
        self.keep_cursor_visible();
    }

    pub fn sort_keys(&self) -> &[ActiveSortKey] {
        &self.sort_keys
    }

    pub(super) fn activate_sort_key(
        &mut self,
        column: usize,
        mode: SortMode,
        direction: SortDirection,
    ) {
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

    pub(super) fn sort_mode_for_source(
        &self,
        source_column: usize,
        requested: SortMode,
    ) -> SortMode {
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
}
