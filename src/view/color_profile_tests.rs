use super::*;
use crate::table::{
    CellValue, ColumnDefinition, ColumnId, ColumnSourceIdentity, IndexProgress, LogicalType,
    RelationMetadata, Row, RowId, ScanDirection, ScanProgress, ScanRequest, SchemaDelta,
    SchemaState, TypeOrigin,
};
use ratatui::style::Color;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct CountingColorStore {
    generation: SourceGeneration,
    rows: Vec<Row>,
    indexed: usize,
    exact_scans: Arc<AtomicUsize>,
}

impl TableStore for CountingColorStore {
    fn generation(&self) -> SourceGeneration {
        self.generation
    }

    fn row_count(&self) -> RowCount {
        if self.indexed >= self.rows.len() {
            RowCount::Exact(self.rows.len())
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
        self.exact_scans.fetch_add(1, Ordering::SeqCst);
        if request.direction == ScanDirection::Forward && request.max_rows > 0 {
            self.ensure_indexed_through(RowIndex(
                request.start.0.saturating_add(request.max_rows - 1),
            ))?;
        }
        let mut index = request.start.0;
        let mut visited = 0;
        while visited < request.max_rows {
            let Some(row) = self.rows.get(index) else {
                break;
            };
            visited += 1;
            if visitor.visit(RowIndex(index), row).is_break() {
                return Ok(ScanProgress {
                    visited,
                    next: None,
                    reached_end: false,
                });
            }
            match request.direction {
                ScanDirection::Forward => index += 1,
                ScanDirection::Reverse if index > 0 => index -= 1,
                ScanDirection::Reverse => {
                    return Ok(ScanProgress {
                        visited,
                        next: None,
                        reached_end: true,
                    });
                }
            }
        }
        let reached_end = index >= self.rows.len();
        Ok(ScanProgress {
            visited,
            next: (!reached_end).then_some(RowIndex(index)),
            reached_end,
        })
    }

    fn materialize(&mut self) -> anyhow::Result<InMemoryTable> {
        InMemoryTable::from_rows(self.generation, self.rows.clone())
    }
}

fn counted_view(typed: bool) -> (TableView, Arc<AtomicUsize>) {
    let generation = SourceGeneration::new();
    let exact_scans = Arc::new(AtomicUsize::new(0));
    let rows = [50, 1, 100, 75]
        .into_iter()
        .enumerate()
        .map(|(ordinal, value)| {
            Row::new(
                RowId {
                    generation,
                    ordinal: ordinal as u64,
                },
                vec![if typed {
                    CellValue::Integer(value)
                } else {
                    CellValue::Text(value.to_string())
                }],
            )
        })
        .collect();
    let opened = OpenedTable {
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
                source_type: if typed {
                    LogicalType::Integer
                } else {
                    LogicalType::Text
                },
                type_origin: TypeOrigin::Declared,
            }],
            schema_state: SchemaState::Complete,
            relation: RelationMetadata::implicit("color-test", true),
        },
        store: Box::new(CountingColorStore {
            generation,
            rows,
            indexed: 0,
            exact_scans: exact_scans.clone(),
        }),
        object_mode: None,
        warnings: Vec::new(),
    };
    let mut view = TableView::from_opened_table(opened, Viewport::new(1, 1)).expect("open view");
    install_gradient(&mut view);
    exact_scans.store(0, Ordering::SeqCst);
    (view, exact_scans)
}

fn install_gradient(view: &mut TableView) {
    let saved = crate::saved_views::parse_saved_view_yaml(
        "name: gradient\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    value:\n      colors:\n        - gradient:\n            mode: auto\n            steps: 4\n            colors: ['#000000FF', '#FFFFFFFF']\n",
    )
    .expect("gradient saved view");
    view.install_saved_binding(saved.view.view, true);
}

fn foreground(view: &mut TableView, row: usize) -> Option<Color> {
    view.prepare_conditional_colors(&crate::theme::default_theme());
    let rendered = view.rendered_visible_row(row).expect("visible row");
    view.source_cell_style_context(row, 0, &rendered[0], None)
        .conditional_color
}

