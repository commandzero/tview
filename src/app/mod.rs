mod column_info;
#[cfg(feature = "elasticsearch")]
mod context_startup;
mod filter_prompt;
mod interactive;
mod run;
#[cfg(feature = "saved-views")]
mod saved_view;
mod source_config;
mod startup;
#[cfg(feature = "saved-views")]
mod view_file;

use crate::{command, ingest, ops, theme, ui, view};
use column_info::ColumnInfoModal;
use command::Command;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use filter_prompt::FilterPrompt;
pub(crate) use filter_prompt::FilterPromptView;
use ops::filter::FilterMode;
use ops::search::SearchDirection;
use ops::skip::{
    Axis,
    Direction::{Backward, Forward},
};
use ops::sort::{
    SortDirection::{Ascending, Descending},
    SortMode::{Lexical, Natural, Numeric},
};
pub(crate) use run::{run_batch, run_terminal};
#[cfg(feature = "saved-views")]
use saved_view::{SavedViewRuntime, ViewModal};
use source_config::SourceConfigModal;

fn warning_footer(messages: &[String]) -> Option<String> {
    let first = messages.first()?;
    Some(if messages.len() == 1 {
        first.clone()
    } else {
        format!("{first} (+{} more warnings)", messages.len() - 1)
    })
}

struct App {
    source: ingest::source::InputSource,
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
impl App {
    fn handle_key(&mut self, event: KeyEvent) -> anyhow::Result<bool> {
        if let Some(popup) = self.popup {
            self.handle_popup_key(popup, event);
            return Ok(false);
        }

        let Some(action) = self.key_action(event) else {
            return Ok(false);
        };

        self.apply(action)?;
        Ok(action.command == command::Command::Quit)
    }

    fn handle_popup_key(&mut self, popup: ui::Popup, event: KeyEvent) {
        match popup {
            ui::Popup::Search => self.handle_search_key(event),
            ui::Popup::Filter => self.handle_filter_key(event),
            ui::Popup::ColumnInfo => self.handle_column_info_key(event),
            ui::Popup::SourceConfig => self.handle_source_config_key(event),
            ui::Popup::ViewConfig => self.handle_view_config_key(event),
            ui::Popup::Query => self.handle_query_key(event),
            #[cfg(feature = "saved-views")]
            ui::Popup::SavedView => self.handle_saved_view_key(event),
            _ if closes_popup(event) => self.popup = None,
            _ => {}
        }
    }

    fn handle_view_config_key(&mut self, event: KeyEvent) {
        match event.code {
            KeyCode::Esc | KeyCode::Enter => self.popup = None,
            KeyCode::Char('x') => self.view.clear_view_operations(),
            KeyCode::Char('n') => self.view.toggle_view_null_placement(),
            KeyCode::Char('i') => self.open_column_info_modal(),
            _ => {}
        }
    }

