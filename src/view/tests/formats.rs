use super::*;

#[test]
fn current_cell_match_uses_case_insensitive_query() {
    let view = TableView::classify(
        rows(&[&["Name", "Value"], &["alpha", "1"]]),
        Viewport::new(10, 2),
    );

    assert!(view.current_cell_matches("ALP"));
    assert!(!view.current_cell_matches(""));
}

#[cfg(feature = "saved-views")]
#[test]
fn applies_saved_column_width_alignment_and_visibility() {
    let mut view = TableView::classify(
        rows(&[&["Name", "Count", "Hidden"], &["alpha", "1000", "x"]]),
        Viewport::new(10, 3),
    );
    let parsed = crate::saved_views::parse_saved_view_yaml(
        r#"
name: counts
filenames: [counts.csv]
source: {}
view:
  columns:
    Count:
      type: integer
      width: 12
    Hidden:
      visible: false
"#,
    )
    .expect("parse");
    view.install_saved_binding(parsed.view.view, true);

    assert_eq!(view.column_count(), 2);
    assert_eq!(view.effective_column_widths(), vec![5, 12]);
    assert_eq!(
        view.column_alignment_override(1),
        Some(ColumnAlignment::Right)
    );
    assert_eq!(view.visible_rows_vec(), rows(&[&["alpha", "1000"]]));
    assert!(view.hidden_boundary_after_last());
}

#[cfg(feature = "saved-views")]
#[test]
fn applies_saved_display_formats_without_mutating_raw_rows() {
    let mut view = TableView::classify(
        rows(&[
            &["Name", "Locale", "Mask", "Flag"],
            &["alpha", "1234.50", "1234.5", "yes"],
        ]),
        Viewport::new(10, 4),
    );
    let parsed = crate::saved_views::parse_saved_view_yaml(
        r##"
name: display
filenames: [display.csv]
source: {}
view:
  locale: de_DE
  columns:
    Name:
      type: text
      format: uppercase
    Locale:
      type: float
      format: locale
    Mask:
      type: float
      format: mask
      mask: "#,##0.00"
    Flag:
      type: word
      format: bit
"##,
    )
    .expect("parse");
    view.install_saved_binding(parsed.view.view, true);

    assert_eq!(
        view.visible_rows_vec(),
        rows(&[&["ALPHA", "1.234,50", "1,234.50", "1"]])
    );
    assert_eq!(
        view.visible_raw_rows_vec(),
        rows(&[&["alpha", "1234.50", "1234.5", "yes"]])
    );
}

#[cfg(feature = "saved-views")]
#[test]
fn search_and_text_filter_match_raw_or_rendered_values() {
    let mut view = TableView::classify(
        rows(&[&["Count"], &["1000"], &["2000"]]),
        Viewport::new(10, 1),
    );
    let parsed = crate::saved_views::parse_saved_view_yaml(
        r#"
name: counts
filenames: [counts.csv]
source: {}
view:
  columns:
    Count:
      type: integer
      format: locale
"#,
    )
    .expect("parse");
    let mut config = parsed.view.view;
    config.locale = Some("en_US".to_owned());
    view.install_saved_binding(config, true);

    assert_eq!(
        view.search_rows_vec(),
        rows(&[&["1000\n1,000"], &["2000\n2,000"]])
    );
    assert!(view.visible_cell_matches_search_query(0, 0, "1000"));
    assert!(view.visible_cell_matches_search_query(0, 0, "1,000"));
    view.apply_filter(0, FilterMode::In, FilterKind::Text, "1,000".to_owned())
        .expect("filter");
    assert_eq!(view.visible_raw_rows_vec(), rows(&[&["1000"]]));
    assert_eq!(view.visible_rows_vec(), rows(&[&["1,000"]]));
}

#[cfg(feature = "saved-views")]
#[test]
fn saved_type_metadata_enables_date_semver_and_ip_sorting() {
    assert_saved_type_sort(
        &[
            &["Created"],
            &["2024-01-01T00:00:00Z"],
            &["2023-12-31T23:00:00Z"],
        ],
        r#"
name: dates
filenames: [dates.csv]
source: {}
view:
  columns:
    Created:
      type: date
"#,
        &[&["2023-12-31T23:00:00Z"], &["2024-01-01T00:00:00Z"]],
    );
    assert_saved_type_sort(
        &[&["Version"], &["1.10.0"], &["1.2.0"]],
        r#"
name: versions
filenames: [versions.csv]
source: {}
view:
  columns:
    Version:
      type: semver
"#,
        &[&["1.2.0"], &["1.10.0"]],
    );
    assert_saved_type_sort(
        &[&["Address"], &["2001:db8::1"], &["10.0.0.2"]],
        r#"
name: ips
filenames: [ips.csv]
source: {}
view:
  columns:
    Address:
      type: ip
"#,
        &[&["10.0.0.2"], &["2001:db8::1"]],
    );
}

#[cfg(feature = "saved-views")]
fn assert_saved_type_sort(input: &[&[&str]], yaml: &str, expected: &[&[&str]]) {
    let mut view = TableView::classify(rows(input), Viewport::new(10, 1));
    let parsed = crate::saved_views::parse_saved_view_yaml(yaml).expect("parse");
    view.install_saved_binding(parsed.view.view, true);
    view.sort_current_column(SortMode::Lexical, SortDirection::Ascending);
    assert_eq!(view.visible_raw_rows_vec(), rows(expected));
}