#[test]
fn exact_typed_color_facts_survive_resident_scroll_without_another_scan() {
    let (mut view, exact_scans) = counted_view(true);
    assert_eq!(foreground(&mut view, 0), Some(Color::Rgb(85, 85, 85)));
    assert_eq!(
        view.rows().len(),
        1,
        "color profiling must not extend the viewport"
    );

    view.ensure_source_indexed_through(3)
        .expect("append viewport rows");
    view.goto(3, 0);
    let scans_before_repaint = exact_scans.load(Ordering::SeqCst);
    assert_eq!(foreground(&mut view, 3), Some(Color::Rgb(170, 170, 170)));
    assert_eq!(foreground(&mut view, 0), Some(Color::Rgb(85, 85, 85)));
    assert_eq!(exact_scans.load(Ordering::SeqCst), scans_before_repaint);
}

#[test]
fn exact_text_profile_reuses_scan_but_refreshes_resident_scalar_extrema() {
    let (mut view, exact_scans) = counted_view(false);
    assert_eq!(foreground(&mut view, 0), Some(Color::Rgb(0, 0, 0)));

    view.ensure_source_indexed_through(3)
        .expect("append viewport rows");
    view.goto(3, 0);
    let scans_before_repaint = exact_scans.load(Ordering::SeqCst);
    assert_eq!(foreground(&mut view, 0), Some(Color::Rgb(85, 85, 85)));
    assert_eq!(foreground(&mut view, 3), Some(Color::Rgb(170, 170, 170)));
    assert_eq!(exact_scans.load(Ordering::SeqCst), scans_before_repaint);
}

#[test]
fn resident_sort_and_filter_keep_complete_color_domain() {
    let rows = vec![
        vec!["value".to_owned()],
        vec!["50".to_owned()],
        vec!["1".to_owned()],
        vec!["100".to_owned()],
        vec!["75".to_owned()],
    ];
    let mut view = TableView::classify(rows, Viewport::new(4, 1));
    install_gradient(&mut view);
    assert_eq!(foreground(&mut view, 0), Some(Color::Rgb(85, 85, 85)));
    view.sort_current_column(SortMode::Numeric, SortDirection::Descending);
    let sorted = view.visible_rows_vec();
    let position = sorted
        .iter()
        .position(|row| row[0] == "50")
        .expect("50 remains");
    assert_eq!(
        foreground(&mut view, position),
        Some(Color::Rgb(85, 85, 85))
    );
    view.apply_filter(0, FilterMode::In, FilterKind::Text, "50".to_owned())
        .expect("local filter");
    assert_eq!(view.visible_rows_vec(), vec![vec!["50".to_owned()]]);
    assert_eq!(foreground(&mut view, 0), Some(Color::Rgb(85, 85, 85)));
}

#[test]
fn resident_identifier_keys_keep_sorted_complete_domain_after_reordering_and_filtering() {
    let rows = ["value", "beta", "gamma", "alpha"]
        .into_iter()
        .map(|value| vec![value.to_owned()])
        .collect();
    let mut view = TableView::classify(rows, Viewport::new(3, 1));
    let saved = crate::saved_views::parse_saved_view_yaml(
        "name: identifiers\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    value:\n      colors:\n        - identifiers:\n            colors: [red, green]\n",
    )
    .expect("identifier saved view");
    view.install_saved_binding(saved.view.view, true);
    let beta = foreground(&mut view, 0).expect("beta color");
    let alpha = foreground(&mut view, 2).expect("alpha color");
    assert_ne!(
        beta, alpha,
        "different sorted keys choose different families"
    );

    view.sort_current_column(SortMode::Lexical, SortDirection::Descending);
    let sorted = view.visible_rows_vec();
    let beta_position = sorted
        .iter()
        .position(|row| row[0] == "beta")
        .expect("beta remains");
    assert_eq!(foreground(&mut view, beta_position), Some(beta));
    view.apply_filter(0, FilterMode::In, FilterKind::Text, "beta".to_owned())
        .expect("local filter");
    assert_eq!(view.visible_rows_vec(), vec![vec!["beta".to_owned()]]);
    assert_eq!(foreground(&mut view, 0), Some(beta));
}
