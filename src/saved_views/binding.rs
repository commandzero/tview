//! Invocation-local interpretation of saved presentation against typed source identity.
//! The active view owns installed metadata and operations; this owner retains only work
//! that may resolve on later schema/profile progress and warning identities.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::table::TableDefinition;

use super::{
    ColumnView, FilterAction, FilterKind, ResolvedColumnView, SavedFilter, SavedViewConfig,
    SavedViewWarning, SortDirection, SortKey, SortKind, MAX_SORT_KEYS,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundSort {
    pub column: usize,
    pub direction: SortDirection,
    pub kind: SortKind,
}
impl BoundSort {
    pub fn to_active(
        self,
        type_mode: crate::ops::sort::SortMode,
        nulls: crate::table::NullPlacement,
    ) -> crate::view::ActiveSortKey {
        use crate::ops::sort::{SortDirection as ActiveDirection, SortMode};
        crate::view::ActiveSortKey {
            column: self.column,
            mode: match self.kind {
                SortKind::Lexical => SortMode::Lexical,
                SortKind::Natural => SortMode::Natural,
                SortKind::Numeric => SortMode::Numeric,
                SortKind::Type => type_mode,
            },
            direction: match self.direction {
                SortDirection::Asc => ActiveDirection::Ascending,
                SortDirection::Desc => ActiveDirection::Descending,
            },
            nulls,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundFilter {
    pub index: usize,
    pub column: usize,
    pub action: FilterAction,
    pub kind: FilterKind,
    pub condition: String,
}

#[derive(Debug, Default)]
pub struct BindingUpdate {
    pub columns: Vec<ResolvedColumnView>,
    pub sorts: Option<Vec<BoundSort>>,
    pub filters: Vec<BoundFilter>,
    pub warnings: Vec<SavedViewWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterBindingOutcome {
    Installed,
    NumericUnavailable,
    Invalid(String),
}
impl BoundFilter {
    pub(crate) fn parse_active(
        self,
        profile: crate::ops::sort::NumericColumnProfile,
        numeric_available: bool,
    ) -> Result<crate::ops::filter::ActiveFilter, FilterBindingOutcome> {
        use crate::ops::filter::{
            ActiveFilter, FilterCondition, FilterKind as ActiveKind, FilterMode, FilterParseError,
        };
        let kind = match self.kind {
            FilterKind::Text => ActiveKind::Text,
            FilterKind::Regex => ActiveKind::Regex,
            FilterKind::Numeric => ActiveKind::Numeric,
        };
        if kind == ActiveKind::Numeric && !numeric_available {
            return Err(FilterBindingOutcome::NumericUnavailable);
        }
        let condition = FilterCondition::parse(kind, &self.condition, profile).map_err(
            |error| match error {
                FilterParseError::NumericUnavailable => FilterBindingOutcome::NumericUnavailable,
                other => FilterBindingOutcome::Invalid(other.to_string()),
            },
        )?;
        Ok(ActiveFilter::new(
            self.column,
            match self.action {
                FilterAction::In => FilterMode::In,
                FilterAction::Out => FilterMode::Out,
            },
            kind,
            self.condition,
            condition,
        ))
    }
}

#[derive(Debug, Clone)]
pub struct SavedViewBinding {
    locale: Option<String>,
    nulls: Option<crate::table::NullPlacement>,
    columns: BTreeMap<String, ColumnView>,
    bound_columns: BTreeSet<usize>,
    sorts: Vec<Option<SortKey>>,
    filters: Vec<Option<SavedFilter>>,
    emitted_sorts: Vec<BoundSort>,
    sort_edited: bool,
    edited_columns: BTreeSet<usize>,
    warned: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reference {
    Found(usize),
    Ambiguous,
    Missing,
}

impl SavedViewBinding {
    pub fn new(config: SavedViewConfig, sorts_enabled: bool) -> Self {
        Self {
            locale: config.locale,
            nulls: config.nulls,
            columns: config.columns,
            bound_columns: BTreeSet::new(),
            sorts: if sorts_enabled {
                config.sort.into_iter().map(Some).collect()
            } else {
                Vec::new()
            },
            filters: config.filters.into_iter().map(Some).collect(),
            emitted_sorts: Vec::new(),
            sort_edited: false,
            edited_columns: BTreeSet::new(),
            warned: BTreeSet::new(),
        }
    }

    pub fn locale(&self) -> Option<&str> {
        self.locale.as_deref()
    }

    pub fn nulls(&self) -> Option<crate::table::NullPlacement> {
        self.nulls
    }
    pub fn has_pending_filters(&self) -> bool {
        self.filters.iter().any(Option::is_some)
    }

    pub fn has_pending_numeric_filters(&self) -> bool {
        self.filters
            .iter()
            .flatten()
            .any(|filter| filter.kind == FilterKind::Numeric)
    }

    pub fn has_pending_sorts(&self) -> bool {
        !self.sort_edited && self.sorts.iter().any(Option::is_some)
    }

    /// Interactive edits own their field; subsequent discovery must not seed it again.
    pub fn supersede_sorts(&mut self) {
        self.sort_edited = true;
        self.sorts.clear();
    }
    pub fn supersede_all_filters(&mut self) {
        self.filters.clear();
    }

    pub fn supersede_filters_for_column(
        &mut self,
        column: usize,
        definition: Option<&TableDefinition>,
        headers: &[String],
    ) {
        let (structured, counts) = label_index(definition);
        for filter in &mut self.filters {
            if filter.as_ref().is_some_and(|filter| {
                resolve_reference(definition, headers, structured, &counts, &filter.column)
                    == Reference::Found(column)
            }) {
                *filter = None;
            }
        }
    }

    /// Reconcile only source-indexed binding state when a result is replaced.
    pub fn remap_columns(&mut self, remap: &[Option<usize>]) {
        let remap_set = |columns: &BTreeSet<usize>| {
            columns
                .iter()
                .filter_map(|&column| remap.get(column).copied().flatten())
                .collect()
        };
        self.bound_columns = remap_set(&self.bound_columns);
        self.edited_columns = remap_set(&self.edited_columns);
        self.emitted_sorts = self
            .emitted_sorts
            .iter()
            .filter_map(|sort| {
                Some(BoundSort {
                    column: remap.get(sort.column).copied().flatten()?,
                    ..*sort
                })
            })
            .collect();
    }

    pub fn supersede_column(&mut self, column: usize) {
        self.edited_columns.insert(column);
    }

    pub fn advance(
        &mut self,
        definition: Option<&TableDefinition>,
        headers: &[String],
        schema_complete: bool,
    ) -> BindingUpdate {
        if self.columns.is_empty() && !self.has_pending_sorts() && !self.has_pending_filters() {
            return BindingUpdate::default();
        }
        if definition.is_some_and(|definition| definition.columns.is_empty()) && !schema_complete {
            return BindingUpdate::default();
        }
        let mut update = BindingUpdate::default();
        let (structured, counts) = label_index(definition);
        let column_count = definition.map_or(headers.len(), |definition| definition.columns.len());
        let mut matched = BTreeSet::new();

        for index in 0..column_count {
            let source_label =
                source_label(definition, headers, index).expect("source column label");
            let found = if structured {
                let canonical =
                    definition.and_then(|definition| definition.canonical_column_key(index));
                canonical
                    .as_deref()
                    .and_then(|key| self.columns.get_key_value(key))
                    .or_else(|| {
                        (counts.get(source_label).copied() == Some(1))
                            .then(|| self.columns.get_key_value(source_label))
                            .flatten()
                    })
            } else {
                self.columns
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(source_label))
                    .or_else(|| {
                        self.columns
                            .iter()
                            .filter(|(key, _)| {
                                super::is_wildcard_pattern(key)
                                    && super::column_glob_matches(key, source_label)
                            })
                            .max_by(|left, right| {
                                super::wildcard_specificity(left.0)
                                    .cmp(&super::wildcard_specificity(right.0))
                                    .then_with(|| right.0.cmp(left.0))
                            })
                    })
            };
            if let Some((key, column)) = found {
                matched.insert(key.clone());
                if !self.edited_columns.contains(&index) && self.bound_columns.insert(index) {
                    update.columns.push(ResolvedColumnView {
                        column_index: index,
                        source_key: key.clone(),
                        view: column.clone(),
                    });
                }
            }
        }
        for key in &matched {
            if !super::is_wildcard_pattern(key) || schema_complete {
                self.columns.remove(key);
            }
        }
        let unresolved = self.columns.keys().cloned().collect::<Vec<_>>();
        for key in unresolved {
            let ambiguous = structured && counts.get(key.as_str()).copied().unwrap_or(0) > 1;
            // Only a wildcard that won a source column remains eligible for later columns.
            if !structured && super::is_wildcard_pattern(&key) && matched.contains(&key) {
                continue;
            }
            if ambiguous || schema_complete || !structured || !key.starts_with('/') {
                self.columns.remove(&key);
                let reason = if ambiguous {
                    "display label is ambiguous; use a canonical source key such as name#2"
                } else if structured {
                    "configured column matched no structured source column"
                } else {
                    "configured column matched no header"
                };
                self.warn(
                    &format!("column:{key}"),
                    format!("view.columns.{key}"),
                    reason,
                    &mut update,
                );
            }
        }

        if !self.sort_edited && !self.sorts.is_empty() {
            let mut resolved = Vec::new();
            let mut used = BTreeSet::new();
            for index in 0..self.sorts.len() {
                let Some(sort) = self.sorts[index].as_ref() else {
                    continue;
                };
                let result =
                    resolve_reference(definition, headers, structured, &counts, &sort.column);
                match result {
                    Reference::Found(column) => {
                        if used.insert(column) && resolved.len() < MAX_SORT_KEYS {
                            resolved.push(BoundSort {
                                column,
                                direction: sort.direction,
                                kind: sort.kind,
                            });
                        }
                    }
                    Reference::Ambiguous | Reference::Missing
                        if schema_complete
                            || !structured
                            || !sort.column.starts_with('/')
                            || result == Reference::Ambiguous =>
                    {
                        let key = format!("sort:{index}");
                        let field = format!("view.sort[{index}].column");
                        self.warn(
                            &key,
                            field,
                            if result == Reference::Ambiguous {
                                "display label is ambiguous; use a canonical source key"
                            } else {
                                "configured sort references a missing column"
                            },
                            &mut update,
                        );
                        self.sorts[index] = None;
                    }
                    _ => {}
                }
            }
            if resolved != self.emitted_sorts {
                self.emitted_sorts = resolved.clone();
                update.sorts = Some(resolved);
            }
            if schema_complete || self.sorts.iter().all(Option::is_none) {
                self.sorts.clear();
            }
        }
        for index in 0..self.filters.len() {
            let Some(filter) = self.filters[index].as_ref() else {
                continue;
            };
            match resolve_reference(definition, headers, structured, &counts, &filter.column) {
                Reference::Found(column) => update.filters.push(BoundFilter {
                    index,
                    column,
                    action: filter.action,
                    kind: filter.kind,
                    condition: filter.condition.clone(),
                }),
                result
                    if result == Reference::Ambiguous
                        || schema_complete
                        || !structured
                        || !filter.column.starts_with('/') =>
                {
                    let key = format!("filter:{index}");
                    let field = format!("view.filters[{index}].column");
                    self.warn(
                        &key,
                        field,
                        if result == Reference::Ambiguous {
                            "display label is ambiguous; use a canonical source key"
                        } else {
                            "configured filter references a missing column"
                        },
                        &mut update,
                    );
                    self.filters[index] = None;
                }
                _ => {}
            }
        }
        update
    }

    /// Report application using the consumer's existing filter parser and numeric profile.
    /// Temporary numeric unavailability is not a failed column reference.
    pub fn filter_feedback(
        &mut self,
        index: usize,
        outcome: FilterBindingOutcome,
        profile_definitive: bool,
    ) -> Option<SavedViewWarning> {
        self.filters.get(index)?.as_ref()?;
        let warning = match outcome {
            FilterBindingOutcome::Installed => None,
            FilterBindingOutcome::NumericUnavailable if !profile_definitive => return None,
            FilterBindingOutcome::NumericUnavailable => {
                Some("numeric filter is unavailable for this column".to_owned())
            }
            FilterBindingOutcome::Invalid(message) => Some(format!("invalid filter: {message}")),
        };
        self.filters[index] = None;
        warning.and_then(|message| {
            let identity = format!("filter:{index}");
            self.warned.insert(identity).then(|| SavedViewWarning {
                field: format!("view.filters[{index}]"),
                message,
            })
        })
    }

    fn warn(&mut self, identity: &str, field: String, message: &str, update: &mut BindingUpdate) {
        if self.warned.insert(identity.to_owned()) {
            update.warnings.push(SavedViewWarning {
                field,
                message: message.to_owned(),
            });
        }
    }
}

fn label_index(definition: Option<&TableDefinition>) -> (bool, HashMap<&str, usize>) {
    let structured = definition.is_some_and(|definition| {
        definition.columns.iter().any(|column| {
            column.source_identity.canonical_key().is_some()
                || matches!(
                    column.source_identity,
                    crate::table::ColumnSourceIdentity::RelationColumn { .. }
                )
        })
    });
    let mut counts = HashMap::new();
    if structured {
        for column in &definition.expect("typed structured source").columns {
            *counts.entry(column.display_name.as_str()).or_insert(0usize) += 1;
        }
    }
    (structured, counts)
}

fn source_label<'a>(
    definition: Option<&'a TableDefinition>,
    headers: &'a [String],
    index: usize,
) -> Option<&'a str> {
    if let Some(definition) = definition {
        definition
            .columns
            .get(index)
            .map(|column| column.display_name.as_str())
    } else {
        headers.get(index).map(String::as_str)
    }
}

fn resolve_reference(
    definition: Option<&TableDefinition>,
    headers: &[String],
    structured: bool,
    counts: &HashMap<&str, usize>,
    key: &str,
) -> Reference {
    if structured {
        if let Some(definition) = definition {
            if let Some(index) = (0..definition.columns.len())
                .find(|index| definition.canonical_column_key(*index).as_deref() == Some(key))
            {
                return Reference::Found(index);
            }
            match counts.get(key).copied().unwrap_or(0) {
                count if count > 1 => return Reference::Ambiguous,
                1 => {
                    return Reference::Found(
                        definition
                            .columns
                            .iter()
                            .position(|column| column.display_name == key)
                            .expect("counted source display label"),
                    )
                }
                _ => {}
            }
        }
    }
    if let Some(definition) = definition {
        resolve_compatible_header(
            definition
                .columns
                .iter()
                .map(|column| column.display_name.as_str()),
            key,
        )
    } else if structured {
        Reference::Missing
    } else {
        resolve_compatible_header(headers.iter().map(String::as_str), key)
    }
}

fn resolve_compatible_header<'a>(
    mut labels: impl Iterator<Item = &'a str> + Clone,
    key: &str,
) -> Reference {
    labels
        .clone()
        .position(|label| key.eq_ignore_ascii_case(label))
        .or_else(|| {
            super::is_wildcard_pattern(key)
                .then(|| labels.position(|label| super::column_glob_matches(key, label)))
                .flatten()
        })
        .map_or(Reference::Missing, Reference::Found)
}
