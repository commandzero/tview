use super::*;

#[cfg(feature = "saved-views")]
#[test]
fn keyed_object_view_fills_the_first_viewport_and_saves_resolved_mode() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repositories.json");
    let entries = (0..80)
        .map(|index| format!(r#""repo-{index}":{{"type":"fs","ordinal":{index}}}"#))
        .collect::<Vec<_>>()
        .join(",");
    std::fs::write(&path, format!("{{{entries}}}")).expect("write");
    let options = crate::ingest::OpenOptions {
        format: crate::ingest::InputFormat::Json,
        lazy_threshold_bytes: 1,
        schema_scan_bytes: 1,
        ..crate::ingest::OpenOptions::default()
    };
    let opened = crate::ingest::JsonAdapter::json()
        .open(
            crate::ingest::source::InputSource::Path(path.clone()),
            &options,
        )
        .expect("open")
        .into_implicit_table()
        .expect("table");
    let view = TableView::from_opened_table(opened, Viewport::new(8, 80)).expect("view");

    assert_eq!(view.visible_rows_vec().len(), 8);
    assert_eq!(
        view.current_column_info()
            .unwrap()
            .canonical_source
            .as_deref(),
        Some("@key")
    );
    let mut view = view;
    view.initialize_source_configuration(options)
        .expect("committed source");
    let yaml = view.to_saved_view_yaml("repositories", "repositories.json", None);
    assert!(yaml.contains("source:\n  format: json\n  object_mode: entries\n"));
    assert!(yaml.contains("\nview: {}\n"));

    let record_options = crate::ingest::OpenOptions {
        format: crate::ingest::InputFormat::Json,
        object_mode: crate::ingest::ObjectMode::Record,
        object_mode_origin: crate::ingest::ObjectModeOrigin::Cli,
        ..crate::ingest::OpenOptions::default()
    };
    let record = crate::ingest::JsonAdapter::json()
        .open(
            crate::ingest::source::InputSource::Path(path),
            &record_options,
        )
        .expect("record open")
        .into_implicit_table()
        .expect("record table");
    let mut record_view =
        TableView::from_opened_table(record, Viewport::new(8, 80)).expect("record view");
    record_view
        .initialize_source_configuration(record_options)
        .expect("committed source");
    assert!(record_view
        .to_saved_view_yaml("record", "repositories.json", None)
        .contains("source:\n  format: json\n  object_mode: record\n"));
}

#[cfg(feature = "saved-views")]
#[test]
fn json_array_saved_view_omits_object_mode() {
    let dir = tempfile::tempdir().expect("tempdir");
    let array_path = dir.path().join("array.json");
    std::fs::write(&array_path, r#"[{"name":"one"}]"#).expect("array write");
    let array = crate::ingest::JsonAdapter::json()
        .open(
            crate::ingest::source::InputSource::Path(array_path),
            &crate::ingest::OpenOptions {
                format: crate::ingest::InputFormat::Json,
                ..crate::ingest::OpenOptions::default()
            },
        )
        .expect("array open")
        .into_implicit_table()
        .expect("array table");
    let array_view = TableView::from_opened_table(array, Viewport::new(8, 80)).expect("array view");
    assert!(!array_view
        .to_saved_view_yaml("array", "array.json", None)
        .contains("object_mode:"));
}

#[test]
fn opened_incremental_table_loads_viewport_then_indexes_navigation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("data.csv");
    std::fs::write(&path, "name,value\na,1\na,2\nc,3\n").expect("write");
    let options = crate::ingest::OpenOptions {
        lazy_threshold_bytes: 0,
        ..crate::ingest::OpenOptions::default()
    };
    let opened = crate::ingest::DelimitedAdapter
        .open(crate::ingest::source::InputSource::Path(path), &options)
        .expect("open")
        .into_implicit_table()
        .expect("table");
    let mut view = TableView::from_opened_table(opened, Viewport::new(1, 2)).expect("view");
    assert_eq!(view.total_row_count(), 1);
    assert!(matches!(view.row_count_state(), RowCount::AtLeast(_)));

    let found = view
        .progressive_search("c", crate::ops::search::SearchDirection::Forward)
        .expect("progressive match");
    assert_eq!(found, Position { row: 2, column: 0 });
    assert_eq!(view.total_row_count(), 3);
    view.goto(found.row, found.column);
    assert_eq!(view.current_raw_cell(), Some("c"));

    view.goto(0, 0);
    assert_eq!(
        view.progressive_skip_to_change(
            crate::ops::skip::Axis::Row,
            crate::ops::skip::Direction::Forward,
            1,
        ),
        Position { row: 2, column: 0 }
    );
}

#[test]
fn goto_bottom_batch_loads_large_incremental_delimited_source() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("large.csv");
    let mut data = String::from("value\n");
    for value in 0..10_000 {
        data.push_str(&format!("{value}\n"));
    }
    std::fs::write(&path, data).expect("write");
    let opened = crate::ingest::DelimitedAdapter
        .open(
            crate::ingest::source::InputSource::Path(path),
            &crate::ingest::OpenOptions {
                lazy_threshold_bytes: 0,
                ..crate::ingest::OpenOptions::default()
            },
        )
        .expect("open")
        .into_implicit_table()
        .expect("table");
    let mut view = TableView::from_opened_table(opened, Viewport::new(1, 1)).expect("view");
    assert_eq!(view.effective_column_widths_cached(), [5]);

    view.goto_bottom();

    assert_eq!(view.row_count_state(), RowCount::Exact(10_000));
    assert_eq!(view.current_raw_cell(), Some("9999"));
    assert_eq!(view.effective_column_widths_cached(), [5]);
}

