use super::{TableView, VisibleCellStyleContext};
use crate::ops::search::CaseInsensitiveQuery;
use crate::ops::sort::parse_numeric_scalar;
use crate::table::RowIndex;
use crate::theme::{
    ColorProfileDemand, ColorProfileScope, ColumnColorProfile, CompiledColumnColors, ResolvedTheme,
};
use std::collections::{BTreeMap, BTreeSet};

impl TableView {
    #[cfg(all(test, feature = "saved-views"))]
    pub(super) fn visible_cell_style_context(
        &self,
        position: super::Position,
        rendered: &str,
        query: Option<&CaseInsensitiveQuery<'_>>,
    ) -> VisibleCellStyleContext {
        let Some(source_column) = self.source_column_for_visible(position.column) else {
            return VisibleCellStyleContext {
                conditional_color: None,
                search_match: false,
            };
        };
        self.source_cell_style_context(position.row, source_column, rendered, query)
    }

    pub(super) fn style_indexed_cell(
        &self,
        source_row: usize,
        (source_column, rendered): (usize, &str),
        query: Option<&CaseInsensitiveQuery<'_>>,
    ) -> VisibleCellStyleContext {
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

    pub(super) fn invalidate_conditional_colors(&mut self) {
        self.color_cache_valid = false;
    }

    pub(super) fn invalidate_color_source_facts(&mut self) {
        self.exact_color_profiles = None;
        self.invalidate_conditional_colors();
    }

    pub(super) fn refresh_resident_color_facts_on_append(&mut self) {
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

    pub(super) fn exact_reduction_profiles(
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
