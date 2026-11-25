#[cfg(feature = "saved-views")]
mod colors;
mod construction;
mod formats;
mod indexing;
mod navigation;
mod projection;
mod restore;
mod transforms;
mod width;

use super::*;
use crate::ingest::{OpenedTable, SourceAdapter};
use crate::ops::filter::{FilterKind, FilterMode};
use crate::table::{
    CellValue, ColumnDefinition, ColumnId, ColumnSourceIdentity, IndexProgress, LogicalType,
    RelationMetadata, Row, RowCount, RowId, RowIndex, ScanDirection, ScanProgress, ScanRequest,
    SchemaDelta, SchemaState, TypeOrigin,
};
#[cfg(feature = "saved-views")]
use crate::theme::ConditionalValue;

fn rows(values: &[&[&str]]) -> Vec<Vec<String>> {
    values
        .iter()
        .map(|row| row.iter().map(|cell| (*cell).to_owned()).collect())
        .collect()
}

#[cfg(feature = "saved-views")]
fn color_at(view: &mut TableView, row: usize, column: usize) -> Option<ratatui::style::Color> {
    view.prepare_conditional_colors(&crate::theme::default_theme());
    let rendered = view.rendered_visible_row(row)?;
    let source_column = view.source_column_for_visible(column)?;
    view.source_cell_style_context(row, source_column, rendered.get(column)?, None)
        .conditional_color
}

#[derive(Clone)]
struct QueryTestStore {
    generation: SourceGeneration,
    rows: Vec<Row>,
    indexed: usize,
    fail_materialize: bool,
}

impl TableStore for QueryTestStore {
    fn generation(&self) -> SourceGeneration {
        self.generation
    }

    fn row_count(&self) -> RowCount {
        if self.indexed >= self.rows.len() {
            RowCount::Exact(self.rows.len())
        } else if self.indexed == 0 {
            RowCount::Unknown
        } else {
            RowCount::AtLeast(self.indexed)
        }
    }

    fn column_count(&self) -> usize {
        1
    }

    fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
        self.ensure_indexed_through(index)?;
        Ok(self.rows.get(index.0).cloned())
    }

    fn ensure_indexed_through(&mut self, index: RowIndex) -> anyhow::Result<IndexProgress> {
        self.indexed = self
            .indexed
            .max(index.0.saturating_add(1).min(self.rows.len()));
        Ok(IndexProgress {
            row_count: self.row_count(),
            schema_delta: SchemaDelta::default(),
            bytes_scanned: self.indexed as u64,
        })
    }

    fn scan_rows(
        &mut self,
        request: ScanRequest,
        visitor: &mut dyn crate::table::RowVisitor,
    ) -> anyhow::Result<ScanProgress> {
        if request.max_rows == 0 {
            return Ok(ScanProgress {
                visited: 0,
                next: Some(request.start),
                reached_end: false,
            });
        }
        if request.direction == ScanDirection::Forward {
            self.ensure_indexed_through(RowIndex(
                request
                    .start
                    .0
                    .saturating_add(request.max_rows.saturating_sub(1)),
            ))?;
        }
        let mut current = request.start.0;
        let mut visited = 0;
        while visited < request.max_rows {
            let Some(row) = self.rows.get(current) else {
                break;
            };
            visited += 1;
            if visitor.visit(RowIndex(current), row).is_break() {
                return Ok(ScanProgress {
                    visited,
                    next: None,
                    reached_end: false,
                });
            }
            match request.direction {
                ScanDirection::Forward => current += 1,
                ScanDirection::Reverse if current > 0 => current -= 1,
                ScanDirection::Reverse => {
                    return Ok(ScanProgress {
                        visited,
                        next: None,
                        reached_end: true,
                    });
                }
            }
        }
        let reached_end = current >= self.rows.len();
        Ok(ScanProgress {
            visited,
            next: (!reached_end).then_some(RowIndex(current)),
            reached_end,
        })
    }

    fn materialize(&mut self) -> anyhow::Result<InMemoryTable> {
        if self.fail_materialize {
            anyhow::bail!("injected materialization failure");
        }
        InMemoryTable::from_rows(self.generation, self.rows.clone())
    }
}

fn query_test_table(values: &[&str], fail_materialize: bool) -> OpenedTable {
    let generation = SourceGeneration::new();
    let make_rows = |values: &[&str]| {
        values
            .iter()
            .enumerate()
            .map(|(ordinal, value)| {
                Row::new(
                    RowId {
                        generation,
                        ordinal: ordinal as u64,
                    },
                    vec![CellValue::Text((*value).to_owned())],
                )
            })
            .collect::<Vec<_>>()
    };
    let source_rows = make_rows(values);
    OpenedTable {
        generation,
        definition: TableDefinition {
            generation,
            columns: vec![ColumnDefinition {
                id: ColumnId {
                    generation,
                    ordinal: 0,
                },
                source_identity: ColumnSourceIdentity::Delimited {
                    ordinal: 0,
                    name: Some("value".to_owned()),
                },
                display_name: "value".to_owned(),
                source_declared_type: None,
                source_type: LogicalType::Text,
                type_origin: TypeOrigin::Declared,
            }],
            schema_state: SchemaState::Complete,
            relation: RelationMetadata::implicit("query-test", true),
        },
        store: Box::new(QueryTestStore {
            generation,
            rows: source_rows,
            indexed: 0,
            fail_materialize,
        }),
        object_mode: None,
        warnings: Vec::new(),
    }
}
