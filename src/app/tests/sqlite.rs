use super::*;
#[cfg(feature = "sqlite")]
use crate::app::run::restore_before_export;
#[cfg(feature = "saved-views")]
use crate::saved_views;

#[cfg(all(feature = "sqlite", feature = "saved-views"))]
#[test]
fn native_selection_commits_only_after_successful_replacement() {
    let (_directory, mut app) = sqlite_app(
        &[
            "CREATE TABLE events(id INTEGER PRIMARY KEY, name TEXT)",
            "INSERT INTO events VALUES (1, 'Ada')",
            "INSERT INTO events VALUES (2, 'Grace')",
        ],
        "events",
    );
    let native = "SELECT name AS label FROM events ORDER BY id DESC";
    let mut query = app
        .view
        .active_source_query()
        .expect("generated query")
        .clone();
    query.native_query = Some(native.to_owned());
    assert!(app.view.request_source_query(query, true));
    let pending = app.view.to_saved_view_yaml("events", "events.db", None);
    let source = saved_views::parse_saved_view_yaml(&pending)
        .expect("pending YAML")
        .view
        .source;
    assert_eq!(source.table.as_deref(), Some("events"));
    assert!(source.query.is_none());

    app.view
        .await_latest_source_query()
        .expect("native activation");
    assert_eq!(app.view.visible_rows_vec(), rows(&[&["Grace"], &["Ada"]]));
    let committed = app.view.to_saved_view_yaml("events", "events.db", None);
    let source = saved_views::parse_saved_view_yaml(&committed)
        .expect("committed YAML")
        .view
        .source;
    assert!(source.table.is_none());
    assert_eq!(source.query.as_deref(), Some(native));
    assert!(!committed.contains("column_1"));
}

#[cfg(all(feature = "sqlite", feature = "saved-views"))]
#[test]
fn rejected_native_query_keeps_committed_base_in_save_and_replay() {
    let directory = tempfile::tempdir().expect("fixture");
    let path = directory.path().join("events.db");
    create_sqlite_database(
        &path,
        &[
            "CREATE TABLE events(id INTEGER PRIMARY KEY, name TEXT)",
            "INSERT INTO events VALUES (1, 'Ada')",
            "INSERT INTO events VALUES (2, 'Grace')",
        ],
    );
    let accepted = "SELECT name AS label FROM events ORDER BY id";
    let rejected = "SELECT missing AS label FROM events ORDER BY id";
    let mut app = app_for_source(
        path.clone(),
        ingest::OpenOptions {
            format: ingest::InputFormat::Sqlite,
            native_query: Some(accepted.to_owned()),
            ..ingest::OpenOptions::default()
        },
    );
    assert_eq!(app.view.visible_rows_vec(), rows(&[&["Ada"], &["Grace"]]));

    app.open_source_config_modal();
    let modal = app.source_modal.as_mut().expect("source configuration");
    modal.draft.native_query = Some(rejected.to_owned());
    modal.native_query_changed = true;
    app.handle_key(key(KeyCode::Enter))
        .expect("accept pending query");
    let pending_yaml = app.view.to_saved_view_yaml("events", "events.db", None);
    assert_eq!(
        saved_views::parse_saved_view_yaml(&pending_yaml)
            .expect("pending YAML")
            .view
            .source
            .query
            .as_deref(),
        Some(accepted)
    );
    app.view
        .await_latest_source_query()
        .expect_err("rejected query fails");
    assert_eq!(app.view.visible_rows_vec(), rows(&[&["Ada"], &["Grace"]]));

    let yaml = app.view.to_saved_view_yaml("events", "events.db", None);
    let saved = saved_views::parse_saved_view_yaml(&yaml).expect("saved YAML");
    assert_eq!(saved.view.source.query.as_deref(), Some(accepted));
    assert!(!yaml.contains(rejected));
    let replay = ingest::OpenOptions::merge(
        ingest::OpenOptions::default(),
        &saved.view.source_options(),
        &ingest::SourceOptionOverrides::default(),
    );
    let reopened = app_for_source(path.clone(), replay);
    assert_eq!(
        reopened.view.visible_rows_vec(),
        rows(&[&["Ada"], &["Grace"]])
    );

    assert_reload_supersedes_pending_native_query(&mut app, accepted);
}

