use super::*;
use crate::app::interactive::footer_status;
use crate::app::run::restore_before_export;
#[cfg(feature = "saved-views")]
use crate::app::startup::AppStartup;
#[cfg(feature = "saved-views")]
use crate::output;

#[test]
fn restoration_failure_prevents_final_export() {
    let exported = Cell::new(false);
    let error = restore_before_export(
        || Err(std::io::Error::other("restore failed")),
        || {
            exported.set(true);
            Ok(())
        },
    )
    .expect_err("restoration must fail");
    assert!(error.to_string().contains("restore failed"));
    assert!(!exported.get());
}

#[test]
fn source_status_is_transient_before_the_live_count_returns() {
    assert_eq!(
        footer_status(Some("Indexed 4096 rows"), None, "4096+ rows"),
        "Indexed 4096 rows"
    );
    assert_eq!(footer_status(None, None, "4096+ rows"), "4096+ rows");
}

#[test]
fn missing_source_extent_is_pending_only_while_a_query_runs() {
    assert_eq!(source_extent_label(None, true), "pending");
    assert_eq!(source_extent_label(None, false), "unknown");
}

#[cfg(feature = "saved-views")]
#[test]
fn startup_opens_and_binds_the_same_selected_document_after_an_on_disk_edit() {
    let root = tempfile::tempdir().expect("config root");
    let (config, source, view_path) = selected_view_fixture(root.path());
    let selected = prepare_saved_view_invocation(&config, Some(root.path()))
        .expect("first invocation selection");
    let theme = || theme::ThemeLoad {
        theme: theme::default_theme(),
        warnings: Vec::new(),
    };
    let first = AppStartup::new(&config, theme(), &selected).open(
        source.clone(), None, |source, options| {
            assert_eq!(options.limit.map(|limit| limit.get()), Some(1));
            std::fs::write(
                &view_path,
                "name: chosen\nfilenames: ['*']\nsource:\n  limit: 2\nview:\n  columns:\n    Name: {label: CHANGED, format: lowercase}\n",
            ).expect("external saved YAML edit during source open");
            ingest::open_source(source, options)
        },
    ).expect("open first").expect("selected table");
    assert_eq!(
        first.view.output_header(),
        Some(vec!["ORIGINAL".to_owned()])
    );
    assert_eq!(
        first
            .view
            .committed_open_options()
            .and_then(|options| options.limit)
            .map(|limit| limit.get()),
        Some(1),
    );
    assert_eq!(
        first
            .saved_view
            .as_ref()
            .and_then(|saved| saved.source_path.as_ref()),
        Some(&view_path),
    );
    assert_eq!(startup_projection_row_count(&first), 1);

    let fresh = prepare_saved_view_invocation(&config, Some(root.path()))
        .expect("fresh invocation selection");
    let second = AppStartup::new(&config, theme(), &fresh)
        .open(source, None, ingest::open_source)
        .expect("open fresh")
        .expect("selected table");
    assert_eq!(
        second.view.output_header(),
        Some(vec!["CHANGED".to_owned()])
    );
    assert_eq!(
        second
            .view
            .committed_open_options()
            .and_then(|options| options.limit)
            .map(|limit| limit.get()),
        Some(2),
    );
    assert_eq!(startup_projection_row_count(&second), 2);
}

#[cfg(feature = "saved-views")]
fn selected_view_fixture(
    root: &std::path::Path,
) -> (cli::Config, ingest::source::InputSource, std::path::PathBuf) {
    let bundle = root.join("tview/views/nested");
    std::fs::create_dir_all(&bundle).expect("view bundle");
    let view_path = bundle.join("chosen.yml");
    std::fs::write(
        &view_path,
        "name: chosen\nfilenames: ['*']\nsource:\n  limit: 1\nview:\n  columns:\n    Name: {label: ORIGINAL, format: uppercase}\n",
    ).expect("selected YAML");
    let input_path = root.join("data.csv");
    std::fs::write(&input_path, "Name\nalpha\nbeta\n").expect("CSV input");
    let filename = input_path.to_string_lossy().into_owned();
    let args = cli::Args::try_parse_from([
        "tview",
        "--view",
        "chosen",
        "--output",
        "table",
        filename.as_str(),
    ])
    .expect("arguments");
    let config = cli::Config::from_args(args).expect("configuration");
    let source = ingest::source::InputSource::Path(input_path);
    (config, source, view_path)
}

#[cfg(feature = "saved-views")]
fn startup_projection_row_count(app: &App) -> usize {
    let requirements =
        output::requirements(output::OutputFormat::Table, output::ColorOutput::Never)
            .expect("projection requirements");
    app.view
        .prepare_projection(requirements, &app.theme)
        .expect("startup result")
        .output
        .rows
        .len()
}

#[cfg(feature = "sqlite")]
#[test]
fn cancelling_table_selection_is_a_clean_startup_outcome() {
    let directory = tempfile::tempdir().expect("SQLite directory");
    let path = directory.path().join("multiple.db");
    create_sqlite_database(
        &path,
        &[
            "CREATE TABLE users(id INTEGER PRIMARY KEY)",
            "CREATE TABLE events(id INTEGER PRIMARY KEY)",
        ],
    );
    let filename = path.to_string_lossy().into_owned();
    let args = cli::Args::try_parse_from(["tview", filename.as_str()]).expect("arguments");
    let config = cli::Config {
        #[cfg(feature = "saved-views")]
        saved_view: cli::SavedViewSelection::Disabled,
        ..cli::Config::from_args(args).expect("configuration")
    };
    let source = ingest::source::InputSource::Path(path);
    let theme_load = theme::ThemeLoad {
        theme: theme::default_theme(),
        warnings: Vec::new(),
    };
    let mut selector = |_: &[ingest::RelationCatalogEntry]| Ok(None);

    let prepared =
        prepare_app(&config, theme_load, source, Some(&mut selector)).expect("clean cancellation");

    assert!(prepared.is_none());
}
