use super::*;
use crate::view::saved_yaml::conditional_value_yaml;

#[test]
fn conditional_colors_apply_without_changing_values() {
    let mut view = TableView::classify(
        rows(&[
            &["Status", "Percent"],
            &["active", "5%"],
            &["idle", "50%"],
            &["down", "95%"],
        ]),
        Viewport::new(10, 2),
    );
    let parsed = crate::saved_views::parse_saved_view_yaml(
        r#"
name: colors
filenames: [data.csv]
source: {}
view:
  columns:
    Status:
      colors:
        - match:
            active: green
    Percent:
      type: number
      colors:
        - range:
            "<10": red
        - gradient:
            mode: auto
            colors: [green, yellow]
"#,
    )
    .expect("parse");
    view.install_saved_binding(parsed.view.view, true);

    assert_eq!(
        color_at(&mut view, 0, 0),
        Some(ratatui::style::Color::Rgb(0, 192, 0))
    );
    assert_eq!(
        color_at(&mut view, 0, 1),
        Some(ratatui::style::Color::Rgb(255, 0, 0))
    );
    assert_eq!(
        color_at(&mut view, 1, 1),
        Some(ratatui::style::Color::Rgb(146, 228, 0))
    );
    assert_eq!(
        view.visible_rows_vec(),
        rows(&[&["active", "5%"], &["idle", "50%"], &["down", "95%"]])
    );
}

#[test]
fn color_demand_profiles_only_dependent_rules_and_uses_offscreen_typed_extrema() {
    let mut opened = query_test_table(&["50", "1", "100"], false);
    let generation = opened.generation;
    opened.definition.columns[0].source_type = LogicalType::Integer;
    opened.store = Box::new(QueryTestStore {
        generation,
        rows: [50, 1, 100]
            .into_iter()
            .enumerate()
            .map(|(ordinal, value)| {
                Row::new(
                    RowId {
                        generation,
                        ordinal: ordinal as u64,
                    },
                    vec![CellValue::Integer(value)],
                )
            })
            .collect(),
        indexed: 0,
        fail_materialize: false,
    });
    let mut view = TableView::from_opened_table(opened, Viewport::new(1, 1)).expect("view");
    let fixed = crate::saved_views::parse_saved_view_yaml(
            "name: fixed\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    value:\n      colors:\n        - match:\n            '50': green\n",
        )
        .expect("fixed saved view");
    view.install_saved_binding(fixed.view.view, true);
    view.take_source_status();
    view.prepare_conditional_colors(&crate::theme::default_theme());
    assert!(
        view.take_source_status().is_none(),
        "match rules need no exact color profile"
    );
    assert_eq!(view.rows.len(), 1);
    let gradient = crate::saved_views::parse_saved_view_yaml(
            "name: gradient\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    value:\n      colors:\n        - gradient:\n            mode: auto\n            steps: 4\n            colors: [black, white]\n",
        )
        .expect("gradient saved view");
    view.install_saved_binding(gradient.view.view, true);
    view.take_source_status();
    assert_eq!(
        view.rows.len(),
        1,
        "binding cannot profile for color demand"
    );
    assert_eq!(
        color_at(&mut view, 0, 0),
        Some(ratatui::style::Color::Rgb(85, 85, 85))
    );
    assert_eq!(
        view.take_source_status().as_deref(),
        Some("Profiled 3 rows")
    );
    assert_eq!(
        view.rows.len(),
        1,
        "exact color profile does not widen resident viewport"
    );
}

