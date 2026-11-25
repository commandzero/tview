use super::*;

#[test]
fn local_view_transform_materializes_active_result_and_restores_base() {
    let opened = query_test_table(&["keep-0", "keep-1", "drop"], false);
    let mut view = TableView::from_opened_table(opened, Viewport::new(1, 1)).expect("view");

    view.apply_filter(0, FilterMode::In, FilterKind::Text, "keep".to_owned())
        .expect("filter");

    assert_eq!(view.rows(), rows(&[&["keep-0"], &["keep-1"]]));
    assert_eq!(view.row_count_state(), RowCount::Exact(2));

    view.clear_filters_for_column(0);
    assert_eq!(view.rows(), rows(&[&["keep-0"], &["keep-1"], &["drop"]]));
    assert_eq!(view.row_count_state(), RowCount::Exact(3));
    assert!(!view.view_transform_is_active());
}

#[test]
fn materialization_failures_preserve_prior_view_and_configuration() {
    let expected = "injected materialization failure";
    let opened = query_test_table(&["b", "a"], true);
    let mut view = TableView::from_opened_table(opened, Viewport::new(1, 1)).expect("view");
    let prior_rows = view.rows.clone();
    let prior_cursor = view.cursor;
    let prior_viewport = view.viewport;
    let prior_query = view.active_view_transform.clone();

    view.sort_current_column(SortMode::Lexical, SortDirection::Ascending);

    assert_eq!(view.rows, prior_rows);
    assert_eq!(view.cursor, prior_cursor);
    assert_eq!(view.viewport, prior_viewport);
    assert_eq!(view.active_view_transform, prior_query);
    assert!(view.sort_keys.is_empty());
    assert!(view.filters.is_empty());
    assert!(view.take_source_status().unwrap().contains(expected));

    view.apply_filter(0, FilterMode::In, FilterKind::Text, "a".to_owned())
        .expect("filter input");
    assert_eq!(view.rows, prior_rows);
    assert_eq!(view.cursor, prior_cursor);
    assert_eq!(view.viewport, prior_viewport);
    assert_eq!(view.active_view_transform, prior_query);
    assert!(view.sort_keys.is_empty());
    assert!(view.filters.is_empty());
    assert!(view.take_source_status().unwrap().contains(expected));
}

#[test]
fn sorts_data_rows_by_current_column() {
    let mut view = TableView::classify(
        rows(&[&["Name", "Value"], &["b", "10"], &["a", "2"]]),
        Viewport::new(10, 2),
    );
    view.goto(0, 0);
    view.sort_current_column(SortMode::Lexical, SortDirection::Ascending);
    assert_eq!(view.rows(), rows(&[&["a", "2"], &["b", "10"]]));
}

#[test]
fn filter_in_keeps_matching_visible_rows_without_mutating_backing_rows() {
    let mut view = TableView::classify(
        rows(&[
            &["Name", "Size"],
            &["alpha", "1gb"],
            &["beta", "2gb"],
            &["gamma", "3gb"],
        ]),
        Viewport::new(10, 2),
    );
    view.goto(0, 0);
    view.apply_filter(0, FilterMode::In, FilterKind::Text, "a".to_owned())
        .expect("apply filter");

    assert_eq!(view.row_count(), 3);
    assert_eq!(view.total_row_count(), 3);
    assert_eq!(
        view.rows(),
        rows(&[&["alpha", "1gb"], &["beta", "2gb"], &["gamma", "3gb"]])
    );
    assert_eq!(
        view.visible_rows_vec(),
        rows(&[&["alpha", "1gb"], &["beta", "2gb"], &["gamma", "3gb"]])
    );
}

#[test]
fn filter_out_and_multiple_filters_reduce_visible_rows() {
    let mut view = TableView::classify(
        rows(&[
            &["Name", "Size"],
            &["alpha", "1gb"],
            &["beta", "2gb"],
            &["gamma", "3gb"],
        ]),
        Viewport::new(10, 2),
    );
    view.apply_filter(0, FilterMode::Out, FilterKind::Text, "beta".to_owned())
        .expect("apply text filter");
    view.apply_filter(1, FilterMode::In, FilterKind::Numeric, ">=2gb".to_owned())
        .expect("apply numeric filter");

    assert_eq!(view.visible_rows_vec(), rows(&[&["gamma", "3gb"]]));
}

#[test]
fn filters_clamp_cursor_and_can_be_cleared() {
    let mut view = TableView::classify(
        rows(&[&["Name"], &["alpha"], &["beta"], &["gamma"]]),
        Viewport::new(1, 1),
    );
    view.goto(2, 0);
    view.apply_filter(0, FilterMode::In, FilterKind::Text, "alpha".to_owned())
        .expect("apply filter");

    assert_eq!(view.cursor(), Position { row: 0, column: 0 });
    assert_eq!(view.visible_rows_vec(), rows(&[&["alpha"]]));

    view.clear_filters_for_column(0);
    assert_eq!(view.row_count(), 3);
}

