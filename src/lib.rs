pub mod cli;
pub mod command;
pub mod ingest;
pub mod ops;
pub mod output;
#[cfg(feature = "saved-views")]
pub mod saved_views;
pub mod table;
pub mod theme;
pub mod ui;
pub mod view;

use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyModifiers};
use ops::filter::{FilterKind, FilterMode};
use std::io::IsTerminal;
#[cfg(feature = "saved-views")]
use std::path::{Path, PathBuf};
use std::time::Duration;

pub fn run(args: cli::Args) -> anyhow::Result<()> {
    let config = cli::Config::from_args(args)?;
    let execution = output::resolve_execution_mode(
        config.interactive,
        config.output,
        std::io::stdout().is_terminal(),
    );
    if (config.sorted.is_some() || config.top_lines.is_some())
        && execution != output::ExecutionMode::Batch(output::OutputFormat::Table)
    {
        anyhow::bail!("--sorted and --top-lines require direct table output");
    }
    let theme_load = theme::load_active_theme(None)?;
    let source = config.target.clone();
    match execution {
        output::ExecutionMode::Batch(format) => {
            let Some(mut app) = prepare_app(&config, theme_load, source, |_| Ok(()), None)? else {
                return Ok(());
            };
            emit_diagnostics(&app.diagnostics);
            emit_source_result_warnings(&app.view);
            if let Some(limit) = config.top_lines {
                let remaining = app.view.prepare_preview(
                    limit.get(),
                    config.color == output::ColorOutput::Always,
                    app.open_options.schema_scan == ingest::SchemaScan::Full,
                )?;
                output::write_preview_to_stdout(config.color, &mut app.view, &app.theme, remaining)
            } else {
                output::write_view_to_stdout(format, config.color, &mut app.view, &app.theme)
            }
        }
        output::ExecutionMode::Interactive { emit_on_exit } => {
            ui::terminal::TerminalSession::ensure_available()?;
            let source_is_stdin = source == ingest::source::InputSource::Stdin;
            let mut terminal = ui::terminal::TerminalSession::enter(source_is_stdin)?;
            let source = if source == ingest::source::InputSource::Stdin {
                #[cfg(windows)]
                {
                    ingest::source::stream_reader_for_interactive(
                        terminal.take_data_stdin_reader()?,
                    )
                }
                #[cfg(not(windows))]
                {
                    ingest::source::stream_stdin_for_interactive()
                }
            } else {
                source
            };
            let theme = theme_load.theme.clone();
            terminal.terminal_mut().draw(|frame| {
                ui::render_footer_with_theme(
                    Some(&format!("Loading {}", source.display_name())),
                    frame.area(),
                    frame.buffer_mut(),
                    &theme,
                );
            })?;
            let mut select_relation = |relations: &[ingest::RelationCatalogEntry]| {
                select_table_modal(&mut terminal, &theme, relations)
            };
            let Some(mut app) = prepare_app(
                &config,
                theme_load,
                source,
                |_| Ok(()),
                Some(&mut select_relation),
            )?
            else {
                terminal.restore()?;
                return Ok(());
            };
            run_interactive(&mut app, &mut terminal)?;
            restore_before_export(
                || terminal.restore(),
                || {
                    emit_diagnostics(&app.diagnostics);
                    if let Some(format) = emit_on_exit {
                        app.view.await_latest_source_query()?;
                        emit_source_result_warnings(&app.view);
                        output::write_view_to_stdout(
                            format,
                            config.color,
                            &mut app.view,
                            &app.theme,
                        )?;
                    }
                    Ok(())
                },
            )
        }
    }
}

fn restore_before_export(
    restore: impl FnOnce() -> std::io::Result<()>,
    export: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    restore()?;
    export()
}

fn select_table_modal(
    terminal: &mut ui::terminal::TerminalSession,
    theme: &theme::ResolvedTheme,
    relations: &[ingest::RelationCatalogEntry],
) -> anyhow::Result<Option<String>> {
    let selectable = relations
        .iter()
        .enumerate()
        .filter_map(|(index, relation)| relation.is_selectable().then_some(index))
        .collect::<Vec<_>>();
    if selectable.is_empty() {
        return Ok(None);
    }
    let mut selected = selectable[0];
    loop {
        terminal.terminal_mut().draw(|frame| {
            ui::render_relation_picker_with_theme(
                relations,
                selected,
                popup_area(frame.area()),
                frame.buffer_mut(),
                theme,
            );
        })?;
        let Event::Key(event) = read()? else {
            continue;
        };
        match event.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(None),
            KeyCode::Enter => {
                return Ok(Some(relations[selected].metadata.name.clone()));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let position = selectable
                    .iter()
                    .position(|index| *index == selected)
                    .unwrap_or_default();
                selected = selectable[position.saturating_sub(1)];
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let position = selectable
                    .iter()
                    .position(|index| *index == selected)
                    .unwrap_or_default();
                selected = selectable[(position + 1).min(selectable.len() - 1)];
            }
            _ => {}
        }
    }
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

type RelationSelector<'a> =
    dyn FnMut(&[ingest::RelationCatalogEntry]) -> anyhow::Result<Option<String>> + 'a;

fn prepare_app(
    config: &cli::Config,
    theme_load: theme::ThemeLoad,
    source: ingest::source::InputSource,
    mut report_status: impl FnMut(&str) -> anyhow::Result<()>,
    mut select_relation: Option<&mut RelationSelector<'_>>,
) -> anyhow::Result<Option<App>> {
    report_status(&format!("Loading {}", source.display_name()))?;
    let parse_options = ingest::ParseOptions {
        encoding: config.encoding.clone(),
        delimiter: config.delimiter,
        quoting: config.quoting,
        quote_char: config.quote_char,
    };
    #[cfg(feature = "saved-views")]
    let saved_source_options = selected_saved_view_source_options(config)?;
    #[cfg(not(feature = "saved-views"))]
    let saved_source_options = ingest::SourceOptionOverrides::default();
    let mut open_options = ingest::OpenOptions::merge(
        ingest::OpenOptions::default(),
        &saved_source_options,
        &config.source_options,
    );
    open_options.preview = config.top_lines.is_some();
    open_options.delimited = parse_options.clone();
    open_options.validate()?;
    if let Some(schema_status) = full_schema_scan_status(&source, &open_options) {
        report_status(&schema_status)?;
    }
    let mut opened_source = ingest::open_source(source.clone(), &open_options)?;
    #[cfg(feature = "elasticsearch")]
    if !opened_source.has_selected_table()
        && select_relation.is_none()
        && open_options.format == ingest::InputFormat::Elasticsearch
    {
        anyhow::bail!("direct Elasticsearch output requires --table or --query");
    }
    #[cfg(feature = "elasticsearch")]
    if !opened_source.has_selected_table()
        && opened_source.selectable_relations().next().is_none()
        && open_options.format == ingest::InputFormat::Elasticsearch
    {
        anyhow::bail!("no visible open Elasticsearch indices or data streams are available");
    }
    if !opened_source.has_selected_table() && opened_source.selectable_relations().count() > 0 {
        if let Some(selector) = select_relation.as_mut() {
            let Some(selected) = selector(opened_source.list_relations())? else {
                return Ok(None);
            };
            opened_source.open_relation(&selected)?;
            open_options.table = Some(selected);
        }
    }
    let opened = opened_source.into_implicit_table()?;
    let viewport = view::Viewport::new(if config.top_lines.is_some() { 1 } else { 20 }, 8);
    let mut view = view::TableView::from_opened_table(opened, viewport)?;
    if config.top_lines.is_some() {
        view.defer_preview_preparation();
    }
    view = view.with_column_width_mode(config.width);
    #[cfg(feature = "saved-views")]
    let saved_view = apply_saved_view(config, &mut view)?;
    #[cfg(feature = "saved-views")]
    let mut diagnostics = saved_view
        .as_ref()
        .map(|saved_view| saved_view.messages.clone())
        .unwrap_or_default();
    #[cfg(not(feature = "saved-views"))]
    let mut diagnostics = Vec::new();
    diagnostics.extend(
        theme_load
            .warnings
            .iter()
            .map(|warning| format!("theme warning: {}: {}", warning.field, warning.message)),
    );
    let message = diagnostics.first().cloned();
    if config.top_lines.is_none() {
        view.goto_user_row(config.start_position.row.max(1));
        if let Some(column) = config.start_position.column {
            view.goto_user_column(column.max(1));
        }
    }

    Ok(Some(App {
        source,
        open_options,
        view,
        popup: None,
        filter_prompt: None,
        column_info: None,
        source_modal: None,
        search_query: String::new(),
        keys: command::KeyInterpreter::default(),
        message,
        diagnostics,
        theme: theme_load.theme,
        #[cfg(feature = "saved-views")]
        saved_view,
        #[cfg(feature = "saved-views")]
        view_modal: None,
    }))
}

