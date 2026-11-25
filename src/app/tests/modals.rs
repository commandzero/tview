use super::*;
use crate::app::source_config::parse_source_operand;

#[test]
fn filter_prompt_defaults_from_column_type_and_tabs_enabled_kinds() {
    let mut app = app_with_rows(rows(&[&["Name", "Size"], &["alpha", "1gb"]]));

    app.handle_key(key(KeyCode::Char('f'))).expect("filter in");
    let prompt = app.filter_prompt.as_ref().expect("text prompt");
    assert_eq!(prompt.selected_kind, FilterKind::Text);
    assert!(!prompt.enabled_kinds.contains(&FilterKind::Numeric));

    app.handle_key(key(KeyCode::Esc)).expect("cancel");
    app.view.goto(0, 1);
    app.handle_key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT))
        .expect("filter out");
    let prompt = app.filter_prompt.as_ref().expect("numeric prompt");
    assert_eq!(prompt.mode, FilterMode::Out);
    assert_eq!(prompt.selected_kind, FilterKind::Numeric);
    assert!(prompt.enabled_kinds.contains(&FilterKind::Text));
    assert!(prompt.enabled_kinds.contains(&FilterKind::Regex));
    assert!(prompt.enabled_kinds.contains(&FilterKind::Numeric));

    app.handle_key(key(KeyCode::Tab)).expect("cycle");
    app.handle_key(key(KeyCode::Char('g'))).expect("type");
    let prompt = app.filter_prompt.as_ref().expect("cycled prompt");
    assert_eq!(prompt.selected_kind, FilterKind::Text);
    assert_eq!(prompt.input, "g");
}

#[test]
fn filter_prompt_applies_cancels_and_clears_filters() {
    let mut app = app_with_rows(rows(&[&["Name"], &["alpha"], &["beta"]]));

    app.handle_key(key(KeyCode::Char('f'))).expect("filter in");
    app.handle_key(key(KeyCode::Char('a'))).expect("type");
    app.handle_key(key(KeyCode::Esc)).expect("cancel");
    assert_eq!(app.view.row_count(), 2);

    app.handle_key(key(KeyCode::Char('f'))).expect("filter in");
    for ch in "alp".chars() {
        app.handle_key(key(KeyCode::Char(ch))).expect("type");
    }
    app.handle_key(key(KeyCode::Enter)).expect("submit");
    assert_eq!(app.view.visible_rows_vec(), rows(&[&["alpha"]]));

    app.handle_key(key(KeyCode::Char('f'))).expect("filter in");
    app.handle_key(key(KeyCode::Enter)).expect("clear");
    assert_eq!(app.view.row_count(), 2);
}

#[test]
fn column_info_modal_edits_format_and_sort() {
    let mut app = app_with_rows(rows(&[&["Name"], &["beta"], &["alpha"]]));

    app.handle_key(key(KeyCode::Char('i')))
        .expect("column info");
    assert_eq!(app.popup, Some(ui::Popup::ColumnInfo));
    let popup = app.column_info.as_ref().expect("modal").popup();
    assert!(popup.summary.contains("Name"));
    assert!(popup
        .sections
        .iter()
        .any(|section| section.header == "Type"));
    assert!(popup
        .sections
        .iter()
        .find(|section| section.header == "Filters")
        .is_some_and(|section| section.options.is_empty() && section.details == ["None"]));

    for _ in 0..3 {
        app.handle_key(key(KeyCode::Tab)).expect("next group");
    }
    app.handle_key(key(KeyCode::Down)).expect("uppercase");
    app.handle_key(key(KeyCode::Tab)).expect("sort group");
    app.handle_key(key(KeyCode::Down)).expect("ascending");
    app.handle_key(key(KeyCode::Enter)).expect("save");

    assert_eq!(app.popup, None);
    assert_eq!(
        app.view.rendered_header().expect("header"),
        vec!["▲Name".to_owned()]
    );
    assert_eq!(app.view.visible_rows_vec(), rows(&[&["ALPHA"], &["BETA"]]));
}

#[test]
fn column_info_modal_displays_and_clears_filters() {
    let mut app = app_with_rows(rows(&[&["Name"], &["alpha"], &["beta"]]));
    app.view
        .apply_filter(0, FilterMode::In, FilterKind::Text, "alp".to_owned())
        .expect("apply filter");

    app.handle_key(key(KeyCode::Char('i')))
        .expect("column info");
    let popup = app.column_info.as_ref().expect("modal").popup();
    assert!(popup
        .sections
        .iter()
        .flat_map(|section| section.details.iter())
        .any(|detail| detail.contains("in text alp")));

    for _ in 0..5 {
        app.handle_key(key(KeyCode::Tab)).expect("next group");
    }
    app.handle_key(key(KeyCode::Down)).expect("clear filters");
    app.handle_key(key(KeyCode::Enter)).expect("save");

    assert_eq!(app.popup, None);
    assert_eq!(app.view.row_count(), 2);
    assert!(!app.view.column_has_filter(0));
}

