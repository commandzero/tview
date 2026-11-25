use super::{DisplayFormatMetadata, TableView};
use crate::ops::filter::FilterKind;
use crate::table::{SchemaDelta, SourceGeneration, TableDefinition};
use crate::theme::ResolvedTheme;

impl TableView {
    /// Snapshot only presentation and local operation facts. Source indexing
    /// may advance through the attached store, but the live viewer, cursor,
    /// viewport, and store attachment remain untouched even on failure.
    pub(crate) fn prepare_projection(
        &self,
        requirements: crate::output::OutputRequirements,
        theme: &ResolvedTheme,
    ) -> anyhow::Result<crate::projection::FrozenProjection> {
        let definition = self.projection_definition()?;
        let generation = definition.generation;
        let shared = self
            .incremental_store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("live output has no attached source store"))?;
        let settings = self.projection_settings(&definition);
        let mut delta = crate::table::SchemaDelta::default();
        let prepared = crate::projection::prepare(
            crate::projection::SourceInput {
                store: &mut **shared.0.borrow_mut(),
                definition,
                partial: self.source_result_is_partial,
                replay_schema: Some(&mut delta),
            },
            settings,
            crate::projection::ProjectionPolicy::Complete,
            requirements,
            theme,
        );
        self.record_projection_schema(generation, delta);
        prepared
    }

    fn projection_definition(&self) -> anyhow::Result<TableDefinition> {
        let mut definition = self
            .table_definition
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("live output has no activated source definition"))?
            .clone();
        let pending = self.pending_projection_schema.borrow();
        if let Some((_, replay)) = pending
            .as_ref()
            .filter(|(generation, _)| *generation == definition.generation)
        {
            definition.apply_delta(replay.clone())?;
        }
        Ok(definition)
    }

    fn projection_settings(
        &self,
        definition: &TableDefinition,
    ) -> crate::projection::ProjectionSettings {
        let mut settings = crate::projection::ProjectionSettings::new(
            self.column_width_mode,
            #[cfg(feature = "saved-views")]
            None,
            #[cfg(feature = "saved-views")]
            true,
            definition,
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
        self.apply_projection_layout(&mut settings);
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
        settings
    }

    fn apply_projection_layout(&self, settings: &mut crate::projection::ProjectionSettings) {
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
    }

    fn record_projection_schema(&self, generation: SourceGeneration, delta: SchemaDelta) {
        if delta.is_empty() {
            return;
        }
        let mut pending = self.pending_projection_schema.borrow_mut();
        match pending.as_mut() {
            Some((stored_generation, replay)) if *stored_generation == generation => {
                replay.added_columns.extend(delta.added_columns);
                replay.widened_types.extend(delta.widened_types);
                replay.completed |= delta.completed;
            }
            _ => *pending = Some((generation, delta)),
        }
    }
}
