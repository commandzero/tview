use super::filter_prompt::FilterPromptView;
use super::source_config::QUERY_POPUP_ACTIONS;
use super::{current_cell, App};
use crate::{command, ingest, theme, ui};
use crossterm::event::{poll, read, Event, KeyCode};
use std::time::Duration;

pub(super) fn select_table_modal(
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

pub(super) fn run_interactive(
    app: &mut App,
    terminal: &mut ui::terminal::TerminalSession,
) -> anyhow::Result<()> {
    loop {
        app.view.poll_source_query();
        #[cfg(feature = "saved-views")]
        app.collect_saved_binding_diagnostics();
        let source_status = app.view.take_source_status();
        terminal
            .terminal_mut()
            .draw(|frame| render_interaction(app, frame, source_status.as_deref()))?;
        let needs_poll = app.view.source_query_is_pending() || streaming_input_incomplete(app);
        if needs_poll && !poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(event) = read()? else {
            continue;
        };
        #[cfg(feature = "saved-views")]
        app.collect_saved_binding_diagnostics();
        if app.handle_key(event)? {
            #[cfg(feature = "saved-views")]
            app.collect_saved_binding_diagnostics();
            return Ok(());
        }
    }
}

fn streaming_input_incomplete(app: &App) -> bool {
    app.source.is_streaming()
        && !matches!(app.view.row_count_state(), crate::table::RowCount::Exact(_))
}

fn render_interaction(app: &mut App, frame: &mut ratatui::Frame<'_>, source_status: Option<&str>) {
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
            source_status,
            app.message.as_deref(),
            &count_status,
        )),
        area,
        frame.buffer_mut(),
        &app.theme,
    );
    render_popup(app, frame);
}

fn render_popup(app: &App, frame: &mut ratatui::Frame<'_>) {
    let area = frame.area();
    let popup = popup_area(area);
    let buffer = frame.buffer_mut();
    let theme = &app.theme;
    match app.popup {
        Some(ui::Popup::Help) => ui::render_help_popup_with_theme(
            &command::default_key_bindings(), help_popup_area(area), buffer, theme,
        ),
        Some(ui::Popup::Cell) => {
            if let Some(cell) = current_cell(&app.view) {
                ui::render_cell_popup_with_theme(&cell, "Cell", popup, buffer, theme);
            }
        }
        Some(ui::Popup::Info) => ui::render_info_popup_with_theme(&app.info_text(), popup, buffer, theme),
        Some(ui::Popup::Search) => {
            ui::render_search_prompt_with_theme(&app.search_query, popup, buffer, theme);
        }
        Some(ui::Popup::Filter) => {
            if let Some(prompt) = &app.filter_prompt {
                ui::render_filter_prompt_with_theme(&FilterPromptView::from(prompt), popup, buffer, theme);
            }
        }
        Some(ui::Popup::ColumnInfo) => {
            if let Some(modal) = &app.column_info {
                ui::render_column_info_popup_with_theme(&modal.popup(), popup, buffer, theme);
            }
        }
        Some(ui::Popup::SourceConfig) => ui::render_configuration_popup_with_theme(
            "Source Configuration", &app.source_modal_body(), &["Apply", "Cancel"], popup, buffer, theme,
        ),
        Some(ui::Popup::ViewConfig) => ui::render_configuration_popup_with_theme(
            "View Configuration",
            &format!(
                "{}\n\nx: clear view filters/sorts\nn: toggle null placement\nColumn Info (i) edits the current column.",
                app.view.view_transform_summary()
            ),
            &["Close"], popup, buffer, theme,
        ),
        Some(ui::Popup::Query) => ui::render_configuration_popup_with_theme(
            "Source Query", &app.query_modal_body(), QUERY_POPUP_ACTIONS, popup, buffer, theme,
        ),
        #[cfg(feature = "saved-views")]
        Some(ui::Popup::SavedView) => {
            if let Some(modal) = &app.view_modal {
                ui::render_saved_view_popup_with_theme(
                    &modal.filename, &modal.yaml, modal.scroll, modal.confirming_overwrite,
                    popup, buffer, theme,
                );
            }
        }
        None => {}
    }
}

fn popup_area(area: ratatui::layout::Rect) -> ratatui::layout::Rect {
    let width = (area.width.saturating_mul(3) / 4).max(20).min(area.width);
    let height = (area.height.saturating_mul(3) / 4).max(5).min(area.height);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    ratatui::layout::Rect::new(x, y, width, height)
}

pub(super) fn footer_status<'a>(
    source_status: Option<&'a str>,
    message: Option<&'a str>,
    count_status: &'a str,
) -> &'a str {
    source_status.or(message).unwrap_or(count_status)
}

pub(super) fn source_extent_label(
    extent: Option<crate::table::ResultExtent>,
    pending: bool,
) -> String {
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
