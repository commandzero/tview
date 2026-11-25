use super::column::ColumnIndex;
use super::{
    ColumnAlignment, ColumnFilterSummary, ColumnFormatChoice, ColumnInfo, ColumnInfoUpdate,
    ColumnNullPlacementChoice, ColumnSortChoice, ColumnTypeChoice, ColumnTypeMetadata,
    DisplayFormatMetadata, TableView,
};
use crate::ops::filter::{FilterKind, FilterMode};
use crate::ops::sort::{SortDirection, SortMode};
use crate::table::NullPlacement;

impl TableView {
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
        let definition_column = self
            .table_definition
            .as_ref()
            .and_then(|definition| definition.columns.get(source_column));
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
            source_type: definition_column.map(|column| format!("{:?}", column.source_type)),
            source_declared_type: definition_column
                .and_then(|column| column.source_declared_type.clone()),
            source_operations: definition_column
                .map(|column| self.source_operations_for_column(column.id))
                .unwrap_or_default(),
            filters: self.column_filter_summary(source_column),
        })
    }

    fn column_filter_summary(&self, source_column: usize) -> Vec<ColumnFilterSummary> {
        self.filters
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
            .collect()
    }

    pub fn apply_current_column_info(&mut self, update: ColumnInfoUpdate) {
        let Some(source_column) = self.source_column_for_visible(self.cursor.column) else {
            return;
        };
        #[cfg(feature = "saved-views")]
        self.supersede_bound_column(source_column, update.clear_filters);
        self.set_column_presentation(source_column, &update);
        if update.clear_filters {
            self.filters.retain(|filter| filter.column != source_column);
        }

        self.sort_keys.retain(|key| key.column != source_column);
        let direction = match update.sort {
            ColumnSortChoice::None => None,
            ColumnSortChoice::Ascending => Some(SortDirection::Ascending),
            ColumnSortChoice::Descending => Some(SortDirection::Descending),
        };
        if let Some(direction) = direction {
            let mode = self.sort_mode_for_source(source_column, SortMode::Lexical);
            self.activate_sort_key(source_column, mode, direction);
        }
        self.sampled_column_widths.clear();
        self.computed_column_widths_cache.clear();
        self.apply_query_configuration();
        self.keep_cursor_visible();
    }

    #[cfg(feature = "saved-views")]
    fn supersede_bound_column(&mut self, source_column: usize, clear_filters: bool) {
        if let Some(binding) = self.saved_binding.as_mut() {
            binding.supersede_column(source_column);
            binding.supersede_sorts();
            if clear_filters {
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
    }

    fn set_column_presentation(&mut self, source_column: usize, update: &ColumnInfoUpdate) {
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
    }
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
