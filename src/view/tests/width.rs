use super::*;
use unicode_width::UnicodeWidthStr;

#[test]
fn computes_fixed_and_max_widths() {
    let rows = rows(&[&["a", "wide"], &["bb", "中"]]);
    assert_eq!(column_widths(&rows, ColumnWidthMode::Fixed(3), 2), [3, 3]);
    assert_eq!(column_widths(&rows, ColumnWidthMode::Max, 2), [2, 4]);
}

#[test]
fn sampled_mode_width_fits_widest_observed_value() {
    let rows = rows(&[&["store"], &["0"], &["0"], &["0"], &["4279369981"]]);

    assert_eq!(column_widths(&rows, ColumnWidthMode::Mode, 2), [10]);
}

#[test]
fn opened_json_and_delimited_sources_fit_widest_sampled_value() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json_path = dir.path().join("rows.json");
    let csv_path = dir.path().join("rows.csv");
    let values = ["0", "0", "0", "0", "0", "0", "0", "4279369981"];
    let json = values
        .iter()
        .map(|value| format!(r#"{{"store":"{value}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    std::fs::write(&json_path, format!("[{json}]")).expect("json");
    std::fs::write(&csv_path, format!("store\n{}\n", values.join("\n"))).expect("csv");

    let json = crate::ingest::JsonAdapter::json()
        .open(
            crate::ingest::source::InputSource::Path(json_path),
            &crate::ingest::OpenOptions {
                format: crate::ingest::InputFormat::Json,
                ..crate::ingest::OpenOptions::default()
            },
        )
        .expect("open json")
        .into_implicit_table()
        .expect("json table");
    let mut json_view =
        TableView::from_opened_table(json, Viewport::new(20, 2)).expect("json view");

    let csv = crate::ingest::DelimitedAdapter
        .open(
            crate::ingest::source::InputSource::Path(csv_path),
            &crate::ingest::OpenOptions::default(),
        )
        .expect("open csv")
        .into_implicit_table()
        .expect("csv table");
    let mut csv_view = TableView::from_opened_table(csv, Viewport::new(20, 2)).expect("csv view");

    assert_eq!(json_view.effective_column_widths_cached(), [10]);
    assert_eq!(csv_view.effective_column_widths_cached(), [10]);
}

#[test]
fn incremental_indexing_keeps_cached_sampled_width_stable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("rows.json");
    std::fs::write(&path, r#"[{"store":"0"},{"store":"4279369981"}]"#).expect("json");
    let opened = crate::ingest::JsonAdapter::json()
        .open(
            crate::ingest::source::InputSource::Path(path),
            &crate::ingest::OpenOptions {
                format: crate::ingest::InputFormat::Json,
                lazy_threshold_bytes: 1,
                schema_scan_bytes: 1,
                ..crate::ingest::OpenOptions::default()
            },
        )
        .expect("open json")
        .into_implicit_table()
        .expect("json table");
    let mut view = TableView::from_opened_table(opened, Viewport::new(1, 1)).expect("json view");

    assert_eq!(view.effective_column_widths_cached(), [5]);
    view.goto(1, 0);
    assert_eq!(view.effective_column_widths_cached(), [5]);
}

#[test]
fn automatic_width_is_capped_at_eighty_percent_but_explicit_width_is_not() {
    let mut view = TableView::classify(rows(&[&["12345678901234567890"]]), Viewport::new(1, 1));
    view.set_terminal_width(20);

    assert_eq!(view.effective_column_widths_cached(), [16]);
    view.set_current_column_width(24);
    assert_eq!(view.effective_column_widths_cached(), [24]);
}

#[test]
fn column_width_controls_update_effective_widths() {
    let mut view = TableView::classify(
        rows(&[&["Name", "Value"], &["alpha", "1"]]),
        Viewport::new(3, 2),
    );
    view.set_all_column_widths(4);
    assert_eq!(view.effective_column_widths(), [4, 4]);
    view.set_current_column_width(8);
    assert_eq!(view.effective_column_widths()[0], 8);
    view.adjust_current_column_width(-20);
    assert_eq!(view.effective_column_widths()[0], 1);
    view.adjust_column_gap(3);
    assert_eq!(view.column_gap(), 5);
}

#[test]
fn single_column_resize_scales_and_caps_at_content_or_header_width() {
    let header = "A header wider than every value";
    let header_width = UnicodeWidthStr::width(header);
    let mut view = TableView::classify(
        rows(&[&[header], &["1"], &["1234567890"]]),
        Viewport::new(3, 1),
    );

    view.set_current_column_width(5);
    view.adjust_current_column_width(1);
    assert_eq!(view.effective_column_widths(), [6]);
    view.adjust_current_column_width(2);
    assert_eq!(view.effective_column_widths(), [8]);
    view.adjust_current_column_width(20);
    assert_eq!(view.effective_column_widths(), [header_width]);

    view.adjust_current_column_width(-1);
    assert_eq!(
        view.effective_column_widths(),
        [header_width.saturating_sub((header_width / 5).max(1))]
    );
    view.adjust_current_column_width(-20);
    assert_eq!(view.effective_column_widths(), [1]);
    view.adjust_current_column_width(-1);
    assert_eq!(view.effective_column_widths(), [1]);

    view.set_current_column_width(20);
    view.adjust_current_column_width(1);
    let widened = 24.min(header_width);
    assert_eq!(view.effective_column_widths(), [widened]);
    view.adjust_current_column_width(-1);
    assert_eq!(
        view.effective_column_widths(),
        [widened.saturating_sub((widened / 5).max(1))]
    );
}

#[test]
fn cached_widths_do_not_become_explicit_widths() {
    let mut view = TableView::classify(
        rows(&[&["Name", "Value"], &["alpha", "1000"]]),
        Viewport::new(3, 2),
    );

    assert!(view.column_widths.is_empty());
    assert!(view.computed_column_widths_cache.is_empty());

    assert_eq!(view.effective_column_widths_cached(), vec![5, 5]);

    assert!(view.column_widths.is_empty());
    assert_eq!(view.computed_column_widths_cache, vec![5, 5]);

    view.set_current_column_width(8);
    assert_eq!(view.column_widths[0], 8);
    assert!(view.computed_column_widths_cache.is_empty());
}

#[test]
fn filtered_header_indicator_participates_in_widths() {
    let mut view = TableView::classify(
        rows(&[&["Name", "Size"], &["alpha", "1gb"]]),
        Viewport::new(10, 2),
    );
    view.apply_filter(1, FilterMode::In, FilterKind::Numeric, "<2gb".to_owned())
        .expect("apply filter");

    assert_eq!(
        view.rendered_header().expect("header"),
        vec!["Name".to_owned(), "+Size".to_owned()]
    );
    assert!(view.effective_column_widths()[1] >= 5);
}

#[test]
fn auto_widths_expand_only_when_indicators_need_space() {
    let mut view = TableView::classify(
        rows(&[&["Name", "Size"], &["alpha", "1gb"]]),
        Viewport::new(10, 2),
    );

    assert_eq!(view.effective_column_widths(), vec![5, 4]);
    view.goto(0, 1);
    view.sort_current_column(SortMode::Numeric, SortDirection::Ascending);
    assert_eq!(
        view.rendered_header().expect("header"),
        vec!["Name".to_owned(), "▲Size".to_owned()]
    );
    assert_eq!(view.effective_column_widths(), vec![5, 5]);

    view.apply_filter(1, FilterMode::In, FilterKind::Numeric, "<2gb".to_owned())
        .expect("apply filter");

    assert_eq!(
        view.rendered_header().expect("header"),
        vec!["Name".to_owned(), "▲+Size".to_owned()]
    );
    assert_eq!(view.effective_column_widths(), vec![5, 6]);
}

#[test]
fn filtering_and_navigation_do_not_shrink_computed_widths() {
    let mut view = TableView::classify(
        rows(&[
            &["Name", "Value"],
            &["short", "ok"],
            &["very-very-long", "hidden"],
        ]),
        Viewport::new(10, 2),
    );
    view.set_column_width_mode(ColumnWidthMode::Max);
    let widths_before_filter = view.effective_column_widths();

    view.apply_filter(1, FilterMode::In, FilterKind::Text, "ok".to_owned())
        .expect("apply filter");
    let widths_after_filter = view.effective_column_widths();
    view.move_by(1, 0);
    let widths_after_navigation = view.effective_column_widths();

    assert_eq!(widths_after_filter, widths_before_filter);
    assert_eq!(widths_after_navigation, widths_before_filter);
}
