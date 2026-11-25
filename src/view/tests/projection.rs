use super::*;
use crate::output::{self, ColorOutput, OutputFormat};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

struct SchemaThenError {
    inner: InMemoryTable,
    late: ColumnDefinition,
    phase: Arc<AtomicU8>,
}

impl TableStore for SchemaThenError {
    fn generation(&self) -> SourceGeneration {
        self.inner.generation()
    }

    fn row_count(&self) -> RowCount {
        self.inner.row_count()
    }

    fn column_count(&self) -> usize {
        if self.phase.load(Ordering::Relaxed) >= 2 {
            2
        } else {
            1
        }
    }

    fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
        self.inner.row(index)
    }

    fn ensure_indexed_through(&mut self, index: RowIndex) -> anyhow::Result<IndexProgress> {
        if self.phase.load(Ordering::Relaxed) == 2 && index.0 == usize::MAX {
            self.phase.store(3, Ordering::Relaxed);
            anyhow::bail!("injected read failure after schema discovery");
        }
        let mut progress = self.inner.ensure_indexed_through(index)?;
        if self.phase.load(Ordering::Relaxed) == 1 {
            self.phase.store(2, Ordering::Relaxed);
            progress.schema_delta.added_columns.push(self.late.clone());
        }
        Ok(progress)
    }

    fn scan_rows(
        &mut self,
        request: ScanRequest,
        visitor: &mut dyn crate::table::RowVisitor,
    ) -> anyhow::Result<ScanProgress> {
        self.inner.scan_rows(request, visitor)
    }

    fn materialize(&mut self) -> anyhow::Result<InMemoryTable> {
        self.inner.materialize()
    }
}

#[test]
fn failed_projection_preserves_discovered_schema_for_live_indexing() {
    let mut opened = query_test_table(&["alpha"], false);
    let generation = opened.generation;
    let phase = Arc::new(AtomicU8::new(0));
    let mut late = opened.definition.columns[0].clone();
    late.id.ordinal = 1;
    late.source_identity = ColumnSourceIdentity::Positional(1);
    late.display_name = "late".to_owned();
    late.source_type = LogicalType::Integer;
    opened.store = Box::new(SchemaThenError {
        inner: InMemoryTable::from_rows(
            generation,
            vec![Row::new(
                RowId {
                    generation,
                    ordinal: 0,
                },
                vec![CellValue::Text("alpha".to_owned()), CellValue::Integer(7)],
            )],
        )
        .unwrap(),
        late,
        phase: phase.clone(),
    });
    let mut view = TableView::from_opened_table(opened, Viewport::new(1, 1)).unwrap();
    phase.store(1, Ordering::Relaxed);

    let error = prepare_projection(&view, OutputFormat::Json)
        .err()
        .expect("projection must report the read failure");
    assert!(error.to_string().contains("injected read failure"));

    view.ensure_source_indexed_through(0).unwrap();
    assert_eq!(view.header().unwrap(), ["value", "late"]);
    assert_eq!(view.rendered_visible_row(0).unwrap(), ["alpha", "7"]);
}

#[test]
fn failed_frozen_export_keeps_live_source_and_screen_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("broken.ndjson");
    std::fs::write(&path, "{\"id\":1}\ninvalid\n").expect("write");
    let options = crate::ingest::OpenOptions {
        format: crate::ingest::InputFormat::Ndjson,
        lazy_threshold_bytes: 1,
        schema_scan_bytes: 1,
        ..crate::ingest::OpenOptions::default()
    };
    let opened = crate::ingest::JsonAdapter::ndjson()
        .open(crate::ingest::source::InputSource::Path(path), &options)
        .expect("open")
        .into_implicit_table()
        .expect("table");
    let view = TableView::from_opened_table(opened, Viewport::new(1, 1)).expect("view");
    let generation = view.source_generation();
    let cursor = view.cursor();
    let viewport = view.viewport();
    let header = view.header().map(<[String]>::to_vec);
    let rows = view.visible_rows_vec();
    assert!(prepare_projection(&view, OutputFormat::Jsonl).is_err());
    assert_eq!(view.source_generation(), generation);
    assert_eq!(view.cursor(), cursor);
    assert_eq!(view.viewport(), viewport);
    assert_eq!(view.header().map(<[String]>::to_vec), header);
    assert_eq!(view.visible_rows_vec(), rows);
}

#[cfg(feature = "saved-views")]
#[test]
fn frozen_export_replays_late_schema_on_next_live_progress() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("late.json");
    std::fs::write(&path, r#"[{"id":1},{"id":2,"late":"text"}]"#).expect("write");
    let options = crate::ingest::OpenOptions {
        format: crate::ingest::InputFormat::Json,
        schema_scan_bytes: 1,
        lazy_threshold_bytes: 1,
        ..crate::ingest::OpenOptions::default()
    };
    let opened = crate::ingest::JsonAdapter::json()
        .open(crate::ingest::source::InputSource::Path(path), &options)
        .expect("open")
        .into_implicit_table()
        .expect("table");
    let mut view = TableView::from_opened_table(opened, Viewport::new(1, 1)).expect("view");
    assert_eq!(view.header(), Some(&["id".to_owned()][..]));
    let saved = crate::saved_views::parse_saved_view_yaml(
        "name: late presentation\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    /late: {format: uppercase}\n",
    )
    .expect("saved");
    view.install_saved_binding(saved.view.view, true);
    let cursor = view.cursor();
    let viewport = view.viewport();
    let frozen = prepare_projection(&view, OutputFormat::Jsonl).expect("frozen");
    assert_eq!(
        frozen
            .output
            .header
            .iter()
            .map(|cell| cell.text.as_str())
            .collect::<Vec<_>>(),
        ["id", "late"]
    );
    assert_eq!(frozen.output.rows[1][1].text, "TEXT");
    assert_eq!(view.header(), Some(&["id".to_owned()][..]));
    assert_eq!(view.cursor(), cursor);
    assert_eq!(view.viewport(), viewport);
    let before = serialize_jsonl(&frozen.output);
    let repeated = prepare_projection(&view, OutputFormat::Jsonl).expect("repeated frozen export");
    let repeated_output = serialize_jsonl(&repeated.output);
    assert_eq!(repeated_output, before);

    view.ensure_source_indexed_through(2)
        .expect("continue viewer");
    assert_eq!(
        view.header(),
        Some(&["id".to_owned(), "late".to_owned()][..])
    );
    assert_eq!(
        view.rendered_visible_row(1).as_deref(),
        Some(&["2".to_owned(), "TEXT".to_owned()][..])
    );
    view.set_current_column_width(3);
    let after = serialize_jsonl(&frozen.output);
    assert_eq!(before, after);
}

fn prepare_projection(
    view: &TableView,
    format: OutputFormat,
) -> anyhow::Result<crate::projection::FrozenProjection> {
    view.prepare_projection(
        output::requirements(format, ColorOutput::Never).expect("requirements"),
        &crate::theme::default_theme(),
    )
}

#[cfg(feature = "saved-views")]
fn serialize_jsonl(projection: &output::PreparedOutput) -> Vec<u8> {
    let mut bytes = Vec::new();
    output::write_prepared(
        OutputFormat::Jsonl,
        ColorOutput::Never,
        projection,
        None,
        &mut bytes,
    )
    .expect("serialize frozen output");
    bytes
}