#[test]
fn source_filter_null_text_requires_an_explicit_null_operator() {
    assert_eq!(
        parse_source_operand("null"),
        crate::table::SourceOperand::Text("null".to_owned())
    );
    assert_eq!(
        parse_source_operand("NULL"),
        crate::table::SourceOperand::Text("NULL".to_owned())
    );
}

#[test]
fn source_config_surfaces_unresolvable_columns_without_persisting_them() {
    let file = tempfile::NamedTempFile::new().expect("csv fixture");
    std::fs::write(file.path(), "name\nalpha\n").expect("csv");
    let mut app = app_for_source(file.path().to_path_buf(), ingest::OpenOptions::default());
    let original_rows = app.view.visible_rows_vec();
    app.open_source_config_modal();
    app.source_modal
        .as_mut()
        .expect("source modal")
        .draft
        .filters
        .push(crate::table::SourceFilter {
            scope: crate::table::SourceFilterScope::Column(crate::table::ColumnId {
                generation: crate::table::SourceGeneration::new(),
                ordinal: 0,
            }),
            operator: crate::table::SourceFilterOperator::Equal,
            operand: Some(crate::table::SourceOperand::Text("alpha".to_owned())),
        });

    app.handle_key(key(KeyCode::Enter)).expect("apply draft");

    assert_eq!(app.popup, Some(ui::Popup::SourceConfig));
    assert!(app
        .view
        .committed_open_options()
        .expect("committed")
        .source_filters
        .is_empty());
    assert_eq!(app.view.visible_rows_vec(), original_rows);
}

#[test]
fn source_filter_editor_uses_human_readable_operator_names() {
    let file = tempfile::NamedTempFile::new().expect("csv fixture");
    std::fs::write(file.path(), "name,value\nalpha,1\n").expect("csv");
    let mut app = app_for_source(file.path().to_path_buf(), ingest::OpenOptions::default());
    app.open_source_config_modal();
    let modal = app.source_modal.as_mut().expect("source modal");
    modal.editing_filter = true;
    modal.operator = crate::table::SourceFilterOperator::LessThanOrEqual;

    let body = app.source_modal_body();

    assert!(body.contains("less than or equal"));
    assert!(!body.contains("LessThanOrEqual"));
}

#[test]
fn applying_an_unchanged_source_draft_is_a_no_op() {
    let file = tempfile::NamedTempFile::new().expect("csv fixture");
    std::fs::write(file.path(), "name,value\nalpha,1\nbeta,2\n").expect("csv");
    let mut app = app_for_source(file.path().to_path_buf(), ingest::OpenOptions::default());
    let active = app
        .view
        .active_source_query()
        .expect("active source query")
        .clone();

    app.open_source_config_modal();
    app.handle_key(key(KeyCode::Enter))
        .expect("apply unchanged draft");

    assert_eq!(app.popup, None);
    assert!(!app.view.source_query_is_pending());
    assert_eq!(app.view.active_source_query(), Some(&active));
    assert_eq!(
        app.view.committed_open_options().expect("committed").limit,
        None
    );
}

#[test]
fn applying_an_unbounded_source_draft_does_not_persist_the_limit_sentinel() {
    let file = tempfile::NamedTempFile::new().expect("csv fixture");
    std::fs::write(file.path(), "name\nalpha\nbeta\n").expect("csv");
    let mut app = app_for_source(file.path().to_path_buf(), ingest::OpenOptions::default());
    app.open_source_config_modal();
    let column = app.view.table_definition().expect("definition").columns[0].id;
    app.source_modal
        .as_mut()
        .expect("source modal")
        .draft
        .filters
        .push(crate::table::SourceFilter {
            scope: crate::table::SourceFilterScope::Column(column),
            operator: crate::table::SourceFilterOperator::Equal,
            operand: Some(crate::table::SourceOperand::Text("alpha".to_owned())),
        });

    app.handle_key(key(KeyCode::Enter)).expect("apply draft");
    app.view.await_latest_source_query().expect("source result");

    assert_eq!(
        app.view.committed_open_options().expect("committed").limit,
        None
    );
    assert_eq!(
        app.view
            .committed_open_options()
            .expect("committed")
            .source_filters
            .len(),
        1
    );
}
