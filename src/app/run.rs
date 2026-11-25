#[cfg(feature = "elasticsearch")]
use super::context_startup::prepare_context_app;
use super::interactive::{run_interactive, select_table_modal};
#[cfg(feature = "saved-views")]
use super::saved_view::format_saved_view_warning;
#[cfg(feature = "saved-views")]
use super::startup::prepare_saved_view_invocation;
use super::startup::{configured_open_options, open_selected_source, prepare_app};
use super::App;
#[cfg(feature = "saved-views")]
use crate::saved_views;
use crate::{cli, ingest, output, projection, theme, ui, view};

pub(crate) fn run_batch(
    config: &cli::Config,
    format: output::OutputFormat,
    theme_load: theme::ThemeLoad,
) -> anyhow::Result<()> {
    let requirements = output::requirements(format, config.color)?;
    #[cfg(feature = "saved-views")]
    let invocation = prepare_saved_view_invocation(config, None)?;
    let options = configured_open_options(
        config,
        #[cfg(feature = "saved-views")]
        &invocation,
    )?;
    let Some((mut opened, open_options)) =
        open_selected_source(config.target.clone(), options, None, ingest::open_source)?
    else {
        return Ok(());
    };
    let source_warnings = std::mem::take(&mut opened.warnings);
    let policy = config
        .top_lines
        .map_or(projection::ProjectionPolicy::Complete, |limit| {
            projection::ProjectionPolicy::Prefix {
                limit: limit.get(),
                full_schema: open_options.schema_scan == ingest::SchemaScan::Full,
            }
        });
    let settings = batch_projection_settings(
        config,
        &opened.definition,
        #[cfg(feature = "saved-views")]
        &invocation,
    );
    let partial = opened.store.result_is_partial();
    let frozen = projection::prepare(
        projection::SourceInput {
            store: opened.store.as_mut(),
            definition: opened.definition,
            partial,
            replay_schema: None,
        },
        settings,
        policy,
        requirements,
        &theme_load.theme,
    )?;
    #[cfg(feature = "saved-views")]
    emit_saved_view_warnings(&invocation);
    emit_theme_warnings(&theme_load);
    emit_diagnostics(&frozen.diagnostics);
    if opened.store.result_is_partial() {
        eprintln!("warning: source returned a partial result");
    }
    for warning in source_warnings.iter().chain(opened.store.result_warnings()) {
        eprintln!("warning: {warning}");
    }
    output::write_prepared_to_stdout(
        format,
        config.color,
        &frozen.output,
        config.top_lines.map(|_| frozen.remaining),
    )
}

fn batch_projection_settings(
    config: &cli::Config,
    definition: &crate::table::TableDefinition,
    #[cfg(feature = "saved-views")] invocation: &saved_views::SavedViewInvocation,
) -> projection::ProjectionSettings {
    #[cfg(feature = "saved-views")]
    let selected_view = match invocation {
        saved_views::SavedViewInvocation::Enabled {
            selected: Some(selected),
            ..
        } => Some(selected.view.view.clone()),
        _ => None,
    };
    #[cfg(feature = "saved-views")]
    let sorts_enabled = config.sorted != Some(false);
    projection::ProjectionSettings::new(
        config.width,
        #[cfg(feature = "saved-views")]
        selected_view,
        #[cfg(feature = "saved-views")]
        sorts_enabled,
        definition,
    )
}

pub(crate) fn run_terminal(
    config: &cli::Config,
    emit_on_exit: Option<output::OutputFormat>,
    theme_load: theme::ThemeLoad,
) -> anyhow::Result<()> {
    let source = config.target.clone();
    if let Some(format) = emit_on_exit {
        output::requirements(format, config.color)?;
    }
    ui::terminal::TerminalSession::ensure_available()?;
    let source_is_stdin = source == ingest::source::InputSource::Stdin;
    let mut terminal = ui::terminal::TerminalSession::enter(source_is_stdin)?;
    let source = if source_is_stdin {
        #[cfg(windows)]
        {
            ingest::source::stream_reader_for_interactive(terminal.take_data_stdin_reader()?)
        }
        #[cfg(not(windows))]
        {
            ingest::source::stream_stdin_for_interactive()
        }
    } else {
        source
    };
    let prepared = prepare_terminal_app(config, theme_load, source, &mut terminal)?;
    let Some(mut app) = prepared else {
        terminal.restore()?;
        return Ok(());
    };
    run_interactive(&mut app, &mut terminal)?;
    restore_before_export(
        || terminal.restore(),
        || export_after_interaction(&mut app, emit_on_exit, config.color),
    )
}

fn prepare_terminal_app(
    config: &cli::Config,
    theme_load: theme::ThemeLoad,
    source: ingest::source::InputSource,
    terminal: &mut ui::terminal::TerminalSession,
) -> anyhow::Result<Option<App>> {
    let theme = theme_load.theme.clone();
    terminal.terminal_mut().draw(|frame| {
        ui::render_footer_with_theme(
            Some(&format!("Loading {}", source.display_name())),
            frame.area(),
            frame.buffer_mut(),
            &theme,
        );
    })?;
    #[cfg(feature = "elasticsearch")]
    if source.elastic_context().is_some() {
        return prepare_context_app(config, theme_load, source, terminal);
    }
    let mut select_relation = |relations: &[ingest::RelationCatalogEntry]| {
        select_table_modal(terminal, &theme, relations)
    };
    prepare_app(config, theme_load, source, Some(&mut select_relation))
}

fn export_after_interaction(
    app: &mut App,
    emit_on_exit: Option<output::OutputFormat>,
    color: output::ColorOutput,
) -> anyhow::Result<()> {
    emit_diagnostics(&app.diagnostics);
    let Some(format) = emit_on_exit else {
        return Ok(());
    };
    app.view.await_latest_source_query()?;
    emit_source_result_warnings(&app.view);
    let requirements = output::requirements(format, color)?;
    let frozen = app.view.prepare_projection(requirements, &app.theme)?;
    emit_diagnostics(&frozen.diagnostics);
    output::write_prepared_to_stdout(format, color, &frozen.output, None)
}

#[cfg(feature = "saved-views")]
fn emit_saved_view_warnings(invocation: &saved_views::SavedViewInvocation) {
    if let saved_views::SavedViewInvocation::Enabled { warnings, .. } = invocation {
        emit_diagnostics(
            &warnings
                .iter()
                .map(format_saved_view_warning)
                .collect::<Vec<_>>(),
        );
    }
}

fn emit_theme_warnings(theme_load: &theme::ThemeLoad) {
    for warning in &theme_load.warnings {
        eprintln!("theme warning: {}: {}", warning.field, warning.message);
    }
}

pub(super) fn restore_before_export(
    restore: impl FnOnce() -> std::io::Result<()>,
    export: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    restore()?;
    export()
}

fn emit_diagnostics(diagnostics: &[String]) {
    for diagnostic in diagnostics {
        eprintln!("{diagnostic}");
    }
}

fn emit_source_result_warnings(view: &view::TableView) {
    if view.source_result_is_partial() {
        eprintln!("warning: source returned a partial result");
    }
    for warning in view.source_result_warnings() {
        eprintln!("warning: {warning}");
    }
}