#[test]
fn filter_header_indicators_show_mode_and_multiples() {
    let mut view = TableView::classify(
        rows(&[
            &["Name", "Size"],
            &["alpha", "1gb"],
            &["beta", "2gb"],
            &["gamma", "3gb"],
        ]),
        Viewport::new(10, 2),
    );

    view.apply_filter(0, FilterMode::Out, FilterKind::Text, "beta".to_owned())
        .expect("apply out filter");
    view.apply_filter(1, FilterMode::In, FilterKind::Numeric, ">=1gb".to_owned())
        .expect("apply in filter");
    assert_eq!(
        view.rendered_header().expect("header"),
        vec!["-Name".to_owned(), "+Size".to_owned()]
    );

    view.apply_filter(0, FilterMode::In, FilterKind::Text, "a".to_owned())
        .expect("apply second name filter");
    assert_eq!(
        view.rendered_header().expect("header"),
        vec!["±Name".to_owned(), "+Size".to_owned()]
    );
}

#[test]
fn sorting_preserves_active_filters() {
    let mut view = TableView::classify(
        rows(&[
            &["Name", "Size"],
            &["gamma", "3gb"],
            &["alpha", "1gb"],
            &["beta", "2gb"],
        ]),
        Viewport::new(10, 2),
    );
    view.apply_filter(1, FilterMode::In, FilterKind::Numeric, ">=2gb".to_owned())
        .expect("apply filter");
    view.goto(0, 0);
    view.sort_current_column(SortMode::Lexical, SortDirection::Ascending);

    assert_eq!(
        view.visible_rows_vec(),
        rows(&[&["beta", "2gb"], &["gamma", "3gb"]])
    );
}

#[test]
fn multi_level_sort_tracks_primary_key_and_header_markers() {
    let mut view = TableView::classify(
        rows(&[
            &["Name", "Shard"],
            &["b", "2"],
            &["a", "2"],
            &["c", "1"],
            &["a", "1"],
        ]),
        Viewport::new(10, 2),
    );

    view.goto(0, 1);
    view.sort_current_column(SortMode::Numeric, SortDirection::Ascending);
    view.goto(0, 0);
    view.sort_current_column(SortMode::Lexical, SortDirection::Ascending);

    assert_eq!(
        view.rows(),
        rows(&[&["a", "1"], &["a", "2"], &["b", "2"], &["c", "1"]])
    );
    assert_eq!(
        view.sort_keys(),
        &[
            ActiveSortKey {
                column: 0,
                mode: SortMode::Lexical,
                direction: SortDirection::Ascending,
                nulls: NullPlacement::Last,
            },
            ActiveSortKey {
                column: 1,
                mode: SortMode::Numeric,
                direction: SortDirection::Ascending,
                nulls: NullPlacement::Last,
            },
        ]
    );
    assert_eq!(
        view.rendered_header().expect("header"),
        vec!["▲Name".to_owned(), "▲Shard".to_owned()]
    );

    view.clear_current_column_sort();
    assert_eq!(view.sort_keys().len(), 1);
    assert_eq!(
        view.rendered_header().expect("header"),
        vec!["Name".to_owned(), "▲Shard".to_owned()]
    );
}

#[test]
fn repeated_sort_toggles_primary_key_and_keeps_last_three_sorts() {
    let mut view = TableView::classify(
        rows(&[
            &["A", "B", "C", "D"],
            &["b", "2", "x", "m"],
            &["a", "1", "y", "n"],
        ]),
        Viewport::new(10, 4),
    );

    for column in 0..4 {
        view.goto(0, column);
        view.sort_current_column(SortMode::Lexical, SortDirection::Ascending);
    }

    assert_eq!(
        view.sort_keys()
            .iter()
            .map(|key| key.column)
            .collect::<Vec<_>>(),
        vec![3, 2, 1]
    );

    view.sort_current_column(SortMode::Lexical, SortDirection::Ascending);
    assert_eq!(
        view.sort_keys()
            .iter()
            .map(|key| key.column)
            .collect::<Vec<_>>(),
        vec![2, 1]
    );
}

#[test]
fn opened_typed_table_queries_preserve_identity_and_null_policy() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("data.json");
    std::fs::write(&path, r#"[{"n":2},{"n":null},{"n":1}]"#).expect("write");
    let options = crate::ingest::OpenOptions {
        format: crate::ingest::InputFormat::Json,
        ..crate::ingest::OpenOptions::default()
    };
    let opened = crate::ingest::JsonAdapter::json()
        .open(crate::ingest::source::InputSource::Path(path), &options)
        .expect("open")
        .into_implicit_table()
        .expect("table");
    let mut view = TableView::from_opened_table(opened, Viewport::new(3, 1)).expect("view");
    view.goto(2, 0);
    view.set_view_null_placement(Some(NullPlacement::First));
    view.sort_current_column(SortMode::Numeric, SortDirection::Ascending);
    assert_eq!(view.visible_raw_rows_vec(), rows(&[&[""], &["1"], &["2"]]));
    assert_eq!(view.current_raw_cell(), Some("1"));
    assert_eq!(
        view.active_view_transform().unwrap().order_by[0].nulls,
        NullPlacement::First
    );

    view.clear_current_column_sort();
    assert_eq!(view.visible_raw_rows_vec(), rows(&[&["2"], &[""], &["1"]]));
    assert_eq!(view.current_raw_cell(), Some("1"));

    view.goto(0, 0);
    view.set_mark();
    view.apply_filter(0, FilterMode::In, FilterKind::Text, "1".to_owned())
        .expect("filter");
    assert_eq!(view.visible_raw_rows_vec(), rows(&[&["1"]]));
    view.goto_mark();
    assert_eq!(view.current_raw_cell(), Some("1"));
    view.clear_filters_for_column(0);
    view.goto_mark();
    assert_eq!(view.current_raw_cell(), Some("2"));
}
