use super::identity::compatible_source_column;
use super::{ActiveSortKey, ColumnDisplayMetadata, TableView};
use crate::table::TableDefinition;

impl TableView {
    pub fn restore_view_settings_from(&mut self, previous: &TableView) -> anyhow::Result<()> {
        let remap = self.column_remap(previous);
        let previous_cursor_source = previous.source_column_for_visible(previous.cursor.column);

        self.restore_layout(previous, &remap);
        self.restore_column_metadata(previous, &remap);
        self.restore_operations(previous, &remap);
        #[cfg(feature = "saved-views")]
        self.restore_saved_binding(previous, &remap);
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

    fn column_remap(&self, previous: &Self) -> Vec<Option<usize>> {
        (0..previous.source_column_count())
            .map(
                |old_index| match (&previous.table_definition, &self.table_definition) {
                    (Some(old), Some(new)) => matching_column(old, new, old_index),
                    (None, None) => (old_index < self.source_column_count()).then_some(old_index),
                    _ => None,
                },
            )
            .collect()
    }

    fn restore_layout(&mut self, previous: &Self, remap: &[Option<usize>]) {
        let source_column_count = self.source_column_count();
        self.column_width_mode = previous.column_width_mode;
        self.column_gap = previous.column_gap;
        self.column_widths = if previous.column_widths.is_empty() {
            Vec::new()
        } else {
            remap_values(&previous.column_widths, remap, source_column_count, 1)
        };
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
    }

    fn restore_column_metadata(&mut self, previous: &Self, remap: &[Option<usize>]) {
        let source_column_count = self.source_column_count();
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
            let header = self
                .header
                .as_mut()
                .and_then(|header| header.get_mut(new_index));
            if let (Some(label), Some(header)) = (&self.column_label_overrides[new_index], header) {
                header.clone_from(label);
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
    }

    fn restore_operations(&mut self, previous: &Self, remap: &[Option<usize>]) {
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
    }

    #[cfg(feature = "saved-views")]
    fn restore_saved_binding(&mut self, previous: &Self, remap: &[Option<usize>]) {
        self.saved_binding = previous.saved_binding.clone();
        if let Some(binding) = self.saved_binding.as_mut() {
            binding.remap_columns(remap);
        }
        self.saved_binding_warnings = previous.saved_binding_warnings.clone();
        self.saved_column_widths = remap_values(
            &previous.saved_column_widths,
            remap,
            self.source_column_count(),
            None,
        );
    }
}

pub(super) fn matching_column(
    previous: &TableDefinition,
    next: &TableDefinition,
    old_index: usize,
) -> Option<usize> {
    let old = previous.columns.get(old_index)?;
    let mut matches = next
        .columns
        .iter()
        .enumerate()
        .filter(|(_, candidate)| compatible_source_column(old, candidate));
    let (new_index, new) = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    let unique_old = previous
        .columns
        .iter()
        .filter(|candidate| compatible_source_column(candidate, new))
        .take(2)
        .count()
        == 1;
    unique_old.then_some(new_index)
}

fn remap_values<T: Clone>(
    previous: &[T],
    remap: &[Option<usize>],
    count: usize,
    default: T,
) -> Vec<T> {
    let mut values = vec![default; count];
    for (value, new_index) in previous.iter().zip(remap) {
        if let Some(new_index) = new_index {
            values[*new_index] = value.clone();
        }
    }
    values
}