#[test]
fn exact_store_identifier_keys_keep_raw_display_misses_without_rendered_fallback() {
    let saved = crate::saved_views::parse_saved_view_yaml(
            "name: identifiers\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    value:\n      type: text\n      format: uppercase\n      colors:\n        - identifiers:\n            colors: [green, red]\n",
        )
        .expect("saved view");
    let mut store = TableView::from_opened_table(
        query_test_table(&["alpha", "beta"], false),
        Viewport::new(1, 1),
    )
    .expect("store view");
    store.install_saved_binding(saved.view.view.clone(), true);
    assert_eq!(
        store.rendered_visible_row(0),
        Some(vec!["ALPHA".to_owned()])
    );
    assert_eq!(
        color_at(&mut store, 0, 0),
        None,
        "raw store key does not match rendered uppercase text"
    );
    assert_eq!(
        store.take_source_status().as_deref(),
        Some("Profiled 2 rows")
    );

    let mut resident = TableView::classify(
        rows(&[&["value"], &["alpha"], &["ALPHA"], &["beta"]]),
        Viewport::new(1, 1),
    );
    resident.install_saved_binding(saved.view.view, true);
    let first = color_at(&mut resident, 0, 0);
    assert!(first.is_some(), "resident keys use rendered values");
    assert_eq!(
        color_at(&mut resident, 1, 0),
        first,
        "equal rendered identifiers share a key"
    );
}

#[test]
fn saved_view_yaml_quotes_string_match_values_that_look_numeric() {
    let mut view = TableView::classify(rows(&[&["Code"], &["10"]]), Viewport::new(10, 1));
    let parsed = crate::saved_views::parse_saved_view_yaml(
        r#"
name: colors
filenames: [data.csv]
source: {}
view:
  columns:
    Code:
      colors:
        - match:
            "10": green
"#,
    )
    .expect("parse");
    view.install_saved_binding(parsed.view.view, true);

    let yaml = view.to_saved_view_yaml("colors", "data.csv", None);

    assert!(yaml.contains("          \"10\": green"));
    assert_eq!(
        conditional_value_yaml(&ConditionalValue::String("a\nb".to_owned())),
        "\"a\\nb\""
    );
}

#[test]
fn rendering_keeps_original_conditional_yaml_values_for_save_and_reopen() {
    let original = crate::saved_views::parse_saved_view_yaml(
        r##"
name: configured
filenames: ["*"]
source: {}
view:
  columns:
    Code:
      colors:
        - match:
            "10": "a,b:c(β)"
            10: green
    Family:
      colors:
        - identifiers:
            colors: ["a,b:c(β)", "#25A39AFF"]
    Scale:
      colors:
        - gradient:
            mode: auto
            steps: 3
            colors: ["a,b:c(β)", green]
"##,
    )
    .expect("original saved view");
    let mut view = TableView::classify(
        rows(&[
            &["Code", "Family", "Scale"],
            &["10", "alpha", "5"],
            &["20", "beta", "10"],
        ]),
        Viewport::new(10, 3),
    );
    view.install_saved_binding(original.view.view.clone(), true);
    let foregrounds = theme_foregrounds(&mut view);
    assert_eq!(
        foregrounds,
        vec![
            Some(ratatui::style::Color::Rgb(37, 163, 154)),
            Some(ratatui::style::Color::Rgb(19, 100, 100)),
            Some(ratatui::style::Color::Rgb(37, 163, 154)),
        ]
    );

    let saved = view.to_saved_view_yaml("configured", "data.csv", None);
    let reopened = crate::saved_views::parse_saved_view_yaml(&saved).expect("reopened view");
    for (name, configured) in &original.view.view.columns {
        assert_eq!(
            reopened.view.view.columns[name].colors, configured.colors,
            "configured conditional rules must survive rendering and saving"
        );
    }
    let code_rules = &reopened.view.view.columns["Code"].colors;
    assert!(
        matches!(&code_rules[0], ConditionalColorRule::Match { entries }
            if matches!((&entries[0].value, &entries[1].value),
                (ConditionalValue::String(string), ConditionalValue::Number(number))
                if string == "10" && *number == 10.0))
    );
}

