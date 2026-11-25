use super::column::ColumnIndex;
use super::{
    ActiveSortKey, ColumnAlignment, ColumnTypeMetadata, ColumnWidthMode, DisplayFormatMetadata,
    LocaleMetadata, NumberMaskMetadata, TableView, MAX_ACTIVE_SORT_KEYS,
};

impl TableView {
    pub fn apply_saved_columns(
        &mut self,
        resolved: &[crate::saved_views::ResolvedColumnView],
        locale: Option<&str>,
    ) {
        self.apply_saved_labels(resolved);
        self.saved_column_widths
            .resize(self.source_column_count(), None);
        self.ensure_custom_column_widths();
        let widths = SavedColumnWidths {
            header: self.computed_header_widths(),
            content: self.computed_content_widths(),
            mode: self.computed_column_widths(ColumnWidthMode::Mode),
            max: self.computed_column_widths(ColumnWidthMode::Max),
        };
        let locale = LocaleMetadata::from_posix(locale);

        for resolved_column in resolved {
            let source_column = resolved_column.column_index;
            let column_view = &resolved_column.view;
            if let Some(slot) = self.column_nulls.get_mut(source_column) {
                *slot = column_view.nulls;
            }
            self.apply_saved_display(source_column, column_view, locale);
            self.apply_saved_width(source_column, column_view.width, &widths);
            self.apply_saved_layout(source_column, column_view);
        }
        if self.hidden_columns.len() >= self.source_column_count() {
            self.hidden_columns.clear();
        }
        self.invalidate_conditional_colors();
        self.keep_cursor_visible();
    }

    fn apply_saved_labels(&mut self, resolved: &[crate::saved_views::ResolvedColumnView]) {
        if self.table_definition.is_none() && self.resident_source_header.is_none() {
            self.resident_source_header = resolved
                .iter()
                .any(|column| column.view.label.is_some())
                .then(|| self.header.clone())
                .flatten();
        }
        for resolved_column in resolved {
            let source_column = resolved_column.column_index;
            let Some(label) = resolved_column.view.label.as_ref() else {
                continue;
            };
            if let Some(name) = self
                .header
                .as_mut()
                .and_then(|header| header.get_mut(source_column))
            {
                name.clone_from(label);
            }
            if let Some(slot) = self.column_label_overrides.get_mut(source_column) {
                *slot = Some(label.clone());
            }
            self.column_metadata_modified.insert(source_column);
        }
    }

    fn apply_saved_display(
        &mut self,
        source_column: usize,
        column_view: &crate::saved_views::ColumnView,
        locale: LocaleMetadata,
    ) {
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
            let has_format = column_view.format.is_some() || column_view.mask.is_some();
            if display.type_explicit || has_format {
                self.column_metadata_modified.insert(source_column);
            }
        }
        if let Some(slot) = self.column_color_rules.get_mut(source_column) {
            slot.clone_from(&column_view.colors);
            if !slot.is_empty() {
                self.column_metadata_modified.insert(source_column);
            }
        }
    }

    fn apply_saved_width(
        &mut self,
        source_column: usize,
        width: Option<crate::saved_views::ColumnWidth>,
        widths: &SavedColumnWidths,
    ) {
        let Some(width) = width else {
            return;
        };
        if let Some(saved_width) = self.saved_column_widths.get_mut(source_column) {
            *saved_width =
                (!matches!(width, crate::saved_views::ColumnWidth::Fixed(_))).then_some(width);
        }
        if let Some(target) = self.column_widths.get_mut(source_column) {
            *target = widths.resolve(width, source_column);
            self.column_width_modified.insert(source_column);
        }
    }

    fn apply_saved_layout(
        &mut self,
        source_column: usize,
        column_view: &crate::saved_views::ColumnView,
    ) {
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
        if let Some((alignment, slot)) =
            alignment.zip(self.column_alignment_overrides.get_mut(source_column))
        {
            *slot = Some(alignment);
            self.column_metadata_modified.insert(source_column);
        }

        if let Some(visible) = column_view.visible {
            if visible {
                self.hidden_columns.remove(&source_column);
            } else {
                self.hidden_columns.insert(source_column);
            }
            self.column_metadata_modified.insert(source_column);
        }
    }

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

    fn saved_binding_headers(&self) -> &[String] {
        self.resident_source_header
            .as_deref()
            .or(self.header.as_deref())
            .unwrap_or(&[])
    }

    pub fn take_saved_binding_warnings(&mut self) -> Vec<crate::saved_views::SavedViewWarning> {
        std::mem::take(&mut self.saved_binding_warnings)
    }

    pub(super) fn advance_saved_binding(&mut self, profile_definitive: bool) {
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
}

pub(super) fn system_locale() -> Option<String> {
    ["LC_ALL", "LC_NUMERIC", "LANG"]
        .into_iter()
        .find_map(|key| {
            let value = std::env::var(key).ok()?;
            (!value.is_empty() && value != "C" && value != "POSIX").then_some(value)
        })
}

pub(super) fn column_type_metadata(
    column_type: crate::saved_views::ColumnType,
) -> ColumnTypeMetadata {
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

pub(super) fn display_format_metadata(
    format: crate::saved_views::DisplayFormat,
) -> DisplayFormatMetadata {
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

struct SavedColumnWidths {
    header: Vec<usize>,
    content: Vec<usize>,
    mode: Vec<usize>,
    max: Vec<usize>,
}

impl SavedColumnWidths {
    fn resolve(&self, width: crate::saved_views::ColumnWidth, source_column: usize) -> usize {
        use crate::saved_views::ColumnWidth;

        let measured = match width {
            ColumnWidth::Fixed(width) => return (width as usize).max(1),
            ColumnWidth::Header => &self.header,
            ColumnWidth::Content => &self.content,
            ColumnWidth::Mode => &self.mode,
            ColumnWidth::Max => &self.max,
        };
        measured.get(source_column).copied().unwrap_or(1).max(1)
    }
}
