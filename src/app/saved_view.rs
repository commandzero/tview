use super::view_file::write_saved_view_atomic;
use super::{warning_footer, App};
use crate::saved_views;
use crate::{cli, ui, view};
use crossterm::event::{KeyCode, KeyEvent};
use std::path::PathBuf;

impl App {
    pub(super) fn collect_saved_binding_diagnostics(&mut self) {
        let new = self
            .view
            .take_saved_binding_warnings()
            .iter()
            .map(format_saved_view_warning)
            .collect::<Vec<_>>();
        if let Some(message) = warning_footer(&new) {
            self.message = Some(message);
        }
        self.diagnostics.extend(new);
    }

    pub(super) fn open_saved_view_modal(&mut self) {
        let Some(saved_view) = &self.saved_view else {
            self.message = Some("saved views are disabled".to_owned());
            return;
        };
        let input_filename = self.input_filename();
        let yaml = self.view.to_saved_view_yaml(
            &saved_view.view_name,
            &input_filename,
            saved_view.explicit_locale.as_deref(),
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

    pub(super) fn handle_saved_view_key(&mut self, event: KeyEvent) {
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

    fn save_view_modal(&mut self, confirmed_overwrite: bool) {
        let current_yaml = self.saved_view.as_ref().map(|saved_view| {
            self.view.to_saved_view_yaml(
                &saved_view.view_name,
                &self.input_filename(),
                saved_view.explicit_locale.as_deref(),
            )
        });
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
        modal.yaml = current_yaml.expect("saved view available");

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

    pub(super) fn input_filename(&self) -> String {
        self.source.elastic_context().map_or_else(
            || self.source.saved_view_filename(),
            |context| context.safe_identity(),
        )
    }
}

#[derive(Debug, Clone)]
pub(super) struct SavedViewRuntime {
    pub(super) source_path: Option<PathBuf>,
    pub(super) target_path: Option<PathBuf>,
    pub(super) view_name: String,
    pub(super) explicit_locale: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct ViewModal {
    pub(super) filename: String,
    pub(super) yaml: String,
    pub(super) scroll: usize,
    pub(super) confirming_overwrite: bool,
}

pub(super) fn apply_selected_saved_view(
    config: &cli::Config,
    invocation: &saved_views::SavedViewInvocation,
    view: &mut view::TableView,
) -> Option<SavedViewRuntime> {
    match invocation {
        saved_views::SavedViewInvocation::Disabled => None,
        saved_views::SavedViewInvocation::Enabled {
            selected: Some(selected),
            ..
        } => {
            let explicit_locale = selected.view.view.locale.clone();
            view.install_saved_binding(selected.view.view.clone(), config.sorted != Some(false));
            Some(SavedViewRuntime {
                source_path: Some(selected.path.clone()),
                target_path: Some(selected.path.clone()),
                view_name: selected.canonical_name.clone(),
                explicit_locale,
            })
        }
        saved_views::SavedViewInvocation::Enabled {
            selected: None,
            target_path,
            view_name,
            ..
        } => Some(SavedViewRuntime {
            source_path: None,
            target_path: target_path.clone(),
            view_name: view_name.clone(),
            explicit_locale: None,
        }),
    }
}

pub(super) fn format_saved_view_warning(warning: &saved_views::SavedViewWarning) -> String {
    format!("saved view: {}: {}", warning.field, warning.message)
}
