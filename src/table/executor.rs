use std::cmp::Ordering;

use regex::Regex;

use super::{
    CellValue, ColumnId, FilterMode, InMemoryTable, NullPlacement, Row, SortDirection, SourceQuery,
    SourceQueryValidationError, TableDefinition, TableStore, ValueDomain, ViewFilterPredicate,
    ViewSortMode, ViewTransform,
};

static NULL_CELL: CellValue = CellValue::Null;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ViewTransformValidationError {
    #[error("query belongs to a different source generation")]
    StaleGeneration,
    #[error("query references an unknown or stale column")]
    UnknownColumn,
    #[error("query contains an invalid regular expression: {0}")]
    InvalidRegex(String),
    #[error("query contains a non-finite numeric operand")]
    InvalidNumericOperand,
}

pub fn validate_source_query(
    definition: &TableDefinition,
    query: &SourceQuery,
) -> Result<(), SourceQueryValidationError> {
    if query.generation != definition.generation {
        return Err(SourceQueryValidationError::StaleGeneration);
    }
    for column in query
        .filters
        .iter()
        .filter_map(|filter| match &filter.scope {
            super::SourceFilterScope::WholeRecord => None,
            super::SourceFilterScope::Column(column) => Some(*column),
        })
        .chain(query.order_by.iter().map(|sort| sort.column))
    {
        if column.generation != definition.generation
            || !definition
                .columns
                .iter()
                .any(|candidate| candidate.id == column)
        {
            return Err(SourceQueryValidationError::UnknownColumn);
        }
    }
    for filter in &query.filters {
        filter.validate()?;
    }
    Ok(())
}

pub fn validate_view_transform(
    definition: &TableDefinition,
    query: &ViewTransform,
) -> Result<(), ViewTransformValidationError> {
    prepare_query(definition, query).map(|_| ())
}

fn prepare_query(
    definition: &TableDefinition,
    query: &ViewTransform,
) -> Result<Vec<Option<Regex>>, ViewTransformValidationError> {
    if query.generation != definition.generation {
        return Err(ViewTransformValidationError::StaleGeneration);
    }
    for column in query
        .filters
        .iter()
        .map(|filter| filter.column)
        .chain(query.order_by.iter().map(|sort| sort.column))
    {
        if column.generation != definition.generation
            || !definition
                .columns
                .iter()
                .any(|candidate| candidate.id == column)
        {
            return Err(ViewTransformValidationError::UnknownColumn);
        }
    }
    query
        .filters
        .iter()
        .map(|filter| match &filter.predicate {
            ViewFilterPredicate::Regex { pattern, .. } => Regex::new(pattern)
                .map(Some)
                .map_err(|error| ViewTransformValidationError::InvalidRegex(error.to_string())),
            ViewFilterPredicate::Numeric { operand, .. } if !operand.is_finite() => {
                Err(ViewTransformValidationError::InvalidNumericOperand)
            }
            _ => Ok(None),
        })
        .collect()
}

pub fn execute_view_transform(
    store: &mut dyn TableStore,
    definition: &TableDefinition,
    transform: &ViewTransform,
    render: &dyn Fn(ColumnId, &CellValue) -> String,
) -> anyhow::Result<Box<dyn TableStore>> {
    execute_view_transform_with_profiles(
        store,
        definition,
        transform,
        &|_| crate::ops::sort::NumericColumnProfile::default(),
        render,
    )
}

pub(crate) fn execute_view_transform_with_profiles(
    store: &mut dyn TableStore,
    definition: &TableDefinition,
    transform: &ViewTransform,
    numeric_profile: &dyn Fn(ColumnId) -> crate::ops::sort::NumericColumnProfile,
    render: &dyn Fn(ColumnId, &CellValue) -> String,
) -> anyhow::Result<Box<dyn TableStore>> {
    if store.generation() != definition.generation {
        anyhow::bail!("table store belongs to a different source generation");
    }
    let prepared_regexes = prepare_query(definition, transform)?;
    let base = store.materialize()?;
    Ok(Box::new(execute_prepared_local_query(
        &base,
        definition,
        transform,
        &prepared_regexes,
        numeric_profile,
        render,
    )?))
}