#[cfg(feature = "saved-views")]
fn assert_reload_supersedes_pending_native_query(app: &mut App, accepted: &str) {
    app.reload().expect("reload committed base");
    assert_eq!(app.view.visible_rows_vec(), rows(&[&["Ada"], &["Grace"]]));
    let mut pending = app
        .view
        .active_source_query()
        .expect("reloaded query")
        .clone();
    pending.native_query = Some("SELECT name AS label FROM events ORDER BY id DESC".to_owned());
    assert!(app.view.request_source_query(pending, true));
    app.reload()
        .expect("reload supersedes pending native query");
    app.view
        .await_latest_source_query()
        .expect("latest reload activated");
    assert_eq!(app.view.visible_rows_vec(), rows(&[&["Ada"], &["Grace"]]));
    assert_eq!(
        app.view
            .committed_open_options()
            .expect("committed source")
            .native_query
            .as_deref(),
        Some(accepted)
    );
}

#[cfg(feature = "sqlite")]
#[test]
fn post_interactive_export_waits_for_latest_sqlite_query() {
    let (_directory, mut app) = sqlite_app(
        &[
            "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT)",
            "INSERT INTO users VALUES (1, 'Ada')",
            "INSERT INTO users VALUES (2, 'Grace')",
            "INSERT INTO users VALUES (3, 'Linus')",
        ],
        "users",
    );
    let id = app.view.table_definition().expect("definition").columns[0].id;
    let mut query = app
        .view
        .active_source_query()
        .expect("source query")
        .clone();
    query.order_by = vec![crate::table::SourceSort {
        column: id,
        direction: crate::table::SortDirection::Descending,
    }];
    assert!(app.view.request_source_query(query, false));
    assert!(app.view.source_query_is_pending());

    let mut output = Vec::new();
    restore_before_export(
        || Ok(()),
        || {
            app.view.await_latest_source_query()?;
            write_frozen_test_output(&app, output::OutputFormat::Table, &mut output)
        },
    )
    .expect("post-interactive export");

    let output = String::from_utf8(output).expect("UTF-8 table output");
    let linus = output.find("Linus").expect("latest first row");
    let grace = output.find("Grace").expect("latest second row");
    let ada = output.find("Ada").expect("latest third row");
    assert!(linus < grace && grace < ada);
    assert!(!output.contains("SELECT"));
}

#[cfg(feature = "sqlite")]
#[test]
fn latest_activation_failure_blocks_export_until_newer_success() {
    let (_directory, mut app) = sqlite_app(
        &[
            "CREATE TABLE events(id INTEGER PRIMARY KEY, name TEXT)",
            "INSERT INTO events VALUES (1, 'Ada')",
            "INSERT INTO events VALUES (2, 'Grace')",
        ],
        "events",
    );
    let mut bad = app
        .view
        .active_source_query()
        .expect("source query")
        .clone();
    bad.native_query = Some("SELECT missing FROM events".to_owned());
    assert!(app.view.request_source_query(bad, true));
    assert!(app.view.await_latest_source_query().is_err());
    let _ = app.view.take_source_status();

    let mut output = Vec::new();
    let result = restore_before_export(
        || Ok(()),
        || {
            app.view.await_latest_source_query()?;
            write_frozen_test_output(&app, output::OutputFormat::Jsonl, &mut output)
        },
    );
    assert!(
        result.is_err(),
        "consumed TUI status does not clear latest failure"
    );
    assert!(output.is_empty(), "failed revision emitted adapter bytes");

    let mut valid = app
        .view
        .active_source_query()
        .expect("old committed query")
        .clone();
    valid.native_query = Some("SELECT name FROM events ORDER BY id DESC".to_owned());
    assert!(app.view.request_source_query(valid, true));
    app.view
        .await_latest_source_query()
        .expect("newer activation");
    restore_before_export(
        || Ok(()),
        || write_frozen_test_output(&app, output::OutputFormat::Jsonl, &mut output),
    )
    .expect("newer result exported");
    let emitted = String::from_utf8(output).expect("JSONL output");
    assert!(emitted.find("Grace").expect("first") < emitted.find("Ada").expect("second"));
}

