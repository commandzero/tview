use super::*;
#[cfg(feature = "saved-views")]
use crate::saved_views;

#[cfg(feature = "elasticsearch")]
#[test]
fn elasticsearch_source_configuration_exposes_mapping_result_and_partial_state() {
    let server = MockElasticsearch::start(vec![
        MockElasticsearchResponse::ok(
            r#"{"logs-a":{"mappings":{"properties":{"message":{"type":"keyword"},"latency":{"type":"long"}}}}}"#,
        ),
        MockElasticsearchResponse::ok(
            r#"{"fields":{"message":{"keyword":{"searchable":true,"aggregatable":true}},"latency":{"long":{"searchable":true,"aggregatable":true}}}}"#,
        ),
        MockElasticsearchResponse::ok(
            r#"{"columns":[{"name":"message","type":"keyword"},{"name":"latency","type":"long"},{"name":"_index","type":"keyword"},{"name":"_id","type":"keyword"}],"values":[["boom",42,"logs-a","one"]],"is_partial":true,"warnings":["one shard timed out"]}"#,
        ),
    ]);
    let options = ingest::OpenOptions {
        format: ingest::InputFormat::Elasticsearch,
        table: Some("logs-a".to_owned()),
        ..ingest::OpenOptions::default()
    };
    let mut app = app_for_elasticsearch(&server, options);

    app.handle_key(key(KeyCode::Char('u'))).unwrap();
    let body = app.source_modal_body();
    assert!(body.contains(&format!("Endpoint: {}", server.endpoint)));
    assert!(body.contains("Source: logs-a"));
    assert!(body.contains("Partial: yes"), "{body}");
    assert!(body.contains("Result fields: 4"));
    assert!(body.contains("Mapping fields: 2"));
    assert!(body.contains("Selected mapping: message [keyword]"));
    assert!(body.contains("Source sorting: supported"));
    assert!(body.contains("ES|QL:"));

    app.handle_key(key(KeyCode::Char('q'))).unwrap();
    assert!(app.source_modal_body().contains("Native query editor"));
    app.handle_key(key(KeyCode::Esc)).unwrap();
    app.handle_key(key(KeyCode::Esc)).unwrap();
    assert_eq!(app.popup, None);
    assert_eq!(server.request_count(), 3, "cancelling made no replacement");

    app.handle_key(key(KeyCode::Char('p'))).unwrap();
    let query = app.query_modal_body();
    assert!(query.contains("Parameterized ES|QL"));
    assert!(query.contains("FROM logs-a METADATA _index, _id"));
    assert!(!query.contains("one shard timed out"));
}

#[cfg(feature = "elasticsearch")]
#[test]
fn elasticsearch_tui_keeps_prior_rows_pending_then_activates_new_schema() {
    let server = MockElasticsearch::start(vec![
        MockElasticsearchResponse::ok(
            r#"{"logs-a":{"mappings":{"properties":{"message":{"type":"keyword"}}}}}"#,
        ),
        MockElasticsearchResponse::ok(
            r#"{"fields":{"message":{"keyword":{"searchable":true,"aggregatable":true}}}}"#,
        ),
        MockElasticsearchResponse::ok(
            r#"{"columns":[{"name":"message","type":"keyword"}],"values":[["before"]]}"#,
        ),
        MockElasticsearchResponse::delayed(
            r#"{"columns":[{"name":"count","type":"long"}],"values":[[2]]}"#,
            std::time::Duration::from_millis(40),
        ),
    ]);
    let options = ingest::OpenOptions {
        format: ingest::InputFormat::Elasticsearch,
        table: Some("logs-a".to_owned()),
        ..ingest::OpenOptions::default()
    };
    let mut app = app_for_elasticsearch(&server, options);
    assert_eq!(app.view.current_raw_cell(), Some("before"));

    app.handle_key(key(KeyCode::Char('u'))).unwrap();
    {
        let modal = app.source_modal.as_mut().unwrap();
        modal.query_input = "FROM logs-a | STATS count = COUNT(*)".to_owned();
        modal.draft.native_query = Some(modal.query_input.clone());
        modal.native_query_changed = true;
    }
    app.handle_key(key(KeyCode::Enter)).unwrap();
    assert!(app.view.source_query_is_pending());
    assert_eq!(app.view.current_raw_cell(), Some("before"));

    app.view.await_latest_source_query().unwrap();
    assert_eq!(app.view.header().unwrap(), ["count"]);
    assert_eq!(app.view.current_raw_cell(), Some("2"));
    let mut output = Vec::new();
    write_frozen_test_output(&app, output::OutputFormat::Table, &mut output).unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("count"));
    assert!(output.contains('2'));
    assert!(!output.contains("FROM logs-a"));
}

