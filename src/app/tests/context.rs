use super::*;
#[cfg(feature = "saved-views")]
use crate::app::startup::configured_open_options;
use crate::app::view_file::write_saved_view_atomic;
use crate::saved_views;

#[cfg(all(feature = "elasticsearch", feature = "saved-views"))]
#[test]
fn context_saved_matching_merging_and_yaml_use_canonical_identity() {
    let root = tempfile::tempdir().expect("config root");
    let views = root.path().join("tview/views");
    std::fs::create_dir_all(&views).expect("views dir");
    std::fs::write(
        views.join("context.yml"),
        "name: context\nfilenames: ['.production.elasticsearch://logs-*', '.production.elasticsearch://']\nsource:\n  format: json\n  query: FROM saved-logs\nview: {}\n",
    )
    .expect("context view");
    for source in [
        ".production.es://logs-*",
        ".production.elasticsearch://logs-*",
        ".production.es://",
    ] {
        let args = cli::Args::try_parse_from(["tview", source]).expect("arguments");
        let config = cli::Config::from_args(args).expect("configuration");
        let invocation = prepare_saved_view_invocation(&config, Some(root.path()))
            .expect("saved view selection");
        let saved_views::SavedViewInvocation::Enabled {
            selected: Some(selected),
            target_path: Some(target_path),
            ..
        } = &invocation
        else {
            panic!("canonical context source must select saved view");
        };
        assert_eq!(selected.canonical_name, "context");
        assert_eq!(target_path.parent(), Some(views.as_path()));
        let options = configured_open_options(&config, &invocation).expect("merged options");
        assert_eq!(options.format, ingest::InputFormat::Elasticsearch);
        if config.target.elastic_context().unwrap().table().is_some() {
            assert_eq!(options.table.as_deref(), Some("logs-*"));
            assert_eq!(options.native_query, None);
        } else {
            assert_eq!(options.table, None);
            assert_eq!(options.native_query.as_deref(), Some("FROM saved-logs"));
        }
        let canonical = config.target.safe_identity();
        let mut app = app_with_rows(rows(&[&["message"], &["saved"]]));
        app.source = config.target;
        let yaml = app
            .view
            .to_saved_view_yaml("context", &app.input_filename(), None);
        let parsed = saved_views::parse_saved_view_yaml(&yaml).expect("generated YAML");
        assert_eq!(parsed.view.filenames[0].raw, canonical);
    }
}

#[cfg(all(feature = "elasticsearch", feature = "saved-views"))]
#[test]
fn generated_context_views_roundtrip_through_discovery_for_both_aliases() {
    for (source, alias) in [
        (".es://", ".elasticsearch://"),
        (
            ".production.us-west.es://",
            ".production.us-west.elasticsearch://",
        ),
    ] {
        let root = tempfile::tempdir().expect("config root");
        let args =
            cli::Args::try_parse_from(["tview", source, "--query", "FROM logs-* | KEEP message"])
                .expect("initial arguments");
        let config = cli::Config::from_args(args).expect("initial configuration");
        let invocation =
            prepare_saved_view_invocation(&config, Some(root.path())).expect("initial invocation");
        let saved_views::SavedViewInvocation::Enabled {
            selected: None,
            target_path: Some(path),
            view_name,
            ..
        } = &invocation
        else {
            panic!("new context source must prepare an authoring path");
        };
        assert_eq!(
            path.extension().and_then(|value| value.to_str()),
            Some("yml")
        );
        assert_eq!(
            path.file_stem().and_then(|value| value.to_str()),
            Some(config.target.saved_view_filename().as_str())
        );
        let mut app = app_with_rows(rows(&[&["message"], &["context-row"]]));
        app.source = config.target.clone();
        app.view
            .initialize_source_configuration(
                configured_open_options(&config, &invocation).expect("opening options"),
            )
            .expect("committed source");
        let yaml = app
            .view
            .to_saved_view_yaml(view_name, &app.input_filename(), None);
        write_saved_view_atomic(path, &yaml).expect("saved generated view");
        for target in [source, alias] {
            assert_generated_context_view(root.path(), target, path);
        }
    }
}

fn assert_generated_context_view(root: &std::path::Path, target: &str, path: &std::path::Path) {
    let args = cli::Args::try_parse_from(["tview", target]).expect("reopen arguments");
    let config = cli::Config::from_args(args).expect("reopen configuration");
    let reopened = prepare_saved_view_invocation(&config, Some(root)).expect("reopened invocation");
    let saved_views::SavedViewInvocation::Enabled {
        selected: Some(selected),
        target_path: Some(generated_path),
        ..
    } = &reopened
    else {
        panic!("generated context view must be rediscovered without CLI selection");
    };
    assert_eq!(generated_path, path);
    assert_eq!(selected.path, *path);
    assert_eq!(
        selected.view.filenames[0].raw,
        config.target.safe_identity()
    );
    let options = configured_open_options(&config, &reopened).expect("saved options");
    assert_eq!(
        options.native_query.as_deref(),
        Some("FROM logs-* | KEEP message")
    );
    assert_eq!(options.table, None);
}