fn run_interactive(
    app: &mut App,
    terminal: &mut ui::terminal::TerminalSession,
) -> anyhow::Result<()> {
    loop {
        app.view.poll_source_query();
        let source_status = app.view.take_source_status();
        terminal.terminal_mut().draw(|frame| {
            let area = frame.area();
            let table_area = table_area(area);
            let count_status = app.source_count_status();
            ui::render_table_with_theme(
                &mut app.view,
                table_area,
                frame.buffer_mut(),
                &app.theme,
                Some(&app.search_query),
            );
            ui::render_footer_with_theme(
                Some(footer_status(
                    source_status.as_deref(),
                    app.message.as_deref(),
                    &count_status,
                )),
                area,
                frame.buffer_mut(),
                &app.theme,
            );
            match app.popup {
                Some(ui::Popup::Help) => ui::render_help_popup_with_theme(
                    &command::default_key_bindings(),
                    help_popup_area(area),
                    frame.buffer_mut(),
                    &app.theme,
                ),
                Some(ui::Popup::Cell) => {
                    if let Some(cell) = current_cell(&app.view) {
                        ui::render_cell_popup_with_theme(
                            &cell,
                            "Cell",
                            popup_area(area),
                            frame.buffer_mut(),
                            &app.theme,
                        );
                    }
                }
                Some(ui::Popup::Info) => {
                    ui::render_info_popup_with_theme(
                        &app.info_text(),
                        popup_area(area),
                        frame.buffer_mut(),
                        &app.theme,
                    );
                }
                Some(ui::Popup::Search) => {
                    ui::render_search_prompt_with_theme(
                        &app.search_query,
                        popup_area(area),
                        frame.buffer_mut(),
                        &app.theme,
                    );
                }
                Some(ui::Popup::Filter) => {
                    if let Some(prompt) = &app.filter_prompt {
                        ui::render_filter_prompt_with_theme(
                            &FilterPromptView::from(prompt),
                            popup_area(area),
                            frame.buffer_mut(),
                            &app.theme,
                        );
                    }
                }
                Some(ui::Popup::ColumnInfo) => {
                    if let Some(modal) = &app.column_info {
                        ui::render_column_info_popup_with_theme(
                            &modal.popup(),
                            popup_area(area),
                            frame.buffer_mut(),
                            &app.theme,
                        );
                    }
                }
                Some(ui::Popup::SourceConfig) => {
                    ui::render_configuration_popup_with_theme(
                        "Source Configuration",
                        &app.source_modal_body(),
                        &["Apply", "Cancel"],
                        popup_area(area),
                        frame.buffer_mut(),
                        &app.theme,
                    );
                }
                Some(ui::Popup::ViewConfig) => {
                    ui::render_configuration_popup_with_theme(
                        "View Configuration",
                        &format!(
                            "{}\n\nx: clear view filters/sorts\nn: toggle null placement\nColumn Info (i) edits the current column.",
                            app.view.view_transform_summary()
                        ),
                        &["Close"],
                        popup_area(area),
                        frame.buffer_mut(),
                        &app.theme,
                    );
                }
                Some(ui::Popup::Query) => {
                    ui::render_configuration_popup_with_theme(
                        "Source Query",
                        &app.query_modal_body(),
                        QUERY_POPUP_ACTIONS,
                        popup_area(area),
                        frame.buffer_mut(),
                        &app.theme,
                    );
                }
                #[cfg(feature = "saved-views")]
                Some(ui::Popup::SavedView) => {
                    if let Some(modal) = &app.view_modal {
                        ui::render_saved_view_popup_with_theme(
                            &modal.filename,
                            &modal.yaml,
                            modal.scroll,
                            modal.confirming_overwrite,
                            popup_area(area),
                            frame.buffer_mut(),
                            &app.theme,
                        );
                    }
                }
                None => {}
            }
        })?;

        let input_ready = if app.view.source_query_is_pending()
            || (app.source.is_streaming()
                && !matches!(app.view.row_count_state(), crate::table::RowCount::Exact(_)))
        {
            poll(Duration::from_millis(100))?
        } else {
            true
        };
        if input_ready {
            if let Event::Key(event) = read()? {
                if app.handle_key(event)? {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn full_schema_scan_status(
    source: &ingest::source::InputSource,
    options: &ingest::OpenOptions,
) -> Option<String> {
    let structured_hint = match options.format {
        ingest::InputFormat::Json | ingest::InputFormat::Ndjson => true,
        ingest::InputFormat::Delimited => false,
        #[cfg(feature = "sqlite")]
        ingest::InputFormat::Sqlite => false,
        #[cfg(feature = "elasticsearch")]
        ingest::InputFormat::Elasticsearch => false,
        ingest::InputFormat::Auto => {
            options.json_path.is_some()
                || matches!(
                    source,
                    ingest::source::InputSource::Path(path)
                        if path
                            .extension()
                            .and_then(|extension| extension.to_str())
                            .is_some_and(|extension| {
                                matches!(
                                    extension.to_ascii_lowercase().as_str(),
                                    "json" | "ndjson" | "jsonl"
                                )
                            })
                )
        }
    };
    (options.schema_scan == ingest::SchemaScan::Full && structured_hint)
        .then(|| format!("Scanning full schema for {}", source.display_name()))
}

#[cfg(feature = "saved-views")]
fn selected_saved_view_source_options(
    config: &cli::Config,
) -> anyhow::Result<ingest::SourceOptionOverrides> {
    use crate::cli::SavedViewSelection as CliSavedViewSelection;
    use crate::saved_views::SavedViewSelection;

    let target_identity = PathBuf::from(config.target.saved_view_filename());
    let selection = match &config.saved_view {
        CliSavedViewSelection::Disabled => return Ok(ingest::SourceOptionOverrides::default()),
        CliSavedViewSelection::Auto => SavedViewSelection::Auto {
            input_path: &target_identity,
        },
        CliSavedViewSelection::Force(name) => SavedViewSelection::Force { name },
    };
    let discovered = saved_views::discover_saved_views(None);
    let Some(selected) = saved_views::select_saved_view(&discovered.views, selection) else {
        if let CliSavedViewSelection::Force(name) = &config.saved_view {
            anyhow::bail!("saved view '{name}' was requested but was not found");
        }
        return Ok(ingest::SourceOptionOverrides::default());
    };
    Ok(selected.view.view.source_options())
}

struct App {
    source: ingest::source::InputSource,
    open_options: ingest::OpenOptions,
    view: view::TableView,
    popup: Option<ui::Popup>,
    filter_prompt: Option<FilterPrompt>,
    column_info: Option<ColumnInfoModal>,
    source_modal: Option<SourceConfigModal>,
    search_query: String,
    keys: command::KeyInterpreter,
    message: Option<String>,
    diagnostics: Vec<String>,
    theme: theme::ResolvedTheme,
    #[cfg(feature = "saved-views")]
    saved_view: Option<SavedViewRuntime>,
    #[cfg(feature = "saved-views")]
    view_modal: Option<ViewModal>,
}

const QUERY_POPUP_ACTIONS: &[&str] = &["Copy (y)", "Close (Enter/Esc)"];

#[derive(Debug, Clone)]
struct SourceConfigModal {
    draft: crate::table::SourceQuery,
    column: usize,
    operator: crate::table::SourceFilterOperator,
    filter_input: String,
    editing_filter: bool,
    query_input: String,
    editing_query: bool,
    native_query_changed: bool,
    error: Option<String>,
}

#[cfg(feature = "saved-views")]
#[derive(Debug, Clone)]
struct SavedViewRuntime {
    source_path: Option<PathBuf>,
    target_path: Option<PathBuf>,
    view_name: String,
    explicit_locale: Option<String>,
    messages: Vec<String>,
}

#[cfg(feature = "saved-views")]
#[derive(Debug, Clone)]
struct ViewModal {
    filename: String,
    yaml: String,
    scroll: usize,
    confirming_overwrite: bool,
}

impl App {
    fn handle_key(&mut self, event: KeyEvent) -> anyhow::Result<bool> {
        if self.popup == Some(ui::Popup::Search) {
            self.handle_search_key(event);
            return Ok(false);
        }
        if self.popup == Some(ui::Popup::Filter) {
            self.handle_filter_key(event);
            return Ok(false);
        }
        if self.popup == Some(ui::Popup::ColumnInfo) {
            self.handle_column_info_key(event);
            return Ok(false);
        }
        if self.popup == Some(ui::Popup::SourceConfig) {
            self.handle_source_config_key(event);
            return Ok(false);
        }
        if self.popup == Some(ui::Popup::ViewConfig) {
            match event.code {
                KeyCode::Esc | KeyCode::Enter => self.popup = None,
                KeyCode::Char('x') => self.view.clear_view_operations(),
                KeyCode::Char('n') => self.view.toggle_view_null_placement(),
                KeyCode::Char('i') => self.open_column_info_modal(),
                _ => {}
            }
            return Ok(false);
        }
        if self.popup == Some(ui::Popup::Query) {
            match event.code {
                KeyCode::Esc | KeyCode::Enter => self.popup = None,
                KeyCode::Char('y') => {
                    let sql = self
                        .view
                        .source_query_provenance()
                        .map(|provenance| provenance.copyable.clone());
                    let _ = ops::clipboard::yank_text(sql.as_deref());
                }
                _ => {}
            }
            return Ok(false);
        }
        #[cfg(feature = "saved-views")]
        if self.popup == Some(ui::Popup::SavedView) {
            self.handle_saved_view_key(event);
            return Ok(false);
        }

        if self.popup.is_some() {
            if closes_popup(event) {
                self.popup = None;
            }
            return Ok(false);
        }

        let Some(action) = self.key_action(event) else {
            return Ok(false);
        };

        self.apply(action)?;
        Ok(action.command == command::Command::Quit)
    }

    fn key_action(&mut self, event: KeyEvent) -> Option<command::KeyAction> {
        if event.modifiers == KeyModifiers::CONTROL {
            return command::lookup_key_event(event).map(|command| command::KeyAction {
                command,
                count: None,
            });
        }

        match event.code {
            KeyCode::Char(ch) => self.keys.handle_char(ch),
            _ => command::lookup_key_event(event).map(|command| command::KeyAction {
                command,
                count: None,
            }),
        }
    }

    fn apply(&mut self, action: command::KeyAction) -> anyhow::Result<()> {
        use command::Command;
        use ops::search::SearchDirection;
        use ops::skip::{Axis, Direction};
        use ops::sort::{SortDirection, SortMode};

        let count = action.count.unwrap_or(1);
        match action.command {
            Command::Quit => {}
            Command::Reload => self.reload()?,
            Command::MoveUp => self.view.move_by(-(count as isize), 0),
            Command::MoveDown => self.view.move_by(count as isize, 0),
            Command::MoveLeft => self.view.move_by(0, -(count as isize)),
            Command::MoveRight => self.view.move_by(0, count as isize),
            Command::Help => self.popup = Some(ui::Popup::Help),
            Command::PageUp => self.view.page_by(-1, 0, count),
            Command::PageDown => self.view.page_by(1, 0, count),
            Command::PageLeft => self.view.page_by(0, -1, count),
            Command::PageRight => self.view.page_by(0, 1, count),
            Command::LineHome => self.view.goto(self.view.cursor().row, 0),
            Command::LineEnd => {
                self.view.goto(
                    self.view.cursor().row,
                    self.view.column_count().saturating_sub(1),
                );
            }
            Command::GotoTop => self.view.goto_top(),
            Command::GotoRow => {
                if let Some(row) = action.count {
                    self.view.goto_user_row(row);
                } else {
                    self.view.goto_bottom();
                }
            }
            Command::GotoColumn => {
                if let Some(column) = action.count {
                    self.view.goto_user_column(column);
                } else {
                    self.view.goto(self.view.cursor().row, 0);
                }
            }
            Command::Mark => self.view.set_mark(),
            Command::GotoMark => self.view.goto_mark(),
            Command::ShowCell => {
                if current_cell(&self.view).is_some_and(|cell| !cell.is_empty()) {
                    self.popup = Some(ui::Popup::Cell);
                }
            }
            Command::Search => {
                self.search_query.clear();
                self.popup = Some(ui::Popup::Search);
            }
            Command::ColumnInfo => self.open_column_info_modal(),
            Command::SourceConfig => self.open_source_config_modal(),
            Command::ViewConfig => self.popup = Some(ui::Popup::ViewConfig),
            Command::Query => self.popup = Some(ui::Popup::Query),
            #[cfg(feature = "saved-views")]
            Command::SavedView => self.open_saved_view_modal(),
            Command::FilterIn => self.open_filter_prompt(FilterMode::In),
            Command::FilterOut => self.open_filter_prompt(FilterMode::Out),
            Command::NextSearchResult => self.search(SearchDirection::Forward),
            Command::PreviousSearchResult => self.search(SearchDirection::Reverse),
            Command::ToggleHeader => self.view.toggle_header(),
            Command::GapDown => self.view.adjust_column_gap(-(count as isize)),
            Command::GapUp => self.view.adjust_column_gap(count as isize),
            Command::AllColumnsNarrower => self.view.adjust_all_column_widths(-(count as isize)),
            Command::AllColumnsWider => self.view.adjust_all_column_widths(count as isize),
            Command::CurrentColumnNarrower => {
                self.view.adjust_current_column_width(-(count as isize));
            }
            Command::CurrentColumnWider => self.view.adjust_current_column_width(count as isize),
            Command::SortNaturalAsc => self
                .view
                .sort_current_column(SortMode::Natural, SortDirection::Ascending),
            Command::SortNaturalDesc => self
                .view
                .sort_current_column(SortMode::Natural, SortDirection::Descending),
            Command::SortNumericAsc => self
                .view
                .sort_current_column(SortMode::Numeric, SortDirection::Ascending),
            Command::SortNumericDesc => self
                .view
                .sort_current_column(SortMode::Numeric, SortDirection::Descending),
            Command::SortLexicalAsc => self
                .view
                .sort_current_column(SortMode::Lexical, SortDirection::Ascending),
            Command::SortLexicalDesc => self
                .view
                .sort_current_column(SortMode::Lexical, SortDirection::Descending),
            Command::YankCell => {
                let rendered = self.view.current_cell_rendered();
                let _ = ops::clipboard::yank_text(rendered.as_deref());
            }
            Command::YankRawCell => {
                let _ = ops::clipboard::yank_text(self.view.current_raw_cell());
            }
            Command::ToggleColumnWidthMode => {
                if let Some(width) = action.count {
                    self.view.set_all_column_widths(width);
                } else {
                    self.view.toggle_variable_column_width_mode();
                }
            }
            Command::SetCurrentColumnWidth => {
                if let Some(width) = action.count {
                    self.view.set_current_column_width(width);
                } else {
                    self.view.maximize_current_column_width();
                }
            }
            Command::ColumnHideLeft => self.view.hide_columns_left(count),
            Command::ColumnHideRight => self.view.hide_columns_right(count),
            Command::ColumnHideCurrent => self.view.hide_current_column(),
            Command::ColumnShowLeft => self.view.show_hidden_left(count),
            Command::ColumnShowRight => self.view.show_hidden_right(count),
            Command::ColumnSortAsc => {
                let mode = if self.view.is_numeric_column(self.view.cursor().column) {
                    SortMode::Numeric
                } else {
                    SortMode::Lexical
                };
                self.view
                    .sort_current_column(mode, SortDirection::Ascending);
            }
            Command::ColumnSortDesc => {
                let mode = if self.view.is_numeric_column(self.view.cursor().column) {
                    SortMode::Numeric
                } else {
                    SortMode::Lexical
                };
                self.view
                    .sort_current_column(mode, SortDirection::Descending);
            }
            Command::ColumnSortClear => self.view.clear_current_column_sort(),
            Command::SkipRowChangeForward => {
                let position =
                    self.view
                        .progressive_skip_to_change(Axis::Row, Direction::Forward, count);
                self.view.goto(position.row, position.column);
            }
            Command::SkipRowChangeBackward => {
                let position =
                    self.view
                        .progressive_skip_to_change(Axis::Row, Direction::Backward, count);
                self.view.goto(position.row, position.column);
            }
            Command::SkipColumnChangeForward => {
                let position =
                    self.view
                        .progressive_skip_to_change(Axis::Column, Direction::Forward, count);
                self.view.goto(position.row, position.column);
            }
            Command::SkipColumnChangeBackward => {
                let position =
                    self.view
                        .progressive_skip_to_change(Axis::Column, Direction::Backward, count);
                self.view.goto(position.row, position.column);
            }
            Command::ShowInfo => self.popup = Some(ui::Popup::Info),
            Command::Redraw => {}
        }
        Ok(())
    }

    fn handle_search_key(&mut self, event: KeyEvent) {
        match event.code {
            KeyCode::Esc | KeyCode::Enter => self.popup = None,
            KeyCode::Char('\n' | '\r') => self.popup = None,
            KeyCode::Backspace => {
                self.search_query.pop();
                self.search_current_or_next();
            }
            KeyCode::Char(ch)
                if event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT =>
            {
                self.search_query.push(ch);
                self.search_current_or_next();
            }
            _ => {}
        }
    }

    fn open_source_config_modal(&mut self) {
        let Some(query) = self.view.active_source_query().cloned() else {
            self.message =
                Some("This source does not expose a configurable bounded source query".to_owned());
            return;
        };
        let column = self
            .view
            .table_definition()
            .map(|definition| {
                self.view
                    .cursor()
                    .column
                    .min(definition.columns.len().saturating_sub(1))
            })
            .unwrap_or_default();
        let query_input = query.native_query.clone().unwrap_or_default();
        self.source_modal = Some(SourceConfigModal {
            draft: query,
            column,
            operator: crate::table::SourceFilterOperator::Equal,
            filter_input: String::new(),
            editing_filter: false,
            query_input,
            editing_query: false,
            native_query_changed: false,
            error: None,
        });
        self.popup = Some(ui::Popup::SourceConfig);
    }

    fn source_modal_body(&self) -> String {
        let Some(modal) = &self.source_modal else {
            return "Source configuration is unavailable".to_owned();
        };
        let relation = self
            .view
            .table_definition()
            .map(|definition| definition.relation.display_name.as_str())
            .unwrap_or("source");
        let column = self
            .view
            .table_definition()
            .and_then(|definition| definition.columns.get(modal.column))
            .map(|column| column.display_name.as_str())
            .unwrap_or("none");
        let extent = source_extent_label(
            self.view.source_result_extent(),
            self.view.source_query_is_pending(),
        );
        let capabilities = self.view.source_capabilities();
        let sort_capability = match &capabilities.sorting {
            crate::table::CapabilityStatus::Supported => "supported".to_owned(),
            crate::table::CapabilityStatus::Unavailable { reason } => {
                format!("unavailable: {reason}")
            }
        };
        let sql = self
            .view
            .source_query_provenance()
            .map(|provenance| format!("\n\n{}:\n{}", provenance.language, provenance.logical))
            .unwrap_or_default();
        let editor = if modal.editing_query {
            format!(
                "\n\nNative query editor:\n{}\nEnter: stage query  Esc: cancel editor",
                modal.query_input
            )
        } else if modal.editing_filter {
            format!(
                "\n\nFilter editor: {} {} {}\nTab: operator  Enter: add  Esc: cancel editor",
                column, modal.operator, modal.filter_input
            )
        } else {
            "\n\n←/→: column  f: add filter  q: edit native query  Tab: operator\ns/S: source sort asc/desc  x: clear column\n+/-: adjust limit  Enter: apply  Esc: cancel"
                .to_owned()
        };
        let limit = if modal.draft.limit == std::num::NonZeroUsize::MAX {
            "unbounded".to_owned()
        } else {
            modal.draft.limit.to_string()
        };
        let partial = if self.view.source_result_is_partial() {
            "  Partial: yes"
        } else {
            ""
        };
        let mapping_fields = self.view.source_field_catalog();
        let mapping = if mapping_fields.is_empty() {
            "Mapping fields: result schema only".to_owned()
        } else {
            let selected = mapping_fields
                .iter()
                .find(|field| field.name == column)
                .map(|field| {
                    format!(
                        "\nSelected mapping: {} [{}] searchable={} aggregatable={}{}{}{}",
                        field.name,
                        field.source_types.join("|"),
                        field.searchable,
                        field.aggregatable,
                        if field.conflict { " conflict" } else { "" },
                        if field.runtime { " runtime" } else { "" },
                        if field.multifield { " multifield" } else { "" },
                    )
                })
                .unwrap_or_default();
            format!("Mapping fields: {}{selected}", mapping_fields.len())
        };
        format!(
            "Endpoint: {}\nSource: {relation}\nLimit: {}  Extent: {extent}{partial}\nResult fields: {}\n{mapping}\nFilters: {}  Sort keys: {}\nSelected column: {column}\nSource sorting: {sort_capability}{}{}{}",
            self.source.safe_identity(),
            limit,
            self.view.column_count(),
            modal.draft.filters.len(),
            modal.draft.order_by.len(),
            editor,
            modal
                .error
                .as_ref()
                .map(|error| format!("\nError: {error}"))
                .unwrap_or_default(),
            sql
        )
    }

    fn query_modal_body(&self) -> String {
        let view_note = format!(
            "\n\nLocal transformations not represented by the native query:\n{}",
            self.view.view_transform_summary()
        );
        self.view
            .source_query_provenance()
            .map(|provenance| {
                format!(
                    "Parameterized {}:\n{}\n\nParameters: {:?}\n\nCopyable {}:\n{}{}",
                    provenance.language,
                    provenance.logical,
                    provenance.parameters,
                    provenance.language,
                    provenance.copyable,
                    view_note
                )
            })
            .unwrap_or_else(|| format!("This source has no native query artifact.{}", view_note))
    }

    fn handle_source_config_key(&mut self, event: KeyEvent) {
        let Some(mut modal) = self.source_modal.take() else {
            self.popup = None;
            return;
        };
        if modal.editing_query {
            match event.code {
                KeyCode::Esc => {
                    modal.editing_query = false;
                    modal.query_input = modal.draft.native_query.clone().unwrap_or_default();
                    modal.error = None;
                }
                KeyCode::Backspace => {
                    modal.query_input.pop();
                    modal.error = None;
                }
                KeyCode::Enter => {
                    let query = modal.query_input.trim();
                    if query.is_empty() {
                        modal.error = Some("native query cannot be empty".to_owned());
                    } else {
                        modal.draft.native_query = Some(query.to_owned());
                        modal.native_query_changed = true;
                        modal.editing_query = false;
                        modal.error = None;
                    }
                }
                KeyCode::Char(ch)
                    if event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT =>
                {
                    modal.query_input.push(ch);
                    modal.error = None;
                }
                _ => {}
            }
            self.source_modal = Some(modal);
            return;
        }
        if modal.editing_filter {
            match event.code {
                KeyCode::Esc => {
                    modal.editing_filter = false;
                    modal.error = None;
                }
                KeyCode::Tab => modal.operator = next_source_operator(modal.operator),
                KeyCode::Backspace => {
                    modal.filter_input.pop();
                    modal.error = None;
                }
                KeyCode::Enter => {
                    let operand = if modal.operator.requires_operand() {
                        if modal.filter_input.is_empty() {
                            modal.error = Some("filter value is required".to_owned());
                            self.source_modal = Some(modal);
                            return;
                        }
                        Some(parse_source_operand(&modal.filter_input))
                    } else {
                        None
                    };
                    if !self
                        .view
                        .source_capabilities()
                        .supports_filter(modal.operator)
                    {
                        modal.error =
                            Some(format!("source filter '{}' is unavailable", modal.operator));
                    } else if let Some(column) = self
                        .view
                        .table_definition()
                        .and_then(|definition| definition.columns.get(modal.column))
                    {
                        modal.draft.filters.push(crate::table::SourceFilter {
                            scope: crate::table::SourceFilterScope::Column(column.id),
                            operator: modal.operator,
                            operand,
                        });
                        modal.filter_input.clear();
                        modal.editing_filter = false;
                        modal.error = None;
                    }
                }
                KeyCode::Char(ch)
                    if event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT =>
                {
                    modal.filter_input.push(ch);
                    modal.error = None;
                }
                _ => {}
            }
            self.source_modal = Some(modal);
            return;
        }

        match event.code {
            KeyCode::Esc => {
                self.popup = None;
                return;
            }
            KeyCode::Enter => {
                let query = modal.draft.clone();
                if self.view.active_source_query() == Some(&query) {
                    self.popup = None;
                    return;
                }
                let source_filters = query
                    .filters
                    .iter()
                    .map(|filter| {
                        let crate::table::SourceFilterScope::Column(column) = filter.scope else {
                            return Some(ingest::SourceFilterRequest {
                                column: "*".to_owned(),
                                operator: filter.operator,
                                operand: filter.operand.clone(),
                            });
                        };
                        Some(ingest::SourceFilterRequest {
                            column: self.view.source_column_name_for_id(column)?,
                            operator: filter.operator,
                            operand: filter.operand.clone(),
                        })
                    })
                    .collect::<Option<Vec<_>>>();
                let source_sort = query
                    .order_by
                    .iter()
                    .map(|sort| {
                        Some(ingest::SourceSortRequest {
                            column: self.view.source_column_name_for_id(sort.column)?,
                            direction: sort.direction,
                        })
                    })
                    .collect::<Option<Vec<_>>>();
                let (Some(source_filters), Some(source_sort)) = (source_filters, source_sort)
                else {
                    modal.error = Some("source query references an unavailable column".to_owned());
                    self.source_modal = Some(modal);
                    return;
                };
                if self.view.request_source_query(query.clone()) {
                    self.open_options.limit =
                        (query.limit != std::num::NonZeroUsize::MAX).then_some(query.limit);
                    self.open_options.source_filters = source_filters;
                    self.open_options.source_sort = source_sort;
                    if modal.native_query_changed {
                        self.open_options.native_query = query.native_query.clone();
                        self.open_options.table = None;
                    }
                    self.popup = None;
                    return;
                }
                modal.error = self.view.take_source_status();
            }
            KeyCode::Left | KeyCode::Char('h') => {
                modal.column = modal.column.saturating_sub(1);
            }
            KeyCode::Right | KeyCode::Char('l') => {
                let last = self
                    .view
                    .table_definition()
                    .map(|definition| definition.columns.len().saturating_sub(1))
                    .unwrap_or_default();
                modal.column = (modal.column + 1).min(last);
            }
            KeyCode::Char('f') => {
                modal.editing_filter = true;
                modal.error = None;
            }
            KeyCode::Char('q') => {
                if modal.draft.native_query.is_none() {
                    modal.error =
                        Some("this source does not expose an editable native query".to_owned());
                } else {
                    modal.query_input = modal.draft.native_query.clone().unwrap_or_default();
                    modal.editing_query = true;
                    modal.error = None;
                }
            }
            KeyCode::Tab => modal.operator = next_source_operator(modal.operator),
            KeyCode::Char('s' | 'S') => {
                if !self.view.source_capabilities().sorting.is_supported() {
                    modal.error = Some("source sorting is unavailable".to_owned());
                } else if let Some(column) = self
                    .view
                    .table_definition()
                    .and_then(|definition| definition.columns.get(modal.column))
                {
                    modal.draft.order_by.retain(|sort| sort.column != column.id);
                    modal.draft.order_by.push(crate::table::SourceSort {
                        column: column.id,
                        direction: if event.code == KeyCode::Char('s') {
                            crate::table::SortDirection::Ascending
                        } else {
                            crate::table::SortDirection::Descending
                        },
                    });
                }
            }
            KeyCode::Char('x') => {
                if let Some(column) = self
                    .view
                    .table_definition()
                    .and_then(|definition| definition.columns.get(modal.column))
                {
                    modal.draft.filters.retain(|filter| {
                        !matches!(
                            filter.scope,
                            crate::table::SourceFilterScope::Column(id) if id == column.id
                        )
                    });
                    modal.draft.order_by.retain(|sort| sort.column != column.id);
                }
            }
            KeyCode::Char('+') => {
                let next = if modal.draft.limit == std::num::NonZeroUsize::MAX {
                    1_000
                } else {
                    modal.draft.limit.get().saturating_add(100)
                };
                modal.draft.limit =
                    std::num::NonZeroUsize::new(next).expect("positive source limit");
            }
            KeyCode::Char('-') => {
                let next = if modal.draft.limit == std::num::NonZeroUsize::MAX {
                    1_000
                } else {
                    modal.draft.limit.get().saturating_sub(100).max(1)
                };
                modal.draft.limit =
                    std::num::NonZeroUsize::new(next).expect("positive source limit");
            }
            _ => {}
        }
        self.source_modal = Some(modal);
    }

    fn open_filter_prompt(&mut self, mode: FilterMode) {
        let column = self.view.cursor().column;
        self.filter_prompt = Some(FilterPrompt::new(&self.view, mode, column));
        self.popup = Some(ui::Popup::Filter);
    }

    fn handle_filter_key(&mut self, event: KeyEvent) {
        let Some(prompt) = &mut self.filter_prompt else {
            self.popup = None;
            return;
        };
        match event.code {
            KeyCode::Esc => {
                self.filter_prompt = None;
                self.popup = None;
            }
            KeyCode::Enter | KeyCode::Char('\n' | '\r') => self.submit_filter_prompt(),
            KeyCode::Tab => prompt.cycle_kind(),
            KeyCode::Backspace => {
                prompt.input.pop();
                prompt.error = None;
            }
            KeyCode::Char(ch)
                if event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT =>
            {
                prompt.input.push(ch);
                prompt.error = None;
            }
            _ => {}
        }
    }

    fn submit_filter_prompt(&mut self) {
        let Some(prompt) = &self.filter_prompt else {
            self.popup = None;
            return;
        };
        if prompt.input.trim().is_empty() {
            self.view.clear_filters_for_column(prompt.column);
            self.filter_prompt = None;
            self.popup = None;
            return;
        }
        let result = self.view.apply_filter(
            prompt.column,
            prompt.mode,
            prompt.selected_kind,
            prompt.input.clone(),
        );
        match result {
            Ok(()) => {
                self.filter_prompt = None;
                self.popup = None;
            }
            Err(err) => {
                if let Some(prompt) = &mut self.filter_prompt {
                    prompt.error = Some(err.to_string());
                }
            }
        }
    }

    fn open_column_info_modal(&mut self) {
        if let Some(info) = self.view.current_column_info() {
            self.column_info = Some(ColumnInfoModal::from_info(info));
            self.popup = Some(ui::Popup::ColumnInfo);
        }
    }

    fn handle_column_info_key(&mut self, event: KeyEvent) {
        let Some(modal) = &mut self.column_info else {
            self.popup = None;
            return;
        };
        match event.code {
            KeyCode::Esc => {
                self.column_info = None;
                self.popup = None;
            }
            KeyCode::Enter | KeyCode::Char('\n' | '\r') => {
                let update = modal.to_update();
                self.view.apply_current_column_info(update);
                self.column_info = None;
                self.popup = None;
                self.message = Some("column view updated".to_owned());
            }
            KeyCode::Tab => modal.next_group(),
            KeyCode::BackTab => modal.previous_group(),
            KeyCode::Up | KeyCode::Left | KeyCode::Char('k') | KeyCode::Char('h') => {
                modal.previous_option();
            }
            KeyCode::Down | KeyCode::Right | KeyCode::Char('j') | KeyCode::Char('l') => {
                modal.next_option();
            }
            _ => {}
        }
    }

    fn search_current_or_next(&mut self) {
        if self.search_query.is_empty() {
            return;
        }
        if self.view.current_cell_matches(&self.search_query) {
            return;
        }
        self.search(ops::search::SearchDirection::Forward);
    }

    fn search(&mut self, direction: ops::search::SearchDirection) {
        if let Some(position) = self.view.progressive_search(&self.search_query, direction) {
            self.view.goto(position.row, position.column);
        }
    }

    #[cfg(feature = "saved-views")]
    fn open_saved_view_modal(&mut self) {
        let Some(saved_view) = &self.saved_view else {
            self.message = Some("saved views are disabled".to_owned());
            return;
        };
        let input_filename = self.input_filename();
        let yaml = self.view.to_saved_view_yaml_with_source_options(
            &saved_view.view_name,
            &input_filename,
            saved_view.explicit_locale.as_deref(),
            &self.open_options,
        );
        let filename = saved_view
            .target_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<no saved view path>".to_owned());
        self.view_modal = Some(ViewModal {
            filename,
            yaml,
            scroll: 0,
            confirming_overwrite: false,
        });
        self.popup = Some(ui::Popup::SavedView);
    }

    #[cfg(feature = "saved-views")]
    fn handle_saved_view_key(&mut self, event: KeyEvent) {
        match event.code {
            KeyCode::Esc => {
                self.view_modal = None;
                self.popup = None;
            }
            KeyCode::Char('s') => self.save_view_modal(false),
            KeyCode::Char('y')
                if self
                    .view_modal
                    .as_ref()
                    .is_some_and(|modal| modal.confirming_overwrite) =>
            {
                self.save_view_modal(true);
            }
            KeyCode::Char('n') => {
                if let Some(modal) = &mut self.view_modal {
                    modal.confirming_overwrite = false;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(modal) = &mut self.view_modal {
                    modal.scroll = modal.scroll.saturating_add(1);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(modal) = &mut self.view_modal {
                    modal.scroll = modal.scroll.saturating_sub(1);
                }
            }
            _ => {}
        }
    }

    #[cfg(feature = "saved-views")]
    fn save_view_modal(&mut self, confirmed_overwrite: bool) {
        let Some(saved_view) = &mut self.saved_view else {
            self.message = Some("saved views are disabled".to_owned());
            return;
        };
        let Some(target_path) = saved_view.target_path.clone() else {
            self.message = Some("saved view path is unavailable".to_owned());
            return;
        };
        let Some(modal) = &mut self.view_modal else {
            return;
        };
        if target_path.exists() && !confirmed_overwrite {
            modal.confirming_overwrite = true;
            return;
        }

        match write_saved_view_atomic(&target_path, &modal.yaml) {
            Ok(()) => {
                saved_view.source_path = Some(target_path.clone());
                saved_view.target_path = Some(target_path.clone());
                modal.confirming_overwrite = false;
                self.message = Some(format!("saved view {}", target_path.display()));
            }
            Err(err) => {
                modal.confirming_overwrite = false;
                self.message = Some(format!("failed to save view: {err}"));
                eprintln!("failed to save view {}: {err}", target_path.display());
            }
        }
    }

    #[cfg(feature = "saved-views")]
    fn input_filename(&self) -> String {
        self.source.saved_view_filename()
    }

    fn reload(&mut self) -> anyhow::Result<()> {
        if self.source.is_stdin() {
            return Ok(());
        }

        let cursor_row = self.view.cursor().row;
        let viewport = self.view.viewport();
        let opened =
            ingest::open_source(self.source.clone(), &self.open_options)?.into_implicit_table()?;
        let mut reloaded = view::TableView::from_opened_table(opened, viewport)?;
        reloaded.restore_view_settings_from(&self.view);
        let restored_column = reloaded.cursor().column;
        reloaded.goto(cursor_row, restored_column);
        self.view = reloaded;
        Ok(())
    }

    fn info_text(&self) -> String {
        let rows = match self.view.row_count_state() {
            crate::table::RowCount::Exact(count) => count.to_string(),
            crate::table::RowCount::AtLeast(count) => format!("{count}+"),
            crate::table::RowCount::Unknown => "unknown".to_owned(),
        };
        let object_mode = self
            .view
            .object_mode_resolution()
            .map(|mode| {
                if mode.requested == ingest::ObjectMode::Auto {
                    format!(
                        "\nObject mode: auto → {}{}",
                        mode.resolved,
                        if mode.resolved == ingest::ResolvedObjectMode::Entries {
                            " (use --object-mode record for one row)"
                        } else {
                            ""
                        }
                    )
                } else {
                    format!("\nObject mode: {}", mode.resolved)
                }
            })
            .unwrap_or_default();
        let mut extent = self
            .view
            .source_result_extent()
            .map(|extent| match extent {
                crate::table::ResultExtent::Complete { .. } => "complete".to_owned(),
                crate::table::ResultExtent::Truncated { limit, .. } => {
                    format!("limited to {limit}")
                }
            })
            .unwrap_or_else(|| "in progress".to_owned());
        if self.view.source_result_is_partial() {
            extent.push_str(", partial");
        }
        format!(
            "Rows: {}\nVisible rows: {}\nFetched source rows: {}\nSource extent: {}\nColumns: {}\nPosition: {},{}\nWidth mode: {:?}\nColumn gap: {}\nMark: {}{}",
            rows,
            self.view.visible_row_count(),
            self.view.fetched_source_row_count(),
            extent,
            self.view.column_count(),
            self.view.cursor().row + 1,
            self.view.cursor().column + 1,
            self.view.column_width_mode(),
            self.view.column_gap(),
            self.view
                .mark()
                .map(|position| format!("{},{}", position.row + 1, position.column + 1))
                .unwrap_or_else(|| "none".to_owned()),
            object_mode
        )
    }

    fn source_count_status(&self) -> String {
        let mut extent = match self.view.source_result_extent() {
            Some(crate::table::ResultExtent::Complete { .. }) => "complete".to_owned(),
            Some(crate::table::ResultExtent::Truncated { limit, .. }) => {
                format!("limited {limit}")
            }
            None if self.view.source_query_is_pending() => "querying".to_owned(),
            None => "fetching".to_owned(),
        };
        if self.view.source_result_is_partial() {
            extent.push_str(", partial");
        }
        format!(
            "{} visible / {} source ({extent})",
            self.view.visible_row_count(),
            self.view.fetched_source_row_count()
        )
    }
}

fn next_source_operator(
    current: crate::table::SourceFilterOperator,
) -> crate::table::SourceFilterOperator {
    use crate::table::SourceFilterOperator as Operator;
    match current {
        Operator::Equal => Operator::NotEqual,
        Operator::NotEqual => Operator::LessThan,
        Operator::LessThan => Operator::LessThanOrEqual,
        Operator::LessThanOrEqual => Operator::GreaterThan,
        Operator::GreaterThan => Operator::GreaterThanOrEqual,
        Operator::GreaterThanOrEqual => Operator::Contains,
        Operator::Contains => Operator::Prefix,
        Operator::Prefix => Operator::IsNull,
        Operator::IsNull => Operator::IsNotNull,
        Operator::IsNotNull => Operator::Equal,
    }
}

fn parse_source_operand(value: &str) -> crate::table::SourceOperand {
    if value.eq_ignore_ascii_case("true") {
        crate::table::SourceOperand::Boolean(true)
    } else if value.eq_ignore_ascii_case("false") {
        crate::table::SourceOperand::Boolean(false)
    } else if let Ok(value) = value.parse::<i64>() {
        crate::table::SourceOperand::Integer(value)
    } else if let Ok(value) = value.parse::<f64>() {
        crate::table::SourceOperand::Float(value)
    } else {
        crate::table::SourceOperand::Text(value.to_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FilterPrompt {
    mode: FilterMode,
    column: usize,
    selected_kind: FilterKind,
    enabled_kinds: Vec<FilterKind>,
    input: String,
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnInfoGroup {
    Visibility,
    Align,
    Type,
    Format,
    Sort,
    Filters,
}

impl ColumnInfoGroup {
    const VISUAL: [Self; 6] = [
        Self::Visibility,
        Self::Format,
        Self::Align,
        Self::Sort,
        Self::Type,
        Self::Filters,
    ];
    const TAB_ORDER: [Self; 6] = [
        Self::Visibility,
        Self::Align,
        Self::Type,
        Self::Format,
        Self::Sort,
        Self::Filters,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Visibility => "Visibility",
            Self::Align => "Align",
            Self::Type => "Type",
            Self::Format => "Format",
            Self::Sort => "Sort",
            Self::Filters => "Filters",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ColumnInfoModal {
    column_name: String,
    visible_column: usize,
    source_column: usize,
    filter_details: Vec<String>,
    filter_indicator: Option<&'static str>,
    active_group: usize,
    visibility: usize,
    alignment: usize,
    column_type: usize,
    format: usize,
    sort: usize,
    nulls: view::ColumnNullPlacementChoice,
    canonical_source: Option<String>,
    source_type: Option<String>,
    source_declared_type: Option<String>,
    source_operations: Vec<String>,
    filters: usize,
}

impl ColumnInfoModal {
    fn from_info(info: view::ColumnInfo) -> Self {
        let filter_indicator = filter_indicator(&info.filters);
        Self {
            column_name: info.name,
            visible_column: info.visible_column,
            source_column: info.source_column,
            filter_details: if info.filters.is_empty() {
                vec!["None".to_owned()]
            } else {
                info.filters
                    .into_iter()
                    .map(|filter| format!("{} {} {}", filter.mode, filter.kind, filter.input))
                    .collect()
            },
            filter_indicator,
            active_group: 0,
            visibility: usize::from(!info.visible),
            alignment: match info.alignment {
                None => 0,
                Some(view::ColumnAlignment::Left) => 1,
                Some(view::ColumnAlignment::Right) => 2,
            },
            column_type: column_type_index(info.column_type),
            format: column_format_index(info.format),
            sort: match info.sort {
                view::ColumnSortChoice::None => 0,
                view::ColumnSortChoice::Ascending => 1,
                view::ColumnSortChoice::Descending => 2,
            },
            nulls: info.nulls,
            canonical_source: info.canonical_source,
            source_type: info.source_type,
            source_declared_type: info.source_declared_type,
            source_operations: info.source_operations,
            filters: 0,
        }
    }

    fn popup(&self) -> ui::ColumnInfoPopup {
        ui::ColumnInfoPopup {
            title: "Column Info".to_owned(),
            summary: format!(
                "{}  visible:{} source:{}  nulls:{:?}{}{}{}{}",
                self.column_name,
                self.visible_column + 1,
                self.source_column + 1,
                self.nulls,
                self.canonical_source
                    .as_ref()
                    .map(|value| format!(" canonical:{value}"))
                    .unwrap_or_default(),
                self.source_type
                    .as_ref()
                    .map(|value| format!(" type:{value}"))
                    .unwrap_or_default(),
                self.source_declared_type
                    .as_ref()
                    .map(|value| format!(" declared:{value}"))
                    .unwrap_or_default(),
                if self.source_operations.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", self.source_operations.join(", "))
                }
            ),
            sections: ColumnInfoGroup::VISUAL
                .into_iter()
                .map(|group| ui::ColumnInfoSection {
                    header: group.label().to_owned(),
                    active: group == self.active_group(),
                    options: self.section_options(group),
                    details: if group == ColumnInfoGroup::Filters {
                        self.filter_details.clone()
                    } else {
                        Vec::new()
                    },
                })
                .collect(),
        }
    }

    fn next_group(&mut self) {
        self.active_group = (self.active_group + 1) % ColumnInfoGroup::TAB_ORDER.len();
    }

    fn previous_group(&mut self) {
        self.active_group = self
            .active_group
            .checked_sub(1)
            .unwrap_or(ColumnInfoGroup::TAB_ORDER.len() - 1);
    }

    fn next_option(&mut self) {
        let group = self.active_group();
        let options = self.option_count(group);
        if options == 0 {
            return;
        }
        let mut index = self.selected_index(group);
        for _ in 0..options {
            index = (index + 1) % options;
            if self.option_enabled(group, index) {
                self.set_selected_index(group, index);
                break;
            }
        }
    }

    fn previous_option(&mut self) {
        let group = self.active_group();
        let options = self.option_count(group);
        if options == 0 {
            return;
        }
        let mut index = self.selected_index(group);
        for _ in 0..options {
            index = index.checked_sub(1).unwrap_or(options - 1);
            if self.option_enabled(group, index) {
                self.set_selected_index(group, index);
                break;
            }
        }
    }

    fn active_group(&self) -> ColumnInfoGroup {
        ColumnInfoGroup::TAB_ORDER[self.active_group]
    }

    fn selected_index(&self, group: ColumnInfoGroup) -> usize {
        match group {
            ColumnInfoGroup::Visibility => self.visibility,
            ColumnInfoGroup::Align => self.alignment,
            ColumnInfoGroup::Type => self.column_type,
            ColumnInfoGroup::Format => self.format,
            ColumnInfoGroup::Sort => self.sort,
            ColumnInfoGroup::Filters => self.filters,
        }
    }

    fn set_selected_index(&mut self, group: ColumnInfoGroup, index: usize) {
        match group {
            ColumnInfoGroup::Visibility => self.visibility = index,
            ColumnInfoGroup::Align => self.alignment = index,
            ColumnInfoGroup::Type => {
                self.column_type = index;
                if !self.option_enabled(ColumnInfoGroup::Format, self.format) {
                    self.format = 0;
                }
            }
            ColumnInfoGroup::Format => self.format = index,
            ColumnInfoGroup::Sort => self.sort = index,
            ColumnInfoGroup::Filters => self.filters = index,
        }
    }

    fn option_count(&self, group: ColumnInfoGroup) -> usize {
        match group {
            ColumnInfoGroup::Visibility => 2,
            ColumnInfoGroup::Align => 3,
            ColumnInfoGroup::Type => 7,
            ColumnInfoGroup::Format => 7,
            ColumnInfoGroup::Sort => 3,
            ColumnInfoGroup::Filters => usize::from(self.filter_indicator.is_some()) + 1,
        }
    }

    fn option_enabled(&self, group: ColumnInfoGroup, index: usize) -> bool {
        match group {
            ColumnInfoGroup::Format => format_valid_for_type(self.column_type, index),
            ColumnInfoGroup::Filters => self.filter_indicator.is_some(),
            _ => true,
        }
    }

    fn section_options(&self, group: ColumnInfoGroup) -> Vec<ui::ColumnInfoOption> {
        if group == ColumnInfoGroup::Filters && self.filter_indicator.is_none() {
            return Vec::new();
        }
        (0..self.option_count(group))
            .map(|idx| ui::ColumnInfoOption {
                label: self.option_label(group, idx),
                selected: idx == self.selected_index(group),
                enabled: self.option_enabled(group, idx),
            })
            .collect()
    }

    fn option_label(&self, group: ColumnInfoGroup, index: usize) -> String {
        match group {
            ColumnInfoGroup::Visibility => ["visible", "hidden"][index].to_owned(),
            ColumnInfoGroup::Align => ["auto", "left", "right"][index].to_owned(),
            ColumnInfoGroup::Type => [
                "text", "date", "ip", "float", "integer", "semver", "boolean",
            ][index]
                .to_owned(),
            ColumnInfoGroup::Format => [
                "plain",
                "locale",
                "uppercase",
                "lowercase",
                "char",
                "bit",
                "word",
            ][index]
                .to_owned(),
            ColumnInfoGroup::Sort => ["none", "▲ ascending", "▼ descending"][index].to_owned(),
            ColumnInfoGroup::Filters => {
                let indicator = self.filter_indicator.unwrap_or(" ");
                match index {
                    0 => format!("keep {indicator}"),
                    _ => format!("clear {indicator}"),
                }
            }
        }
    }

    fn to_update(&self) -> view::ColumnInfoUpdate {
        view::ColumnInfoUpdate {
            visible: self.visibility == 0,
            alignment: match self.alignment {
                1 => Some(view::ColumnAlignment::Left),
                2 => Some(view::ColumnAlignment::Right),
                _ => None,
            },
            column_type: column_type_choice(self.column_type),
            format: column_format_choice(self.format),
            sort: match self.sort {
                1 => view::ColumnSortChoice::Ascending,
                2 => view::ColumnSortChoice::Descending,
                _ => view::ColumnSortChoice::None,
            },
            nulls: self.nulls,
            clear_filters: self.filters == 1,
        }
    }
}

fn filter_indicator(filters: &[view::ColumnFilterSummary]) -> Option<&'static str> {
    let first = filters.first()?;
    if filters.len() > 1 {
        return Some("±");
    }
    Some(match first.mode {
        "in" => "+",
        "out" => "-",
        _ => "±",
    })
}

fn format_valid_for_type(column_type: usize, format: usize) -> bool {
    match format {
        0 => true,
        1 => matches!(column_type, 3 | 4),
        2 | 3 => matches!(column_type, 0 | 1 | 2 | 5),
        4..=6 => column_type == 6,
        _ => false,
    }
}

fn column_type_index(choice: view::ColumnTypeChoice) -> usize {
    match choice {
        view::ColumnTypeChoice::Text => 0,
        view::ColumnTypeChoice::Date => 1,
        view::ColumnTypeChoice::Ip => 2,
        view::ColumnTypeChoice::Float => 3,
        view::ColumnTypeChoice::Integer => 4,
        view::ColumnTypeChoice::SemVer => 5,
        view::ColumnTypeChoice::Boolean => 6,
    }
}

fn column_type_choice(index: usize) -> view::ColumnTypeChoice {
    match index {
        1 => view::ColumnTypeChoice::Date,
        2 => view::ColumnTypeChoice::Ip,
        3 => view::ColumnTypeChoice::Float,
        4 => view::ColumnTypeChoice::Integer,
        5 => view::ColumnTypeChoice::SemVer,
        6 => view::ColumnTypeChoice::Boolean,
        _ => view::ColumnTypeChoice::Text,
    }
}

fn column_format_index(choice: view::ColumnFormatChoice) -> usize {
    match choice {
        view::ColumnFormatChoice::Plain => 0,
        view::ColumnFormatChoice::Locale => 1,
        view::ColumnFormatChoice::Uppercase => 2,
        view::ColumnFormatChoice::Lowercase => 3,
        view::ColumnFormatChoice::Char => 4,
        view::ColumnFormatChoice::Bit => 5,
        view::ColumnFormatChoice::Word => 6,
    }
}

fn column_format_choice(index: usize) -> view::ColumnFormatChoice {
    match index {
        1 => view::ColumnFormatChoice::Locale,
        2 => view::ColumnFormatChoice::Uppercase,
        3 => view::ColumnFormatChoice::Lowercase,
        4 => view::ColumnFormatChoice::Char,
        5 => view::ColumnFormatChoice::Bit,
        6 => view::ColumnFormatChoice::Word,
        _ => view::ColumnFormatChoice::Plain,
    }
}

impl FilterPrompt {
    fn new(view: &view::TableView, mode: FilterMode, column: usize) -> Self {
        let enabled_kinds = FilterKind::all()
            .into_iter()
            .filter(|kind| view.filter_kind_enabled(column, *kind))
            .collect::<Vec<_>>();
        let selected_kind = view.default_filter_kind(column);
        Self {
            mode,
            column,
            selected_kind,
            enabled_kinds,
            input: String::new(),
            error: None,
        }
    }

    fn cycle_kind(&mut self) {
        if self.enabled_kinds.is_empty() {
            return;
        }
        let current = self
            .enabled_kinds
            .iter()
            .position(|kind| *kind == self.selected_kind)
            .unwrap_or(0);
        self.selected_kind = self.enabled_kinds[(current + 1) % self.enabled_kinds.len()];
        self.error = None;
    }
}

pub(crate) struct FilterPromptView<'a> {
    pub mode: FilterMode,
    pub column: usize,
    pub selected_kind: FilterKind,
    pub enabled_kinds: &'a [FilterKind],
    pub input: &'a str,
    pub error: Option<&'a str>,
}

impl<'a> From<&'a FilterPrompt> for FilterPromptView<'a> {
    fn from(prompt: &'a FilterPrompt) -> Self {
        Self {
            mode: prompt.mode,
            column: prompt.column,
            selected_kind: prompt.selected_kind,
            enabled_kinds: &prompt.enabled_kinds,
            input: &prompt.input,
            error: prompt.error.as_deref(),
        }
    }
}

#[cfg(feature = "saved-views")]
fn apply_saved_view(
    config: &cli::Config,
    view: &mut view::TableView,
) -> anyhow::Result<Option<SavedViewRuntime>> {
    use crate::cli::SavedViewSelection as CliSavedViewSelection;
    use crate::ops::sort::{SortDirection, SortMode};
    use crate::saved_views::{self, FilterAction, SavedViewSelection, SortKind};

    let target_identity = PathBuf::from(config.target.saved_view_filename());
    let selection = match &config.saved_view {
        CliSavedViewSelection::Disabled => return Ok(None),
        CliSavedViewSelection::Auto => SavedViewSelection::Auto {
            input_path: &target_identity,
        },
        CliSavedViewSelection::Force(name) => SavedViewSelection::Force { name },
    };

    let target_path = placeholder_saved_view_path(&target_identity);
    let view_name = target_path
        .as_deref()
        .and_then(Path::file_stem)
        .and_then(|stem| stem.to_str())
        .unwrap_or("view")
        .to_owned();
    let discovered = saved_views::discover_saved_views(None);
    let mut messages = discovered
        .warnings
        .iter()
        .map(format_saved_view_warning)
        .collect::<Vec<_>>();
    let Some(selected) = saved_views::select_saved_view(&discovered.views, selection) else {
        if let CliSavedViewSelection::Force(name) = &config.saved_view {
            anyhow::bail!("saved view '{name}' was requested but was not found");
        }
        return Ok(Some(SavedViewRuntime {
            source_path: None,
            target_path,
            view_name,
            explicit_locale: None,
            messages,
        }));
    };
    messages.extend(selected.view.warnings.iter().map(format_saved_view_warning));
    messages.extend(selected.warnings.iter().map(format_saved_view_warning));
    let Some(header) = view.header() else {
        return Ok(Some(SavedViewRuntime {
            source_path: Some(selected.view.path.clone()),
            target_path: Some(selected.view.path.clone()),
            view_name: selected.view.canonical_name.clone(),
            explicit_locale: selected.view.view.view.locale.clone(),
            messages,
        }));
    };
    let header = header.to_vec();
    view.set_view_null_placement(selected.view.view.view.nulls);
    let structured_definition = view.table_definition().filter(|definition| {
        definition
            .columns
            .iter()
            .any(|column| column.source_identity.canonical_key().is_some())
    });
    let structured_schema_provisional = structured_definition.is_some_and(|definition| {
        definition.schema_state == crate::table::SchemaState::Provisional
    });
    let resolved = if let Some(definition) = structured_definition {
        saved_views::resolve_structured_columns(&selected.view.view, definition)
    } else {
        saved_views::resolve_columns(&selected.view.view, &header)
    };
    messages.extend(resolved.warnings.iter().map(format_saved_view_warning));
    view.apply_saved_columns(&resolved, selected.view.view.view.locale.as_deref());

    let sort_keys = selected
        .view
        .view
        .view
        .sort
        .iter()
        .filter(|_| config.sorted != Some(false))
        .filter_map(|sort| {
            let column = view
                .table_definition()
                .filter(|definition| {
                    definition
                        .columns
                        .iter()
                        .any(|column| column.source_identity.canonical_key().is_some())
                })
                .and_then(|definition| {
                    saved_views::resolve_structured_column_reference(definition, &sort.column)
                })
                .or_else(|| saved_views::resolve_column_reference(&header, &sort.column))?;
            let direction = match sort.direction {
                saved_views::SortDirection::Asc => SortDirection::Ascending,
                saved_views::SortDirection::Desc => SortDirection::Descending,
            };
            let mode = match sort.kind {
                SortKind::Lexical => SortMode::Lexical,
                SortKind::Natural => SortMode::Natural,
                SortKind::Numeric => SortMode::Numeric,
                SortKind::Type => view.type_sort_mode_for_source(column),
            };
            Some(view::ActiveSortKey {
                column,
                mode,
                direction,
                nulls: view.resolved_null_placement(column),
            })
        })
        .collect::<Vec<_>>();
    view.apply_saved_sort_keys(sort_keys);

    let mut unresolved_filters = Vec::new();
    for filter in &selected.view.view.view.filters {
        let column = view
            .table_definition()
            .filter(|definition| {
                definition
                    .columns
                    .iter()
                    .any(|column| column.source_identity.canonical_key().is_some())
            })
            .and_then(|definition| {
                saved_views::resolve_structured_column_reference(definition, &filter.column)
            })
            .or_else(|| saved_views::resolve_column_reference(&header, &filter.column));
        let Some(column) = column else {
            if structured_schema_provisional && filter.column.starts_with('/') {
                unresolved_filters.push(filter.clone());
            }
            continue;
        };
        let mode = match filter.action {
            FilterAction::In => FilterMode::In,
            FilterAction::Out => FilterMode::Out,
        };
        let kind = match filter.kind {
            saved_views::FilterKind::Text => FilterKind::Text,
            saved_views::FilterKind::Regex => FilterKind::Regex,
            saved_views::FilterKind::Numeric => FilterKind::Numeric,
        };
        let _ = view.apply_source_filter(column, mode, kind, filter.condition.clone());
    }
    if structured_schema_provisional {
        view.retain_pending_saved_operations(
            if config.sorted == Some(false) {
                Vec::new()
            } else {
                selected.view.view.view.sort.clone()
            },
            unresolved_filters,
        );
    }
    Ok(Some(SavedViewRuntime {
        source_path: Some(selected.view.path.clone()),
        target_path: Some(selected.view.path.clone()),
        view_name: selected.view.canonical_name.clone(),
        explicit_locale: selected.view.view.view.locale.clone(),
        messages,
    }))
}

#[cfg(feature = "saved-views")]
fn placeholder_saved_view_path(input: &Path) -> Option<PathBuf> {
    let view_dir = saved_views::saved_view_dir(None)?;
    let basename = input.file_name()?.to_str()?;
    let stem = if let Some((stem, _)) = basename.rsplit_once('.') {
        stem
    } else {
        basename
    };
    Some(view_dir.join(format!("{stem}.yml")))
}

#[cfg(feature = "saved-views")]
fn format_saved_view_warning(warning: &saved_views::SavedViewWarning) -> String {
    format!("saved view: {}: {}", warning.field, warning.message)
}

#[cfg(feature = "saved-views")]
fn write_saved_view_atomic(path: &Path, yaml: &str) -> anyhow::Result<()> {
    use std::fs;
    use std::io::Write;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let contents = if path.exists() {
        let existing = fs::read_to_string(path).unwrap_or_default();
        merge_saved_view_comments(&existing, yaml)
    } else {
        yaml.to_owned()
    };
    let tmp_path = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("yml")
    ));
    {
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(feature = "saved-views")]
fn merge_saved_view_comments(existing: &str, yaml: &str) -> String {
    let header = saved_view_header_comment_block(existing);
    let inline_comments = saved_view_inline_comments(existing);
    let mut merged = apply_saved_view_inline_comments(yaml, &inline_comments);
    if !header.is_empty() {
        merged = format!("{header}\n{merged}");
    }
    merged
}

#[cfg(feature = "saved-views")]
fn saved_view_header_comment_block(existing: &str) -> String {
    existing
        .lines()
        .take_while(|line| line.trim().is_empty() || line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(feature = "saved-views")]
fn saved_view_inline_comments(existing: &str) -> std::collections::BTreeMap<String, String> {
    let mut tracker = SavedViewYamlPathTracker::default();
    let mut comments = std::collections::BTreeMap::new();
    for line in existing.lines() {
        let Some((content, comment)) = split_yaml_inline_comment(line) else {
            let _ = tracker.path_for_line(line);
            continue;
        };
        if let Some(path) = tracker.path_for_content(content) {
            let comment = comment.trim().to_owned();
            if let Some(wildcard_path) = saved_view_sequence_wildcard_path(&path) {
                comments.insert(wildcard_path, comment.clone());
            }
            comments.insert(path, comment);
        }
    }
    comments
}

#[cfg(feature = "saved-views")]
fn apply_saved_view_inline_comments(
    yaml: &str,
    comments: &std::collections::BTreeMap<String, String>,
) -> String {
    let mut tracker = SavedViewYamlPathTracker::default();
    let mut output = String::new();
    for line in yaml.lines() {
        let mut line = line.to_owned();
        if let Some(path) = tracker.path_for_line(&line) {
            if let Some(comment) = comments.get(&path).or_else(|| {
                saved_view_sequence_wildcard_path(&path)
                    .as_ref()
                    .and_then(|path| comments.get(path))
            }) {
                line.push(' ');
                line.push_str(comment);
            }
        }
        output.push_str(&line);
        output.push('\n');
    }
    output
}

#[cfg(feature = "saved-views")]
fn saved_view_sequence_wildcard_path(path: &str) -> Option<String> {
    let (prefix, suffix) = path.rsplit_once('.')?;
    suffix
        .chars()
        .all(|ch| ch.is_ascii_digit())
        .then(|| format!("{prefix}.*"))
}

#[cfg(feature = "saved-views")]
fn split_yaml_inline_comment(line: &str) -> Option<(&str, &str)> {
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        match ch {
            '\\' if in_double && !escaped => {
                escaped = true;
                continue;
            }
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single && !escaped => in_double = !in_double,
            '#' if !in_single && !in_double => {
                let content = &line[..idx];
                if content.trim().is_empty() {
                    return None;
                }
                return Some((content.trim_end(), &line[idx..]));
            }
            _ => {}
        }
        escaped = false;
    }
    None
}

#[cfg(feature = "saved-views")]
#[derive(Default)]
struct SavedViewYamlPathTracker {
    stack: Vec<(usize, String)>,
    sequence_indices: std::collections::BTreeMap<String, usize>,
}

#[cfg(feature = "saved-views")]
impl SavedViewYamlPathTracker {
    fn path_for_line(&mut self, line: &str) -> Option<String> {
        let content = split_yaml_inline_comment(line)
            .map(|(content, _)| content)
            .unwrap_or(line);
        self.path_for_content(content)
    }

    fn path_for_content(&mut self, content: &str) -> Option<String> {
        let trimmed = content.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return None;
        }
        let indent = content.len().saturating_sub(trimmed.len());
        if let Some(rest) = trimmed.strip_prefix("- ") {
            self.stack
                .retain(|(stack_indent, _)| *stack_indent < indent);
            let parent = self.current_path();
            let index = self.sequence_indices.entry(parent).or_insert(0);
            let index_component = index.to_string();
            *index += 1;
            self.stack.push((indent, index_component));
            return self
                .path_for_mapping(rest)
                .or_else(|| Some(self.current_path()));
        }

        self.stack
            .retain(|(stack_indent, _)| *stack_indent < indent);
        self.path_for_mapping(trimmed)
    }

    fn path_for_mapping(&mut self, content: &str) -> Option<String> {
        let (key, value) = content.split_once(':')?;
        let key = saved_view_yaml_path_key(key);
        if key.is_empty() {
            return None;
        }
        let mut path = self.current_path();
        if !path.is_empty() {
            path.push('.');
        }
        path.push_str(&key);
        if value.trim().is_empty() {
            let indent = self
                .stack
                .last()
                .map(|(indent, _)| indent.saturating_add(2))
                .unwrap_or(0);
            self.stack.push((indent, key));
        }
        Some(path)
    }

    fn current_path(&self) -> String {
        self.stack
            .iter()
            .map(|(_, component)| component.as_str())
            .collect::<Vec<_>>()
            .join(".")
    }
}

#[cfg(feature = "saved-views")]
fn saved_view_yaml_path_key(key: &str) -> String {
    let key = key.trim();
    key.strip_prefix('"')
        .and_then(|key| key.strip_suffix('"'))
        .or_else(|| {
            key.strip_prefix('\'')
                .and_then(|key| key.strip_suffix('\''))
        })
        .unwrap_or(key)
        .to_owned()
}

fn closes_popup(event: KeyEvent) -> bool {
    matches!(event.code, KeyCode::Esc | KeyCode::Enter)
        || matches!(
            command::lookup_key_event(event),
            Some(command::Command::Quit | command::Command::Help)
        )
}

fn current_cell(view: &view::TableView) -> Option<String> {
    view.current_cell_rendered()
}

fn popup_area(area: ratatui::layout::Rect) -> ratatui::layout::Rect {
    let width = (area.width.saturating_mul(3) / 4).max(20).min(area.width);
    let height = (area.height.saturating_mul(3) / 4).max(5).min(area.height);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    ratatui::layout::Rect::new(x, y, width, height)
}

fn footer_status<'a>(
    source_status: Option<&'a str>,
    message: Option<&'a str>,
    count_status: &'a str,
) -> &'a str {
    source_status.or(message).unwrap_or(count_status)
}

fn source_extent_label(extent: Option<crate::table::ResultExtent>, pending: bool) -> String {
    match extent {
        Some(extent) => format!("{extent:?}"),
        None if pending => "pending".to_owned(),
        None => "unknown".to_owned(),
    }
}

fn table_area(area: ratatui::layout::Rect) -> ratatui::layout::Rect {
    ratatui::layout::Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1))
}

fn help_popup_area(area: ratatui::layout::Rect) -> ratatui::layout::Rect {
    if area.width <= 4 || area.height <= 4 {
        return area;
    }
    ratatui::layout::Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "sqlite")]
    use clap::Parser;
    use crossterm::event::KeyModifiers;
    use std::cell::Cell;
    #[cfg(feature = "elasticsearch")]
    use std::io::Read;
    use std::io::{Seek, Write};
    #[cfg(feature = "elasticsearch")]
    use std::net::TcpListener;
    #[cfg(feature = "elasticsearch")]
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    };

    #[cfg(feature = "elasticsearch")]
    struct MockElasticsearchResponse {
        status: u16,
        body: &'static str,
        delay: std::time::Duration,
    }

    #[cfg(feature = "elasticsearch")]
    impl MockElasticsearchResponse {
        fn ok(body: &'static str) -> Self {
            Self {
                status: 200,
                body,
                delay: std::time::Duration::ZERO,
            }
        }

        fn delayed(body: &'static str, delay: std::time::Duration) -> Self {
            Self {
                status: 200,
                body,
                delay,
            }
        }

        fn error(status: u16, body: &'static str) -> Self {
            Self {
                status,
                body,
                delay: std::time::Duration::ZERO,
            }
        }
    }

    #[cfg(feature = "elasticsearch")]
    struct MockElasticsearch {
        endpoint: url::Url,
        requests: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    #[cfg(feature = "elasticsearch")]
    impl MockElasticsearch {
        fn start(responses: Vec<MockElasticsearchResponse>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("mock Elasticsearch");
            listener.set_nonblocking(true).unwrap();
            let endpoint =
                url::Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let worker_requests = requests.clone();
            let stop = Arc::new(AtomicBool::new(false));
            let worker_stop = stop.clone();
            let worker = std::thread::spawn(move || {
                let mut responses = responses.into_iter();
                'server: while !worker_stop.load(Ordering::Acquire) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(2));
                            continue;
                        }
                        Err(error) => panic!("mock accept: {error}"),
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut chunk = [0_u8; 4096];
                    let header_end = loop {
                        let count = match stream.read(&mut chunk) {
                            Ok(count) => count,
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                                ) =>
                            {
                                continue 'server;
                            }
                            Err(error) => panic!("read mock request: {error}"),
                        };
                        if count == 0 {
                            continue 'server;
                        }
                        request.extend_from_slice(&chunk[..count]);
                        if let Some(index) =
                            request.windows(4).position(|window| window == b"\r\n\r\n")
                        {
                            break index + 4;
                        }
                    };
                    let header = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = header
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or_default();
                    while request.len().saturating_sub(header_end) < content_length {
                        let count = match stream.read(&mut chunk) {
                            Ok(0) => continue 'server,
                            Ok(count) => count,
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                                ) =>
                            {
                                continue 'server;
                            }
                            Err(error) => panic!("read mock body: {error}"),
                        };
                        request.extend_from_slice(&chunk[..count]);
                    }
                    worker_requests
                        .lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&request).into_owned());
                    let Some(response) = responses.next() else {
                        panic!("unexpected Elasticsearch request");
                    };
                    if !response.delay.is_zero() {
                        std::thread::sleep(response.delay);
                    }
                    let reason = if response.status < 400 {
                        "OK"
                    } else {
                        "Bad Request"
                    };
                    write!(
                        stream,
                        "HTTP/1.1 {} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        response.status,
                        reason,
                        response.body.len(),
                        response.body
                    )
                    .expect("mock response");
                    stream.flush().expect("flush mock response");
                }
            });
            Self {
                endpoint,
                requests,
                stop,
                worker: Some(worker),
            }
        }

        fn source(&self) -> ingest::source::InputSource {
            ingest::source::InputSource::Url(self.endpoint.clone())
        }

        fn request_count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }

    #[cfg(feature = "elasticsearch")]
    impl Drop for MockElasticsearch {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let result = worker.join();
                if !std::thread::panicking() {
                    result.expect("mock Elasticsearch worker panicked");
                }
            }
        }
    }

    fn rows(values: &[&[&str]]) -> Vec<Vec<String>> {
        values
            .iter()
            .map(|row| row.iter().map(|cell| (*cell).to_owned()).collect())
            .collect()
    }

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

    #[test]
    fn explicit_full_schema_scan_has_specific_opening_status() {
        let source = ingest::source::InputSource::Path("response.json".into());
        let options = ingest::OpenOptions {
            schema_scan: ingest::SchemaScan::Full,
            ..ingest::OpenOptions::default()
        };

        assert_eq!(
            full_schema_scan_status(&source, &options).as_deref(),
            Some("Scanning full schema for response.json")
        );
        assert!(full_schema_scan_status(&source, &ingest::OpenOptions::default()).is_none());

        let delimited = ingest::source::InputSource::Path("data.csv".into());
        assert!(full_schema_scan_status(&delimited, &options).is_none());

        let explicitly_delimited = ingest::OpenOptions {
            format: ingest::InputFormat::Delimited,
            ..options.clone()
        };
        assert!(full_schema_scan_status(&source, &explicitly_delimited).is_none());

        let selected_json = ingest::OpenOptions {
            json_path: Some("/rows".parse().unwrap()),
            ..options
        };
        let unknown = ingest::source::InputSource::Path("response.data".into());
        assert_eq!(
            full_schema_scan_status(&unknown, &selected_json).as_deref(),
            Some("Scanning full schema for response.data")
        );
    }

    fn app_with_rows(rows: Vec<Vec<String>>) -> App {
        App {
            source: ingest::source::InputSource::Stdin,
            open_options: ingest::OpenOptions::default(),
            view: view::TableView::classify(rows, view::Viewport::new(10, 4)),
            popup: None,
            filter_prompt: None,
            column_info: None,
            source_modal: None,
            search_query: String::new(),
            keys: command::KeyInterpreter::default(),
            message: None,
            diagnostics: Vec::new(),
            theme: theme::default_theme(),
            #[cfg(feature = "saved-views")]
            saved_view: None,
            #[cfg(feature = "saved-views")]
            view_modal: None,
        }
    }

    fn app_for_source(path: std::path::PathBuf, options: ingest::OpenOptions) -> App {
        let opened = ingest::open_source(ingest::source::InputSource::Path(path.clone()), &options)
            .expect("open")
            .into_implicit_table()
            .expect("table");
        let mut app = app_with_rows(rows(&[&["placeholder"], &["value"]]));
        app.source = ingest::source::InputSource::Path(path);
        app.open_options = options;
        app.view =
            view::TableView::from_opened_table(opened, view::Viewport::new(8, 80)).expect("view");
        app
    }

    #[cfg(feature = "elasticsearch")]
    fn app_for_elasticsearch(server: &MockElasticsearch, options: ingest::OpenOptions) -> App {
        let source = server.source();
        let opened = ingest::open_source(source.clone(), &options)
            .expect("open Elasticsearch")
            .into_implicit_table()
            .expect("Elasticsearch table");
        let mut app = app_with_rows(rows(&[&["placeholder"], &["value"]]));
        app.source = source;
        app.open_options = options;
        app.view =
            view::TableView::from_opened_table(opened, view::Viewport::new(8, 80)).expect("view");
        app
    }

    #[cfg(feature = "sqlite")]
    fn create_sqlite_database(path: &std::path::Path, statements: &[&str]) {
        let runtime = tokio::runtime::Runtime::new().expect("sqlite runtime");
        runtime.block_on(async {
            let database = turso::Builder::new_local(path.to_str().expect("utf8 path"))
                .build()
                .await
                .expect("sqlite database");
            let connection = database.connect().expect("sqlite connection");
            for statement in statements {
                connection
                    .execute(statement, ())
                    .await
                    .expect("sqlite statement");
            }
        });
    }

    #[cfg(feature = "sqlite")]
    fn sqlite_app(statements: &[&str], table: &str) -> (tempfile::TempDir, App) {
        let directory = tempfile::tempdir().expect("sqlite fixture");
        let path = directory.path().join("modal.db");
        create_sqlite_database(&path, statements);
        let options = ingest::OpenOptions {
            table: Some(table.to_owned()),
            ..ingest::OpenOptions::default()
        };
        (directory, app_for_source(path, options))
    }

    #[cfg(feature = "sqlite")]
    fn sqlite_artifact_snapshot(path: &std::path::Path) -> Vec<(String, Option<Vec<u8>>)> {
        ["", "-journal", "-wal", "-shm", "-tshm", "-log"]
            .into_iter()
            .map(|suffix| {
                let mut artifact = path.as_os_str().to_os_string();
                artifact.push(suffix);
                (
                    suffix.to_owned(),
                    std::fs::read(std::path::PathBuf::from(artifact)).ok(),
                )
            })
            .collect()
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

        let prepared = prepare_app(&config, theme_load, source, |_| Ok(()), Some(&mut selector))
            .expect("clean cancellation");

        assert!(prepared.is_none());
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
        assert!(app.view.request_source_query(query));
        assert!(app.view.source_query_is_pending());

        let mut output = Vec::new();
        restore_before_export(
            || Ok(()),
            || {
                app.view.await_latest_source_query()?;
                crate::output::write_view(
                    crate::output::OutputFormat::Table,
                    crate::output::ColorOutput::Never,
                    &mut app.view,
                    &app.theme,
                    &mut output,
                )
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

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[cfg(feature = "saved-views")]
    fn app_with_saved_view_target(rows: Vec<Vec<String>>, target_path: PathBuf) -> App {
        App {
            source: ingest::source::InputSource::Path(PathBuf::from("cat_shards.txt")),
            open_options: ingest::OpenOptions::default(),
            view: view::TableView::classify(rows, view::Viewport::new(10, 4)),
            popup: None,
            filter_prompt: None,
            column_info: None,
            source_modal: None,
            search_query: String::new(),
            keys: command::KeyInterpreter::default(),
            message: None,
            diagnostics: Vec::new(),
            theme: theme::default_theme(),
            saved_view: Some(SavedViewRuntime {
                source_path: None,
                target_path: Some(target_path),
                view_name: "cat_shards".to_owned(),
                explicit_locale: None,
                messages: Vec::new(),
            }),
            view_modal: None,
        }
    }

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
    fn query_popup_actions_document_their_keyboard_shortcuts() {
        assert_eq!(QUERY_POPUP_ACTIONS, ["Copy (y)", "Close (Enter/Esc)"]);
    }

    #[test]
    fn source_config_surfaces_unresolvable_columns_without_persisting_them() {
        let file = tempfile::NamedTempFile::new().expect("csv fixture");
        std::fs::write(file.path(), "name\nalpha\n").expect("csv");
        let mut app = app_for_source(file.path().to_path_buf(), ingest::OpenOptions::default());
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
            .source_modal
            .as_ref()
            .and_then(|modal| modal.error.as_deref())
            .is_some_and(|error| error.contains("unavailable column")));
        assert!(app.open_options.source_filters.is_empty());
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
        crate::output::write_view(
            crate::output::OutputFormat::Table,
            crate::output::ColorOutput::Never,
            &mut app.view,
            &app.theme,
            &mut output,
        )
        .unwrap();
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
            MockElasticsearchResponse::error(
                400,
                r#"{"error":{"reason":"invalid ES|QL test query"}}"#,
            ),
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
        let yaml = app.view.to_saved_view_yaml_with_source_options(
            "errors",
            &app.input_filename(),
            None,
            &app.open_options,
        );
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
        assert_eq!(app.open_options.limit, None);
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

        assert_eq!(app.open_options.limit, None);
        assert_eq!(app.open_options.source_filters.len(), 1);
    }

    #[test]
    fn reload_reapplies_active_filters_and_clamps_cursor() {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        writeln!(file, "Name").expect("write header");
        writeln!(file, "alpha").expect("write row");
        writeln!(file, "beta").expect("write row");

        let mut app = App {
            source: ingest::source::InputSource::Path(file.path().to_path_buf()),
            open_options: ingest::OpenOptions::default(),
            view: view::TableView::classify(
                rows(&[&["Name"], &["alpha"], &["beta"]]),
                view::Viewport::new(10, 4),
            ),
            popup: None,
            filter_prompt: None,
            column_info: None,
            source_modal: None,
            search_query: String::new(),
            keys: command::KeyInterpreter::default(),
            message: None,
            diagnostics: Vec::new(),
            theme: theme::default_theme(),
            #[cfg(feature = "saved-views")]
            saved_view: None,
            #[cfg(feature = "saved-views")]
            view_modal: None,
        };
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

    #[cfg(feature = "saved-views")]
    #[test]
    fn saved_view_modal_displays_placeholder_and_saves_immediately() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("views").join("cat_shards.yml");
        let mut app = app_with_saved_view_target(rows(&[&["Name"], &["alpha"]]), target.clone());

        app.open_saved_view_modal();
        let modal = app.view_modal.as_ref().expect("modal");
        assert!(modal.filename.contains("cat_shards.yml"));
        assert!(modal.yaml.contains("name: cat_shards"));
        assert!(modal.yaml.contains("filenames:\n  - cat_shards.txt"));

        app.handle_saved_view_key(key(KeyCode::Char('s')));

        let saved = std::fs::read_to_string(&target).expect("saved file");
        assert!(saved.contains("name: cat_shards"));
        assert!(app
            .message
            .as_deref()
            .is_some_and(|message| message.contains("saved view")));
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn saved_view_modal_confirms_and_declines_overwrite() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("cat_shards.yml");
        std::fs::write(
            &target,
            concat!(
                "# keep me\n",
                "# describe this view\n",
                "name: old # view name\n",
                "filenames: # filename patterns\n",
                "  - old # first filename\n",
            ),
        )
        .expect("write old");
        let mut app = app_with_saved_view_target(rows(&[&["Name"], &["alpha"]]), target.clone());
        app.open_saved_view_modal();

        app.handle_saved_view_key(key(KeyCode::Char('s')));
        assert!(app.view_modal.as_ref().expect("modal").confirming_overwrite);
        app.handle_saved_view_key(key(KeyCode::Char('n')));
        assert!(!app.view_modal.as_ref().expect("modal").confirming_overwrite);
        assert!(std::fs::read_to_string(&target)
            .expect("old")
            .contains("name: old"));

        app.handle_saved_view_key(key(KeyCode::Char('s')));
        app.handle_saved_view_key(key(KeyCode::Char('y')));
        let saved = std::fs::read_to_string(&target).expect("saved file");
        assert!(saved.starts_with("# keep me\n# describe this view\n"));
        assert!(saved.contains("name: cat_shards # view name"));
        assert!(saved.contains("filenames: # filename patterns"));
        assert!(
            saved.contains("  - cat_shards.txt # first filename"),
            "{saved}"
        );
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn saved_view_comment_merge_keeps_header_and_inline_comments() {
        let existing = "# header\nname: old # view name\nfilenames: # filename patterns\n  - old # first filename\n";
        let yaml = "name: cat_shards\nfilenames:\n  - cat_shards.txt\n";

        let merged = merge_saved_view_comments(existing, yaml);

        assert!(merged.starts_with("# header\n"));
        assert!(merged.contains("name: cat_shards # view name"));
        assert!(merged.contains("filenames: # filename patterns"));
        assert!(
            merged.contains("  - cat_shards.txt # first filename"),
            "{merged}"
        );
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn saved_view_modal_scrolls_and_reports_save_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().to_path_buf();
        let mut app = app_with_saved_view_target(
            rows(&[&["A", "B", "C"], &["1", "2", "3"], &["4", "5", "6"]]),
            target,
        );
        app.open_saved_view_modal();

        app.handle_saved_view_key(key(KeyCode::Char('j')));
        assert_eq!(app.view_modal.as_ref().expect("modal").scroll, 1);

        app.handle_saved_view_key(key(KeyCode::Char('s')));
        assert!(app.view_modal.as_ref().expect("modal").confirming_overwrite);
        app.handle_saved_view_key(key(KeyCode::Char('y')));

        assert_eq!(app.popup, Some(ui::Popup::SavedView));
        assert!(app
            .message
            .as_deref()
            .is_some_and(|message| message.contains("failed to save view")));
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn saved_view_binding_reports_disabled_when_no_view_context() {
        let mut app = app_with_rows(rows(&[&["Name"], &["alpha"]]));

        app.apply(command::KeyAction {
            command: command::Command::SavedView,
            count: None,
        })
        .expect("apply");

        assert_eq!(app.popup, None);
        assert_eq!(app.message.as_deref(), Some("saved views are disabled"));
    }
}