    fn handle_query_key(&mut self, event: KeyEvent) {
        match event.code {
            KeyCode::Esc | KeyCode::Enter => self.popup = None,
            KeyCode::Char('y') => {
                let sql = self
                    .view
                    .source_query_provenance()
                    .map(|provenance| provenance.copyable.as_str());
                let _ = ops::clipboard::yank_text(sql);
            }
            _ => {}
        }
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
        let count = action.count.unwrap_or(1);
        let steps = count as isize;
        match action.command {
            Command::Quit | Command::Redraw => {}
            Command::Reload => self.reload()?,
            Command::MoveUp => self.view.move_by(-steps, 0),
            Command::MoveDown => self.view.move_by(steps, 0),
            Command::MoveLeft => self.view.move_by(0, -steps),
            Command::MoveRight => self.view.move_by(0, steps),
            Command::Help => self.popup = Some(ui::Popup::Help),
            Command::PageUp => self.view.page_by(-1, 0, count),
            Command::PageDown => self.view.page_by(1, 0, count),
            Command::PageLeft => self.view.page_by(0, -1, count),
            Command::PageRight => self.view.page_by(0, 1, count),
            Command::LineHome => self.view.goto(self.view.cursor().row, 0),
            Command::LineEnd => self.goto_line_end(),
            Command::GotoTop => self.view.goto_top(),
            Command::GotoRow => self.goto_row(action.count),
            Command::GotoColumn => self.goto_column(action.count),
            Command::Mark => self.view.set_mark(),
            Command::GotoMark => self.view.goto_mark(),
            Command::ShowCell => self.show_current_cell(),
            Command::Search => self.open_search(),
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
            Command::GapDown => self.view.adjust_column_gap(-steps),
            Command::GapUp => self.view.adjust_column_gap(steps),
            Command::AllColumnsNarrower => self.view.adjust_all_column_widths(-steps),
            Command::AllColumnsWider => self.view.adjust_all_column_widths(steps),
            Command::CurrentColumnNarrower => self.view.adjust_current_column_width(-steps),
            Command::CurrentColumnWider => self.view.adjust_current_column_width(steps),
            Command::SortNaturalAsc => self.view.sort_current_column(Natural, Ascending),
            Command::SortNaturalDesc => self.view.sort_current_column(Natural, Descending),
            Command::SortNumericAsc => self.view.sort_current_column(Numeric, Ascending),
            Command::SortNumericDesc => self.view.sort_current_column(Numeric, Descending),
            Command::SortLexicalAsc => self.view.sort_current_column(Lexical, Ascending),
            Command::SortLexicalDesc => self.view.sort_current_column(Lexical, Descending),
            Command::YankCell => self.yank_rendered_cell(),
            Command::YankRawCell => self.yank_raw_cell(),
            Command::ToggleColumnWidthMode => self.set_width_mode(action.count),
            Command::SetCurrentColumnWidth => self.set_column_width(action.count),
            Command::ColumnHideLeft => self.view.hide_columns_left(count),
            Command::ColumnHideRight => self.view.hide_columns_right(count),
            Command::ColumnHideCurrent => self.view.hide_current_column(),
            Command::ColumnShowLeft => self.view.show_hidden_left(count),
            Command::ColumnShowRight => self.view.show_hidden_right(count),
            Command::ColumnSortAsc => self.sort_current_column(Ascending),
            Command::ColumnSortDesc => self.sort_current_column(Descending),
            Command::ColumnSortClear => self.view.clear_current_column_sort(),
            Command::SkipRowChangeForward => self.skip_change(Axis::Row, Forward, count),
            Command::SkipRowChangeBackward => self.skip_change(Axis::Row, Backward, count),
            Command::SkipColumnChangeForward => self.skip_change(Axis::Column, Forward, count),
            Command::SkipColumnChangeBackward => self.skip_change(Axis::Column, Backward, count),
            Command::ShowInfo => self.popup = Some(ui::Popup::Info),
        }
        Ok(())
    }

    fn goto_line_end(&mut self) {
        self.view.goto(
            self.view.cursor().row,
            self.view.column_count().saturating_sub(1),
        );
    }

    fn goto_row(&mut self, count: Option<usize>) {
        if let Some(row) = count {
            self.view.goto_user_row(row);
        } else {
            self.view.goto_bottom();
        }
    }

    fn goto_column(&mut self, count: Option<usize>) {
        if let Some(column) = count {
            self.view.goto_user_column(column);
        } else {
            self.view.goto(self.view.cursor().row, 0);
        }
    }

    fn show_current_cell(&mut self) {
        if current_cell(&self.view).is_some_and(|cell| !cell.is_empty()) {
            self.popup = Some(ui::Popup::Cell);
        }
    }

    fn open_search(&mut self) {
        self.search_query.clear();
        self.popup = Some(ui::Popup::Search);
    }

    fn yank_rendered_cell(&mut self) {
        let rendered = self.view.current_cell_rendered();
        let _ = ops::clipboard::yank_text(rendered.as_deref());
    }

    fn yank_raw_cell(&mut self) {
        let _ = ops::clipboard::yank_text(self.view.current_raw_cell());
    }

    fn set_width_mode(&mut self, count: Option<usize>) {
        if let Some(width) = count {
            self.view.set_all_column_widths(width);
        } else {
            self.view.toggle_variable_column_width_mode();
        }
    }

    fn set_column_width(&mut self, count: Option<usize>) {
        if let Some(width) = count {
            self.view.set_current_column_width(width);
        } else {
            self.view.maximize_current_column_width();
        }
    }

    fn sort_current_column(&mut self, direction: ops::sort::SortDirection) {
        let mode = if self.view.is_numeric_column(self.view.cursor().column) {
            ops::sort::SortMode::Numeric
        } else {
            ops::sort::SortMode::Lexical
        };
        self.view.sort_current_column(mode, direction);
    }

    fn skip_change(
        &mut self,
        axis: ops::skip::Axis,
        direction: ops::skip::Direction,
        count: usize,
    ) {
        let position = self.view.progressive_skip_to_change(axis, direction, count);
        self.view.goto(position.row, position.column);
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

    fn reload(&mut self) -> anyhow::Result<()> {
        self.view.reload_committed_source(&self.source)
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

#[cfg(test)]
mod tests;