fn theme_foregrounds(view: &mut TableView) -> Vec<Option<ratatui::style::Color>> {
    let fixture = include_str!("../../../examples/data/config/themes/cmdzro.yml")
        .replace("  hex_teal:", "  \"a,b:c(β)\": \"#25A39AFF\"\n  hex_teal:");
    let theme =
        crate::theme::parse_theme_yaml(&fixture, crate::theme::TerminalColorMode::TrueColor)
            .expect("selected theme");
    view.prepare_conditional_colors(&theme);
    let rendered = view.rendered_visible_row(0).expect("rendered row");
    rendered
        .iter()
        .enumerate()
        .map(|(column, cell)| {
            let source_column = view
                .source_column_for_visible(column)
                .expect("source column");
            view.source_cell_style_context(0, source_column, cell, None)
                .conditional_color
        })
        .collect()
}

#[test]
fn column_info_format_changes_keep_identifier_foreground_on_rendered_value() {
    let mut view = TableView::classify(rows(&[&["Name"], &["alpha"]]), Viewport::new(10, 1));
    let parsed = crate::saved_views::parse_saved_view_yaml(
        r#"
name: identifiers
filenames: [data.csv]
source: {}
view:
  columns:
    Name:
      type: text
      format: uppercase
      colors:
        - identifiers:
            colors: auto
"#,
    )
    .expect("parse");
    view.install_saved_binding(parsed.view.view, true);

    assert_eq!(view.visible_rows_vec(), rows(&[&["ALPHA"]]));
    let first = color_at(&mut view, 0, 0);
    assert!(first.is_some());

    view.apply_current_column_info(ColumnInfoUpdate {
        visible: true,
        alignment: None,
        column_type: ColumnTypeChoice::Text,
        format: ColumnFormatChoice::Lowercase,
        sort: ColumnSortChoice::None,
        nulls: ColumnNullPlacementChoice::Inherited,
        clear_filters: false,
    });

    assert_eq!(view.visible_rows_vec(), rows(&[&["alpha"]]));
    assert_eq!(color_at(&mut view, 0, 0), first);
}

#[test]
fn identifier_colors_are_stable_for_unique_rendered_values() {
    let mut view = TableView::classify(
        rows(&[&["Address"], &["10.0.0.2"], &["10.0.0.1"], &["10.0.0.2"]]),
        Viewport::new(10, 1),
    );
    let parsed = crate::saved_views::parse_saved_view_yaml(
        r#"
name: identifiers
filenames: [data.csv]
source: {}
view:
  columns:
    Address:
      type: ip
      colors:
        - identifiers: {}
"#,
    )
    .expect("parse");
    view.install_saved_binding(parsed.view.view, true);

    let first = color_at(&mut view, 0, 0).expect("first foreground");
    let second = color_at(&mut view, 1, 0).expect("second foreground");
    let repeated = color_at(&mut view, 2, 0).expect("repeated foreground");
    assert_ne!(first, second);
    assert_eq!(first, repeated);
    view.sort_current_column(SortMode::Lexical, SortDirection::Descending);
    assert_eq!(
        view.visible_raw_rows_vec(),
        rows(&[&["10.0.0.2"], &["10.0.0.2"], &["10.0.0.1"]])
    );
    assert_eq!(color_at(&mut view, 0, 0), Some(first));
    assert_eq!(color_at(&mut view, 1, 0), Some(first));
    assert_eq!(color_at(&mut view, 2, 0), Some(second));
}

#[test]
fn restored_view_settings_keep_identifier_foregrounds() {
    let mut view = TableView::classify(
        rows(&[&["Address"], &["10.0.0.2"], &["10.0.0.1"]]),
        Viewport::new(10, 1),
    );
    let parsed = crate::saved_views::parse_saved_view_yaml(
        r#"
name: identifiers
filenames: [data.csv]
source: {}
view:
  columns:
    Address:
      type: ip
      colors:
        - identifiers: {}
"#,
    )
    .expect("parse");
    view.install_saved_binding(parsed.view.view, true);

    let mut restored = TableView::classify(
        rows(&[&["Address"], &["10.0.0.2"], &["10.0.0.1"]]),
        Viewport::new(10, 1),
    );
    restored
        .restore_view_settings_from(&view)
        .expect("compatible view");

    assert_eq!(color_at(&mut restored, 0, 0), color_at(&mut view, 0, 0));
}