#[cfg(feature = "elasticsearch")]
#[test]
fn elasticsearch_tui_reports_replacement_error_and_preserves_result() {
    let server = MockElasticsearch::start(vec![
        MockElasticsearchResponse::ok(
            r#"{"logs-a":{"mappings":{"properties":{"message":{"type":"keyword"}}}}}"#,
        ),
        MockElasticsearchResponse::ok(
            r#"{"fields":{"message":{"keyword":{"searchable":true,"aggregatable":true}}}}"#,
        ),
        MockElasticsearchResponse::ok(
            r#"{"columns":[{"name":"message","type":"keyword"}],"values":[["before"]]}"#,
        ),
        MockElasticsearchResponse::error(400, r#"{"error":{"reason":"invalid ES|QL test query"}}"#),
    ]);
    let options = ingest::OpenOptions {
        format: ingest::InputFormat::Elasticsearch,
        table: Some("logs-a".to_owned()),
        ..ingest::OpenOptions::default()
    };
    let mut app = app_for_elasticsearch(&server, options);
    app.open_source_config_modal();
    app.source_modal.as_mut().unwrap().draft.limit = std::num::NonZeroUsize::new(5).unwrap();
    app.handle_key(key(KeyCode::Enter)).unwrap();

    let error = app.view.await_latest_source_query().unwrap_err();
    assert!(error.to_string().contains("invalid ES|QL test query"));
    assert_eq!(app.view.header().unwrap(), ["message"]);
    assert_eq!(app.view.current_raw_cell(), Some("before"));
    assert!(app
        .view
        .take_source_status()
        .unwrap()
        .contains("prior result retained"));
}

#[cfg(feature = "elasticsearch")]
#[test]
fn query_only_elasticsearch_source_configuration_uses_result_schema() {
    let server = MockElasticsearch::start(vec![MockElasticsearchResponse::ok(
        r#"{"columns":[{"name":"level","type":"keyword"}],"values":[["error"]]}"#,
    )]);
    let options = ingest::OpenOptions {
        format: ingest::InputFormat::Elasticsearch,
        native_query: Some("FROM logs-* | KEEP level".to_owned()),
        ..ingest::OpenOptions::default()
    };
    let mut app = app_for_elasticsearch(&server, options);
    app.open_source_config_modal();
    let body = app.source_modal_body();
    assert!(body.contains("Mapping fields: result schema only"));
    assert!(body.contains("Selected column: level"));
}

#[cfg(all(feature = "elasticsearch", feature = "saved-views"))]
#[test]
fn generated_elasticsearch_saved_view_persists_base_not_composed_esql() {
    let server = MockElasticsearch::start(vec![MockElasticsearchResponse::ok(
        r#"{"columns":[{"name":"level","type":"keyword"}],"values":[["error"]]}"#,
    )]);
    let options = ingest::OpenOptions {
        format: ingest::InputFormat::Elasticsearch,
        native_query: Some("FROM logs-*".to_owned()),
        limit: std::num::NonZeroUsize::new(25),
        source_filters: vec![ingest::SourceFilterRequest {
            column: "level".to_owned(),
            operator: crate::table::SourceFilterOperator::Equal,
            operand: Some(crate::table::SourceOperand::Text("error".to_owned())),
        }],
        ..ingest::OpenOptions::default()
    };
    let app = app_for_elasticsearch(&server, options);
    let yaml = app
        .view
        .to_saved_view_yaml("errors", &app.input_filename(), None);
    let parsed = saved_views::parse_saved_view_yaml(&yaml).expect("generated YAML");

    assert_eq!(parsed.view.source.query.as_deref(), Some("FROM logs-*"));
    assert_eq!(parsed.view.source.limit, Some(25));
    assert_eq!(parsed.view.source.filters.len(), 1);
    assert!(!yaml.contains("?v1"));
    assert!(!yaml.contains("LIMIT 26"));
    assert_eq!(
        parsed.view.filenames[0].raw,
        app.source.saved_view_filename()
    );
}

#[cfg(feature = "elasticsearch")]
#[test]
fn elasticsearch_reload_refetches_catalog_and_activates_new_generation() {
    let server = MockElasticsearch::start(vec![
        MockElasticsearchResponse::ok(
            r#"{"logs-a":{"mappings":{"properties":{"message":{"type":"keyword"}}}}}"#,
        ),
        MockElasticsearchResponse::ok(
            r#"{"fields":{"message":{"keyword":{"searchable":true,"aggregatable":true}}}}"#,
        ),
        MockElasticsearchResponse::ok(
            r#"{"columns":[{"name":"message","type":"keyword"}],"values":[["before"]]}"#,
        ),
        MockElasticsearchResponse::ok(
            r#"{"logs-a":{"mappings":{"properties":{"message":{"type":"keyword"},"latency":{"type":"long"}}}}}"#,
        ),
        MockElasticsearchResponse::ok(
            r#"{"fields":{"message":{"keyword":{"searchable":true,"aggregatable":true}},"latency":{"long":{"searchable":true,"aggregatable":true}}}}"#,
        ),
        MockElasticsearchResponse::ok(
            r#"{"columns":[{"name":"message","type":"keyword"},{"name":"latency","type":"long"}],"values":[["after",42]]}"#,
        ),
    ]);
    let options = ingest::OpenOptions {
        format: ingest::InputFormat::Elasticsearch,
        table: Some("logs-a".to_owned()),
        ..ingest::OpenOptions::default()
    };
    let mut app = app_for_elasticsearch(&server, options);
    let generation = app.view.table_definition().unwrap().generation;
    app.reload().unwrap();

    assert_ne!(app.view.table_definition().unwrap().generation, generation);
    assert_eq!(app.view.header().unwrap(), ["message", "latency"]);
    assert_eq!(app.view.current_raw_cell(), Some("after"));
    let fields = app.view.source_field_catalog();
    assert_eq!(fields.len(), 2);
    assert!(std::sync::Arc::ptr_eq(
        &fields,
        &app.view.source_field_catalog()
    ));
}