pub fn execute_local_view_transform(
    base: &InMemoryTable,
    definition: &TableDefinition,
    transform: &ViewTransform,
    render: &dyn Fn(ColumnId, &CellValue) -> String,
) -> anyhow::Result<InMemoryTable> {
    execute_local_view_transform_with_profiles(
        base,
        definition,
        transform,
        &|_| crate::ops::sort::NumericColumnProfile::default(),
        render,
    )
}

pub(crate) fn execute_local_view_transform_with_profiles(
    base: &InMemoryTable,
    definition: &TableDefinition,
    transform: &ViewTransform,
    numeric_profile: &dyn Fn(ColumnId) -> crate::ops::sort::NumericColumnProfile,
    render: &dyn Fn(ColumnId, &CellValue) -> String,
) -> anyhow::Result<InMemoryTable> {
    let prepared_regexes = prepare_query(definition, transform)?;
    execute_prepared_local_query(
        base,
        definition,
        transform,
        &prepared_regexes,
        numeric_profile,
        render,
    )
}

fn execute_prepared_local_query(
    base: &InMemoryTable,
    definition: &TableDefinition,
    query: &ViewTransform,
    prepared_regexes: &[Option<Regex>],
    numeric_profile: &dyn Fn(ColumnId) -> crate::ops::sort::NumericColumnProfile,
    render: &dyn Fn(ColumnId, &CellValue) -> String,
) -> anyhow::Result<InMemoryTable> {
    let mut rows = base
        .rows()
        .iter()
        .filter(|row| {
            query
                .filters
                .iter()
                .zip(prepared_regexes)
                .all(|(filter, prepared_regex)| {
                    let value = cell(row, filter.column);
                    let matched = predicate_matches(
                        &filter.predicate,
                        prepared_regex.as_ref(),
                        numeric_profile(filter.column),
                        filter.column,
                        value,
                        render,
                    );
                    match filter.mode {
                        FilterMode::In => matched,
                        FilterMode::Out => !matched,
                    }
                })
        })
        .cloned()
        .collect::<Vec<_>>();

    if !query.order_by.is_empty() {
        rows.sort_by(|left, right| {
            for spec in &query.order_by {
                let ordering = compare_typed_cells(
                    cell(left, spec.column),
                    cell(right, spec.column),
                    spec.mode,
                    spec.direction,
                    spec.nulls,
                    numeric_profile(spec.column),
                );
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            // `sort_by` is stable, so rows equal under all keys retain their
            // base-source order (the order in `base.rows()`).
            Ordering::Equal
        });
    }
    InMemoryTable::from_rows(definition.generation, rows)
}

fn cell(row: &Row, column: ColumnId) -> &CellValue {
    row.cells.get(column.ordinal as usize).unwrap_or(&NULL_CELL)
}

fn predicate_matches(
    predicate: &ViewFilterPredicate,
    prepared_regex: Option<&Regex>,
    numeric_profile: crate::ops::sort::NumericColumnProfile,
    column: ColumnId,
    value: &CellValue,
    render: &dyn Fn(ColumnId, &CellValue) -> String,
) -> bool {
    match predicate {
        ViewFilterPredicate::Text {
            value: needle,
            domain,
        } => domain_values(*domain, column, value, render)
            .iter()
            .any(|candidate| candidate.contains(needle)),
        ViewFilterPredicate::Regex { domain, .. } => prepared_regex.is_some_and(|regex| {
            domain_values(*domain, column, value, render)
                .iter()
                .any(|candidate| regex.is_match(candidate))
        }),
        ViewFilterPredicate::Numeric { operator, operand } => numeric_value(value, numeric_profile)
            .is_some_and(|value| {
                use super::NumericOperator;
                match operator {
                    NumericOperator::LessThan => value < *operand,
                    NumericOperator::LessThanOrEqual => value <= *operand,
                    NumericOperator::GreaterThan => value > *operand,
                    NumericOperator::GreaterThanOrEqual => value >= *operand,
                    NumericOperator::Equal => value.total_cmp(operand) == Ordering::Equal,
                }
            }),
    }
}

fn domain_values(
    domain: ValueDomain,
    column: ColumnId,
    value: &CellValue,
    render: &dyn Fn(ColumnId, &CellValue) -> String,
) -> Vec<String> {
    match domain {
        ValueDomain::Raw => vec![value.display().into_owned()],
        ValueDomain::Rendered => vec![render(column, value)],
        ValueDomain::RawOrRendered => {
            let raw = value.display().into_owned();
            let rendered = render(column, value);
            if rendered == raw {
                vec![raw]
            } else {
                vec![raw, rendered]
            }
        }
    }
}

fn numeric_value(
    value: &CellValue,
    profile: crate::ops::sort::NumericColumnProfile,
) -> Option<f64> {
    match value {
        CellValue::Integer(value) => Some(*value as f64),
        CellValue::Float(value) => Some(*value),
        CellValue::Text(value) => crate::ops::sort::parse_numeric_scalar(value, profile),
        _ => None,
    }
}

pub(crate) fn compare_typed_cells(
    left: &CellValue,
    right: &CellValue,
    mode: ViewSortMode,
    direction: SortDirection,
    nulls: NullPlacement,
    numeric_profile: crate::ops::sort::NumericColumnProfile,
) -> Ordering {
    let null_order = match (
        matches!(left, CellValue::Null),
        matches!(right, CellValue::Null),
    ) {
        (true, true) => return Ordering::Equal,
        (true, false) => Some(match nulls {
            NullPlacement::First => Ordering::Less,
            NullPlacement::Last => Ordering::Greater,
        }),
        (false, true) => Some(match nulls {
            NullPlacement::First => Ordering::Greater,
            NullPlacement::Last => Ordering::Less,
        }),
        (false, false) => None,
    };
    if let Some(ordering) = null_order {
        return ordering;
    }

    let ordering = if mode == ViewSortMode::Numeric {
        match (left, right) {
            (CellValue::Integer(left), CellValue::Integer(right)) => left.cmp(right),
            _ => match (
                numeric_value(left, numeric_profile),
                numeric_value(right, numeric_profile),
            ) {
                (Some(left), Some(right)) => left.total_cmp(&right),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
        }
    } else {
        crate::ops::sort::compare_cells(
            &left.display(),
            &right.display(),
            operation_sort_mode(mode),
            numeric_profile,
        )
    };
    match direction {
        SortDirection::Ascending => ordering,
        SortDirection::Descending => ordering.reverse(),
    }
}

fn operation_sort_mode(mode: ViewSortMode) -> crate::ops::sort::SortMode {
    match mode {
        ViewSortMode::Lexical => crate::ops::sort::SortMode::Lexical,
        ViewSortMode::Natural => crate::ops::sort::SortMode::Natural,
        ViewSortMode::Numeric => crate::ops::sort::SortMode::Numeric,
        #[cfg(feature = "saved-views")]
        ViewSortMode::Date => crate::ops::sort::SortMode::Date,
        #[cfg(not(feature = "saved-views"))]
        ViewSortMode::Date => crate::ops::sort::SortMode::Lexical,
        #[cfg(feature = "saved-views")]
        ViewSortMode::SemanticVersion => crate::ops::sort::SortMode::SemVer,
        #[cfg(not(feature = "saved-views"))]
        ViewSortMode::SemanticVersion => crate::ops::sort::SortMode::Natural,
        #[cfg(feature = "saved-views")]
        ViewSortMode::Ip => crate::ops::sort::SortMode::Ip,
        #[cfg(not(feature = "saved-views"))]
        ViewSortMode::Ip => crate::ops::sort::SortMode::Natural,
        #[cfg(feature = "saved-views")]
        ViewSortMode::Boolean => crate::ops::sort::SortMode::Boolean,
        #[cfg(not(feature = "saved-views"))]
        ViewSortMode::Boolean => crate::ops::sort::SortMode::Lexical,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{
        ColumnDefinition, ColumnSourceIdentity, LogicalType, NumericOperator, RowId, SchemaState,
        SourceGeneration, TypeOrigin, ViewFilter, ViewSort,
    };

    fn fixture() -> (TableDefinition, InMemoryTable) {
        let generation = SourceGeneration::new();
        let column = |ordinal| ColumnDefinition {
            id: ColumnId {
                generation,
                ordinal,
            },
            source_identity: ColumnSourceIdentity::Positional(ordinal as usize),
            display_name: format!("c{ordinal}"),
            source_declared_type: None,
            source_type: LogicalType::Mixed,
            type_origin: TypeOrigin::Inferred,
        };
        let rows = vec![
            Row::new(
                RowId {
                    generation,
                    ordinal: 0,
                },
                vec![CellValue::Integer(2), CellValue::Text("b".to_owned())],
            ),
            Row::new(
                RowId {
                    generation,
                    ordinal: 1,
                },
                vec![CellValue::Null, CellValue::Text("z".to_owned())],
            ),
            Row::new(
                RowId {
                    generation,
                    ordinal: 2,
                },
                vec![
                    CellValue::Text("10".to_owned()),
                    CellValue::Text("a".to_owned()),
                ],
            ),
            Row::new(
                RowId {
                    generation,
                    ordinal: 3,
                },
                vec![CellValue::Integer(2), CellValue::Text("c".to_owned())],
            ),
        ];
        (
            TableDefinition {
                generation,
                columns: vec![column(0), column(1)],
                schema_state: SchemaState::Complete,
                relation: crate::table::RelationMetadata::implicit("test", true),
            },
            InMemoryTable::from_rows(generation, rows).unwrap(),
        )
    }

    #[test]
    fn typed_numeric_sort_keeps_nulls_last_in_both_directions_and_stable_ties() {
        let (definition, base) = fixture();
        for direction in [SortDirection::Ascending, SortDirection::Descending] {
            let query = ViewTransform {
                generation: definition.generation,
                filters: Vec::new(),
                order_by: vec![ViewSort {
                    column: definition.columns[0].id,
                    mode: ViewSortMode::Numeric,
                    direction,
                    nulls: NullPlacement::Last,
                }],
            };
            let result = execute_local_view_transform(&base, &definition, &query, &|_, value| {
                value.display().into_owned()
            })
            .unwrap();
            assert!(matches!(
                result.rows().last().unwrap().cells[0],
                CellValue::Null
            ));
            let tied = result
                .rows()
                .iter()
                .filter(|row| row.cells[0] == CellValue::Integer(2))
                .map(|row| row.id.ordinal)
                .collect::<Vec<_>>();
            assert_eq!(tied, [0, 3]);
        }
    }

    #[test]
    fn typed_numeric_sort_preserves_large_integer_precision() {
        let (definition, _) = fixture();
        let base = InMemoryTable::from_rows(
            definition.generation,
            vec![
                Row::new(
                    RowId {
                        generation: definition.generation,
                        ordinal: 0,
                    },
                    vec![CellValue::Integer(9_007_199_254_740_993)],
                ),
                Row::new(
                    RowId {
                        generation: definition.generation,
                        ordinal: 1,
                    },
                    vec![CellValue::Integer(9_007_199_254_740_992)],
                ),
            ],
        )
        .unwrap();
        let query = ViewTransform {
            generation: definition.generation,
            filters: Vec::new(),
            order_by: vec![ViewSort {
                column: definition.columns[0].id,
                mode: ViewSortMode::Numeric,
                direction: SortDirection::Ascending,
                nulls: NullPlacement::Last,
            }],
        };

        let result = execute_local_view_transform(&base, &definition, &query, &|_, value| {
            value.display().into_owned()
        })
        .unwrap();

        assert_eq!(
            result
                .rows()
                .iter()
                .map(|row| row.id.ordinal)
                .collect::<Vec<_>>(),
            [1, 0]
        );
    }

    #[test]
    fn filter_out_is_exact_negation_and_domains_are_explicit() {
        let (definition, base) = fixture();
        let query = ViewTransform {
            generation: definition.generation,
            filters: vec![ViewFilter {
                column: definition.columns[1].id,
                mode: FilterMode::Out,
                predicate: ViewFilterPredicate::Regex {
                    pattern: "^[ab]$".to_owned(),
                    domain: ValueDomain::Raw,
                },
            }],
            order_by: Vec::new(),
        };
        let result = execute_local_view_transform(&base, &definition, &query, &|_, value| {
            value.display().into_owned()
        })
        .unwrap();
        assert_eq!(
            result
                .rows()
                .iter()
                .map(|row| row.id.ordinal)
                .collect::<Vec<_>>(),
            [1, 3]
        );
    }

    #[test]
    fn local_numeric_queries_use_the_supplied_column_profile() {
        let (definition, _) = fixture();
        let base = InMemoryTable::from_rows(
            definition.generation,
            vec![
                Row::new(
                    RowId {
                        generation: definition.generation,
                        ordinal: 0,
                    },
                    vec![CellValue::Text("2m".to_owned())],
                ),
                Row::new(
                    RowId {
                        generation: definition.generation,
                        ordinal: 1,
                    },
                    vec![CellValue::Text("30".to_owned())],
                ),
            ],
        )
        .unwrap();
        let profile = |_: ColumnId| crate::ops::sort::NumericColumnProfile::time();
        let render = |_: ColumnId, value: &CellValue| value.display().into_owned();
        let sort_query = ViewTransform {
            generation: definition.generation,
            filters: Vec::new(),
            order_by: vec![ViewSort {
                column: definition.columns[0].id,
                mode: ViewSortMode::Numeric,
                direction: SortDirection::Ascending,
                nulls: NullPlacement::Last,
            }],
        };

        let sorted = execute_local_view_transform_with_profiles(
            &base,
            &definition,
            &sort_query,
            &profile,
            &render,
        )
        .unwrap();
        assert_eq!(
            sorted
                .rows()
                .iter()
                .map(|row| row.id.ordinal)
                .collect::<Vec<_>>(),
            [1, 0]
        );

        let filter_query = ViewTransform {
            generation: definition.generation,
            filters: vec![ViewFilter {
                column: definition.columns[0].id,
                mode: FilterMode::In,
                predicate: ViewFilterPredicate::Numeric {
                    operator: NumericOperator::LessThan,
                    operand: 60.0,
                },
            }],
            order_by: Vec::new(),
        };
        let filtered = execute_local_view_transform_with_profiles(
            &base,
            &definition,
            &filter_query,
            &profile,
            &render,
        )
        .unwrap();
        assert_eq!(
            filtered
                .rows()
                .iter()
                .map(|row| row.id.ordinal)
                .collect::<Vec<_>>(),
            [1]
        );
    }

    #[test]
    fn validates_before_store_execution() {
        let (definition, mut base) = fixture();
        let stale = SourceGeneration::new();
        let query = ViewTransform {
            generation: stale,
            ..ViewTransform::default()
        };
        assert_eq!(
            validate_view_transform(&definition, &query),
            Err(ViewTransformValidationError::StaleGeneration)
        );
        assert!(
            execute_view_transform(&mut base, &definition, &query, &|_, value| value
                .display()
                .into_owned())
            .is_err()
        );

        let query = ViewTransform {
            generation: definition.generation,
            filters: vec![ViewFilter {
                column: definition.columns[0].id,
                mode: FilterMode::In,
                predicate: ViewFilterPredicate::Numeric {
                    operator: NumericOperator::Equal,
                    operand: f64::NAN,
                },
            }],
            order_by: Vec::new(),
        };
        assert_eq!(
            validate_view_transform(&definition, &query),
            Err(ViewTransformValidationError::InvalidNumericOperand)
        );

        let mut mismatched_store =
            InMemoryTable::from_text_rows(SourceGeneration::new(), vec![vec!["1".to_owned()]]);
        let query = ViewTransform {
            generation: definition.generation,
            ..ViewTransform::default()
        };
        let error =
            execute_view_transform(&mut mismatched_store, &definition, &query, &|_, value| {
                value.display().into_owned()
            })
            .err()
            .expect("mismatched store generation");
        assert!(error.to_string().contains("table store"));
    }
}
