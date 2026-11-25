use super::*;

#[test]
fn committed_opening_choices_survive_reload_without_reinterpreting_data() {
    let dir = tempfile::tempdir().expect("fixture directory");
    let delimited_path = dir.path().join("data.csv");
    std::fs::write(&delimited_path, "name;amount\n'A;B';1\n").expect("delimited data");
    let mut csv = app_for_source(
        delimited_path,
        ingest::OpenOptions {
            format: ingest::InputFormat::Delimited,
            delimited: ingest::ParseOptions {
                delimiter: Some(b';'),
                quote_char: b'\'',
                ..ingest::ParseOptions::default()
            },
            ..ingest::OpenOptions::default()
        },
    );
    let interpreted = csv.view.visible_rows_vec();
    assert!(interpreted
        .iter()
        .any(|row| row.iter().any(|cell| cell == "A;B")));
    csv.reload().expect("reload delimited");
    assert_eq!(csv.view.visible_rows_vec(), interpreted);
    assert_eq!(
        csv.view
            .committed_open_options()
            .expect("committed")
            .delimited
            .delimiter,
        Some(b';')
    );
}

#[test]
fn committed_json_opening_choices_survive_reload() {
    let dir = tempfile::tempdir().expect("fixture directory");
    let json_path = dir.path().join("nested.json");
    std::fs::write(
        &json_path,
        r#"{"payload":[{"a":1},{"b":2}],"ignored":[{"x":3}]}"#,
    )
    .expect("JSON data");
    let mut json = app_for_source(
        json_path,
        ingest::OpenOptions {
            format: ingest::InputFormat::Json,
            json_path: Some("/payload".parse().expect("JSON path")),
            schema_scan: ingest::SchemaScan::Full,
            ..ingest::OpenOptions::default()
        },
    );
    assert_eq!(
        json.view
            .committed_open_options()
            .expect("committed")
            .schema_scan,
        ingest::SchemaScan::Full
    );
    assert_eq!(
        json.view
            .committed_open_options()
            .expect("committed")
            .json_path
            .as_ref()
            .map(|path| path.as_str()),
        Some("/payload")
    );
    json.reload().expect("reload selected JSON");
    assert_eq!(json.view.column_count(), 2);
    assert_eq!(
        json.view
            .committed_open_options()
            .expect("committed")
            .schema_scan,
        ingest::SchemaScan::Full
    );
}

#[test]
fn source_info_reports_auto_entries_and_record_override_hint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repositories.json");
    std::fs::write(
        &path,
        r#"{"a":{"type":"fs"},"b":{"type":"s3"},"c":{"type":"gcs"}}"#,
    )
    .expect("write");
    let options = ingest::OpenOptions {
        format: ingest::InputFormat::Json,
        ..ingest::OpenOptions::default()
    };
    let app = app_for_source(path, options);

    let info = app.info_text();
    assert!(info.contains("Object mode: auto → entries"));
    assert!(info.contains("--object-mode record"));
}

#[test]
fn reload_preserves_explicit_mode_and_redetects_effective_auto() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("objects.json");
    std::fs::write(
        &path,
        r#"{"a":{"type":"fs"},"b":{"type":"s3"},"c":{"type":"gcs"}}"#,
    )
    .expect("write keyed");

    let auto_options = ingest::OpenOptions {
        format: ingest::InputFormat::Json,
        ..ingest::OpenOptions::default()
    };
    let mut automatic = app_for_source(path.clone(), auto_options);
    assert_eq!(
        automatic.view.object_mode_resolution().unwrap().resolved,
        ingest::ResolvedObjectMode::Entries
    );
    std::fs::write(
        &path,
        r#"{"name":"cluster","enabled":true,"settings":{"x":1}}"#,
    )
    .expect("write record");
    automatic.reload().expect("reload auto");
    let auto_resolution = automatic.view.object_mode_resolution().unwrap();
    assert_eq!(auto_resolution.requested, ingest::ObjectMode::Auto);
    assert_eq!(auto_resolution.resolved, ingest::ResolvedObjectMode::Record);

    let explicit_options = ingest::OpenOptions {
        format: ingest::InputFormat::Json,
        object_mode: ingest::ObjectMode::Entries,
        object_mode_origin: ingest::ObjectModeOrigin::Cli,
        ..ingest::OpenOptions::default()
    };
    let mut explicit = app_for_source(path.clone(), explicit_options);
    assert_eq!(
        explicit.view.row_count_state(),
        crate::table::RowCount::Exact(3)
    );
    assert!(explicit.info_text().contains("Object mode: entries"));
    assert!(!explicit.info_text().contains("auto →"));
    std::fs::write(&path, r#"{"first":1,"second":2}"#).expect("replace explicit");
    explicit.reload().expect("reload explicit");
    let explicit_resolution = explicit.view.object_mode_resolution().unwrap();
    assert_eq!(explicit_resolution.requested, ingest::ObjectMode::Entries);
    assert_eq!(
        explicit_resolution.resolved,
        ingest::ResolvedObjectMode::Entries
    );
    assert_eq!(
        explicit.view.row_count_state(),
        crate::table::RowCount::Exact(2)
    );
}

#[test]
fn file_source_modal_is_available_without_forcing_initial_materialization() {
    let file = tempfile::NamedTempFile::new().expect("csv fixture");
    std::fs::write(file.path(), "name,value\nalpha,1\nbeta,2\n").expect("csv");
    let mut app = app_for_source(file.path().to_path_buf(), ingest::OpenOptions::default());
    assert!(matches!(
        app.view.row_count_state(),
        crate::table::RowCount::AtLeast(_) | crate::table::RowCount::Exact(_)
    ));

    app.handle_key(key(KeyCode::Char('u')))
        .expect("source modal");
    let body = app.source_modal_body();
    assert!(body.contains("Limit: unbounded"));
    assert!(!body.contains("Extent: pending"));
    assert!(body.contains("Source sorting: unavailable"));
    app.handle_key(key(KeyCode::Char('s')))
        .expect("unavailable source sort");
    assert!(app
        .source_modal
        .as_ref()
        .and_then(|modal| modal.error.as_deref())
        .is_some_and(|error| error.contains("unavailable")));
}

#[test]
fn reload_reapplies_active_filters_and_clamps_cursor() {
    let mut file = tempfile::NamedTempFile::new().expect("temp file");
    writeln!(file, "Name").expect("write header");
    writeln!(file, "alpha").expect("write row");
    writeln!(file, "beta").expect("write row");

    let mut app = app_for_source(file.path().to_path_buf(), ingest::OpenOptions::default());
    app.view
        .apply_filter(0, FilterMode::In, FilterKind::Text, "alp".to_owned())
        .expect("apply filter");
    app.view.goto(10, 0);

    file.as_file_mut().set_len(0).expect("truncate");
    file.rewind().expect("rewind");
    writeln!(file, "Name").expect("write header");
    writeln!(file, "alpha").expect("write row");
    writeln!(file, "gamma").expect("write row");
    file.flush().expect("flush");

    app.reload().expect("reload");

    assert_eq!(app.view.visible_rows_vec(), rows(&[&["alpha"]]));
    assert_eq!(app.view.cursor(), view::Position { row: 0, column: 0 });
}