#[cfg(feature = "saved-views")]
#[test]
fn pending_canonical_saved_column_applies_when_schema_delta_arrives() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("late.json");
    std::fs::write(&path, r#"[{"a":1},{"a":2,"late":3}]"#).expect("write");
    let options = crate::ingest::OpenOptions {
        format: crate::ingest::InputFormat::Json,
        schema_scan_bytes: 1,
        ..crate::ingest::OpenOptions::default()
    };
    let opened = crate::ingest::JsonAdapter::json()
        .open(crate::ingest::source::InputSource::Path(path), &options)
        .expect("open")
        .into_implicit_table()
        .expect("table");
    let mut view = TableView::from_opened_table(opened, Viewport::new(1, 2)).expect("view");
    let saved = crate::saved_views::parse_saved_view_yaml(
        r#"
name: late
filenames: [late.json]
source: {}
view:
  columns:
    /late:
      label: Later
      nulls: first
"#,
    )
    .expect("saved");
    view.install_saved_binding(saved.view.view, true);

    view.goto(1, 0);
    assert_eq!(view.header().unwrap()[1], "Later");
    assert_eq!(view.resolved_null_placement(1), NullPlacement::First);
    assert!(matches!(
        &view.table_definition().unwrap().columns[1].source_identity,
        crate::table::ColumnSourceIdentity::StructuredPath(pointer) if pointer.as_str() == "/late"
    ));
}

#[cfg(feature = "saved-views")]
#[test]
fn pending_structured_sort_and_filter_apply_when_late_column_arrives() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("late.json");
    std::fs::write(&path, r#"[{"a":1},{"a":2,"late":3}]"#).expect("write");
    let options = crate::ingest::OpenOptions {
        format: crate::ingest::InputFormat::Json,
        schema_scan_bytes: 1,
        ..crate::ingest::OpenOptions::default()
    };
    let opened = crate::ingest::JsonAdapter::json()
        .open(crate::ingest::source::InputSource::Path(path), &options)
        .expect("open")
        .into_implicit_table()
        .expect("table");
    let mut view = TableView::from_opened_table(opened, Viewport::new(1, 2)).expect("view");
    let saved = crate::saved_views::parse_saved_view_yaml(
        r#"
name: late operations
filenames: [late.json]
source: {}
view:
  sort:
    - column: /late
      direction: desc
      kind: numeric
  filters:
    - column: /late
      action: in
      kind: numeric
      condition: "> 2"
"#,
    )
    .expect("saved");
    view.install_saved_binding(saved.view.view, true);

    view.goto(1, 0);

    assert_eq!(view.visible_rows_vec(), rows(&[&["2", "3"]]));
    assert_eq!(view.visible_raw_rows_vec(), rows(&[&["2", "3"]]));
}
