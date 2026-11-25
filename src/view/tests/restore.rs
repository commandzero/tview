use super::*;

#[test]
fn captures_and_applies_reload_state() {
    let mut view = TableView::classify(
        rows(&[&["Name", "Value"], &["alpha", "1"], &["beta", "2"]]),
        Viewport::new(1, 1),
    );
    view.goto(1, 1);
    let state = ReloadState::capture(
        &view,
        ColumnWidthMode::Mode,
        2,
        vec![5, 6],
        Some("beta".to_owned()),
    );
    let mut reloaded = TableView::classify(
        rows(&[&["Name", "Value"], &["alpha", "1"], &["beta", "2"]]),
        Viewport::new(1, 1),
    );
    state.apply_to(&mut reloaded);
    assert_eq!(reloaded.cursor(), Position { row: 1, column: 1 });
    assert_eq!(state.search.as_deref(), Some("beta"));
}

#[test]
fn reload_settings_follow_structured_source_identity_after_reordering() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("data.json");
    std::fs::write(&path, r#"[{"a":1,"b":2}]"#).expect("first source");
    let options = crate::ingest::OpenOptions {
        format: crate::ingest::InputFormat::Json,
        ..crate::ingest::OpenOptions::default()
    };
    let opened = crate::ingest::JsonAdapter::json()
        .open(
            crate::ingest::source::InputSource::Path(path.clone()),
            &options,
        )
        .expect("open first")
        .into_implicit_table()
        .expect("table");
    let mut previous =
        TableView::from_opened_table(opened, Viewport::new(5, 2)).expect("first view");
    previous.goto(0, 1);
    previous.set_current_column_width(17);
    previous.set_mark();
    let old_generation = previous.source_generation();

    std::fs::write(&path, r#"[{"b":2,"a":1}]"#).expect("replacement source");
    let opened = crate::ingest::JsonAdapter::json()
        .open(
            crate::ingest::source::InputSource::Path(path.clone()),
            &options,
        )
        .expect("open replacement")
        .into_implicit_table()
        .expect("table");
    let mut reloaded =
        TableView::from_opened_table(opened, Viewport::new(5, 2)).expect("replacement view");
    reloaded
        .restore_view_settings_from(&previous)
        .expect("compatible view");

    assert_ne!(reloaded.source_generation(), old_generation);
    assert_eq!(reloaded.cursor().column, 0, "cursor follows canonical /b");
    assert_eq!(reloaded.column_widths[0], 17, "width follows canonical /b");
    assert!(
        reloaded.mark().is_none(),
        "row identity is generation-scoped"
    );
}

#[test]
fn unrelated_structured_column_does_not_inherit_hidden_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("fields.json");
    std::fs::write(&path, r#"[{"old":1,"stable":2}]"#).expect("source");
    let options = crate::ingest::OpenOptions {
        format: crate::ingest::InputFormat::Json,
        ..crate::ingest::OpenOptions::default()
    };
    let source = crate::ingest::source::InputSource::Path(path.clone());
    let opened = crate::ingest::open_source(source.clone(), &options)
        .expect("open")
        .into_implicit_table()
        .expect("table");
    let mut previous = TableView::from_opened_table(opened, Viewport::new(5, 2)).expect("view");
    previous.goto(0, 0);
    previous.hide_current_column();
    previous.set_current_column_width(17);

    std::fs::write(&path, r#"[{"new":3,"stable":2}]"#).expect("replacement");
    let opened = crate::ingest::open_source(source, &options)
        .expect("reopen")
        .into_implicit_table()
        .expect("table");
    let mut replacement =
        TableView::from_opened_table(opened, Viewport::new(5, 2)).expect("replacement view");
    replacement
        .restore_view_settings_from(&previous)
        .expect("compatible state");
    assert_eq!(replacement.visible_rows_vec(), rows(&[&["3", "2"]]));
    assert_eq!(replacement.column_count(), 2);
    assert_ne!(replacement.effective_column_widths()[0], 17);
}

#[cfg(feature = "saved-views")]
#[test]
fn resident_source_labels_survive_override_and_restoration_without_aliasing() {
    let saved = crate::saved_views::parse_saved_view_yaml(
        "name: count\nfilenames: []\nsource: {}\nview:\n  columns:\n    count: {label: Total}\n  sort:\n    - {column: count, direction: asc, kind: numeric}\n",
    )
    .expect("saved view");
    let source = rows(&[&["count"], &["10"], &["2"], &["1"]]);
    let mut view = TableView::classify(source.clone(), Viewport::new(10, 1));
    view.install_saved_binding(saved.view.view, true);
    assert_eq!(view.header().expect("header"), ["Total"]);
    assert_eq!(
        view.visible_raw_rows_vec(),
        rows(&[&["1"], &["2"], &["10"]])
    );
    assert!(view.take_saved_binding_warnings().is_empty());

    let mut restored = TableView::classify(source.clone(), Viewport::new(10, 1));
    restored
        .restore_view_settings_from(&view)
        .expect("compatible resident view");
    assert_eq!(restored.header().expect("header"), ["Total"]);
    assert_eq!(
        restored.visible_raw_rows_vec(),
        rows(&[&["1"], &["2"], &["10"]])
    );

    let filter = crate::saved_views::parse_saved_view_yaml(
        "name: source\nfilenames: []\nsource: {}\nview:\n  filters:\n    - {column: count, action: in, kind: numeric, condition: '>2'}\n",
    )
    .expect("source-label filter");
    let mut alias = TableView::classify(source, Viewport::new(10, 1));
    alias
        .restore_view_settings_from(&view)
        .expect("compatible resident view");
    restored.install_saved_binding(filter.view.view, true);
    assert_eq!(restored.visible_raw_rows_vec(), rows(&[&["10"]]));
    assert!(restored.take_saved_binding_warnings().is_empty());

    let filter = crate::saved_views::parse_saved_view_yaml(
        "name: alias\nfilenames: []\nsource: {}\nview:\n  filters:\n    - {column: Total, action: in, kind: numeric, condition: '>2'}\n",
    )
    .expect("alias filter");
    alias.install_saved_binding(filter.view.view, true);
    assert_eq!(
        alias.visible_raw_rows_vec(),
        rows(&[&["1"], &["2"], &["10"]])
    );
    let warnings = alias.take_saved_binding_warnings();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].field, "view.filters[0].column");
}
