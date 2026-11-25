use super::App;
use crate::ops::filter::{FilterKind, FilterMode};
use crate::{ui, view};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl App {
    pub(super) fn open_filter_prompt(&mut self, mode: FilterMode) {
        let column = self.view.cursor().column;
        self.filter_prompt = Some(FilterPrompt::new(&self.view, mode, column));
        self.popup = Some(ui::Popup::Filter);
    }

    pub(super) fn handle_filter_key(&mut self, event: KeyEvent) {
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FilterPrompt {
    pub(super) mode: FilterMode,
    pub(super) column: usize,
    pub(super) selected_kind: FilterKind,
    pub(super) enabled_kinds: Vec<FilterKind>,
    pub(super) input: String,
    pub(super) error: Option<String>,
}

impl FilterPrompt {
    pub(super) fn new(view: &view::TableView, mode: FilterMode, column: usize) -> Self {
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