#[cfg(feature = "sqlite")]
#[test]
fn supported_actions_preserve_sqlite_database_and_sidecar_bytes() {
    for (label, wal) in [("rollback", false), ("wal", true)] {
        let directory = tempfile::tempdir().expect("SQLite directory");
        let path = directory.path().join(format!("{label}.db"));
        let writer = rusqlite::Connection::open(&path).expect("fixture connection");
        writer
            .execute_batch(if wal {
                "PRAGMA journal_mode=WAL;
                 PRAGMA wal_autocheckpoint=0;
                 CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT);
                 INSERT INTO users VALUES (1, 'Ada'), (2, 'Grace'), (3, 'Linus');"
            } else {
                "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT);
                 INSERT INTO users VALUES (1, 'Ada'), (2, 'Grace'), (3, 'Linus');"
            })
            .expect("fixture schema and rows");
        let held_writer = if wal {
            Some(writer)
        } else {
            drop(writer);
            None
        };
        let before = sqlite_artifact_snapshot(&path);

        {
            let mut app = app_for_source(
                path.clone(),
                ingest::OpenOptions {
                    table: Some("users".to_owned()),
                    ..ingest::OpenOptions::default()
                },
            );
            app.view.goto(1, 1);
            app.view
                .apply_filter(
                    1,
                    crate::ops::filter::FilterMode::In,
                    crate::ops::filter::FilterKind::Text,
                    "a".to_owned(),
                )
                .expect("view filter");
            app.view.clear_filters_for_column(1);
            app.handle_key(key(KeyCode::Char('u')))
                .expect("source configuration");
            app.handle_key(key(KeyCode::Char('s')))
                .expect("source sort");
            app.handle_key(key(KeyCode::Enter))
                .expect("apply source query");
            app.view.await_latest_source_query().expect("source result");
            assert!(app.query_modal_body().contains("SELECT"));
            app.reload().expect("reload");
        }

        assert_eq!(
            sqlite_artifact_snapshot(&path),
            before,
            "{label} database artifacts changed"
        );
        drop(held_writer);
    }
}

#[cfg(feature = "sqlite")]
#[test]
fn sqlite_source_view_column_and_query_modals_keep_their_scopes_distinct() {
    let (_directory, mut app) = sqlite_app(
        &[
            "CREATE TABLE users(id INTEGER PRIMARY KEY, name VARCHAR(30))",
            "INSERT INTO users VALUES (1, 'Ada')",
            "INSERT INTO users VALUES (2, 'Grace')",
        ],
        "users",
    );
    assert_source_modal_cancels_limit_edit(&mut app);

    app.handle_key(key(KeyCode::Char('u')))
        .expect("source modal");
    app.handle_key(key(KeyCode::Char('s')))
        .expect("stage source sort");
    app.handle_key(key(KeyCode::Enter))
        .expect("apply source sort");
    app.view.await_latest_source_query().expect("source result");

    app.handle_key(key(KeyCode::Char('V'))).expect("view modal");
    assert_eq!(app.popup, Some(ui::Popup::ViewConfig));
    assert!(app.view.view_transform_summary().contains("nulls Last"));
    app.handle_key(key(KeyCode::Char('n')))
        .expect("toggle view null placement");
    assert!(app.view.view_transform_summary().contains("nulls First"));
    app.handle_key(key(KeyCode::Enter))
        .expect("close view modal");

    app.handle_key(key(KeyCode::Char('#')))
        .expect("add view sort");
    app.handle_key(key(KeyCode::Char('V'))).expect("view modal");
    assert!(app
        .view
        .view_transform_summary()
        .contains("1 view sort key"));
    app.handle_key(key(KeyCode::Char('x')))
        .expect("clear view operations");
    assert!(app
        .view
        .view_transform_summary()
        .contains("0 view sort key"));
    app.handle_key(key(KeyCode::Char('i')))
        .expect("column info from view");
    assert_eq!(app.popup, Some(ui::Popup::ColumnInfo));
    let popup = app.column_info.as_ref().unwrap().popup();
    assert!(popup.summary.contains("declared:INTEGER"));
    assert!(popup.summary.contains("source sort"));
    app.handle_key(key(KeyCode::Esc))
        .expect("close column info");

    app.handle_key(key(KeyCode::Char('p')))
        .expect("query modal");
    assert_eq!(app.popup, Some(ui::Popup::Query));
    let query = app.query_modal_body();
    assert!(query.contains("SELECT"));
    assert!(query.contains("Copyable SQL"));
    assert!(query.contains("Local transformations not represented by the native query"));
}

fn assert_source_modal_cancels_limit_edit(app: &mut App) {
    let original_limit = app.view.active_source_query().unwrap().limit;

    app.handle_key(key(KeyCode::Char('u')))
        .expect("source modal");
    assert_eq!(app.popup, Some(ui::Popup::SourceConfig));
    assert!(app.source_modal_body().contains("Source: users"));
    app.handle_key(key(KeyCode::Char('+')))
        .expect("stage source limit");
    assert_ne!(
        app.source_modal.as_ref().unwrap().draft.limit,
        original_limit
    );
    app.handle_key(key(KeyCode::Esc))
        .expect("cancel source draft");
    assert_eq!(
        app.view.active_source_query().unwrap().limit,
        original_limit
    );
}
