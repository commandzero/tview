use super::interactive::source_extent_label;
use super::App;
use crate::ui;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl App {
    pub(super) fn open_source_config_modal(&mut self) {
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

    pub(super) fn source_modal_body(&self) -> String {
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
        let editor = modal.editor_description(column);
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
        let mapping = self.mapping_description(column);
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

    fn mapping_description(&self, column: &str) -> String {
        let mapping_fields = self.view.source_field_catalog();
        if mapping_fields.is_empty() {
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
        }
    }

    pub(super) fn query_modal_body(&self) -> String {
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

    pub(super) fn handle_source_config_key(&mut self, event: KeyEvent) {
        let Some(mut modal) = self.source_modal.take() else {
            self.popup = None;
            return;
        };
        if modal.editing_query {
            modal.handle_query_editor(event);
        } else if modal.editing_filter {
            self.handle_filter_editor(&mut modal, event);
        } else if !self.handle_source_draft_key(&mut modal, event.code) {
            self.popup = None;
            return;
        }
        self.source_modal = Some(modal);
    }

    fn handle_filter_editor(&self, modal: &mut SourceConfigModal, event: KeyEvent) {
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
            KeyCode::Enter => self.stage_source_filter(modal),
            KeyCode::Char(ch) if accepts_editor_text(event.modifiers) => {
                modal.filter_input.push(ch);
                modal.error = None;
            }
            _ => {}
        }
    }

    fn stage_source_filter(&self, modal: &mut SourceConfigModal) {
        let operand = if modal.operator.requires_operand() {
            if modal.filter_input.is_empty() {
                modal.error = Some("filter value is required".to_owned());
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
            modal.error = Some(format!("source filter '{}' is unavailable", modal.operator));
            return;
        }
        let Some(column) = self.selected_source_column(modal) else {
            return;
        };
        modal.draft.filters.push(crate::table::SourceFilter {
            scope: crate::table::SourceFilterScope::Column(column),
            operator: modal.operator,
            operand,
        });
        modal.filter_input.clear();
        modal.editing_filter = false;
        modal.error = None;
    }

    fn handle_source_draft_key(&mut self, modal: &mut SourceConfigModal, key: KeyCode) -> bool {
        use crate::table::SortDirection;

        match key {
            KeyCode::Esc => return false,
            KeyCode::Enter => return self.apply_source_draft(modal),
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
            KeyCode::Char('q') => modal.open_query_editor(),
            KeyCode::Tab => modal.operator = next_source_operator(modal.operator),
            KeyCode::Char('s') => self.stage_source_sort(modal, SortDirection::Ascending),
            KeyCode::Char('S') => self.stage_source_sort(modal, SortDirection::Descending),
            KeyCode::Char('x') => {
                if let Some(column) = self.selected_source_column(modal) {
                    modal.clear_source_column(column);
                }
            }
            KeyCode::Char('+') => modal.adjust_limit(100),
            KeyCode::Char('-') => modal.adjust_limit(-100),
            _ => {}
        }
        true
    }

    fn apply_source_draft(&mut self, modal: &mut SourceConfigModal) -> bool {
        let unchanged = self.view.active_source_query() == Some(&modal.draft);
        if unchanged && !modal.native_query_changed {
            return false;
        }
        if self
            .view
            .request_source_query(modal.draft.clone(), modal.native_query_changed)
        {
            return false;
        }
        modal.error = self.view.take_source_status();
        true
    }

    fn selected_source_column(&self, modal: &SourceConfigModal) -> Option<crate::table::ColumnId> {
        let definition = self.view.table_definition()?;
        definition.columns.get(modal.column).map(|column| column.id)
    }

    fn stage_source_sort(
        &self,
        modal: &mut SourceConfigModal,
        direction: crate::table::SortDirection,
    ) {
        if !self.view.source_capabilities().sorting.is_supported() {
            modal.error = Some("source sorting is unavailable".to_owned());
            return;
        }
        if let Some(column) = self.selected_source_column(modal) {
            modal.draft.order_by.retain(|sort| sort.column != column);
            modal
                .draft
                .order_by
                .push(crate::table::SourceSort { column, direction });
        }
    }
}

pub(super) const QUERY_POPUP_ACTIONS: &[&str] = &["Copy (y)", "Close (Enter/Esc)"];

#[derive(Debug, Clone)]
pub(super) struct SourceConfigModal {
    pub(super) draft: crate::table::SourceQuery,
    pub(super) column: usize,
    pub(super) operator: crate::table::SourceFilterOperator,
    pub(super) filter_input: String,
    pub(super) editing_filter: bool,
    pub(super) query_input: String,
    pub(super) editing_query: bool,
    pub(super) native_query_changed: bool,
    pub(super) error: Option<String>,
}

impl SourceConfigModal {
    fn editor_description(&self, column: &str) -> String {
        if self.editing_query {
            format!(
                "\n\nNative query editor:\n{}\nEnter: stage query  Esc: cancel editor",
                self.query_input
            )
        } else if self.editing_filter {
            format!(
                "\n\nFilter editor: {} {} {}\nTab: operator  Enter: add  Esc: cancel editor",
                column, self.operator, self.filter_input
            )
        } else {
            "\n\n←/→: column  f: add filter  q: edit native query  Tab: operator\ns/S: source sort asc/desc  x: clear column\n+/-: adjust limit  Enter: apply  Esc: cancel"
                .to_owned()
        }
    }

    fn handle_query_editor(&mut self, event: KeyEvent) {
        match event.code {
            KeyCode::Esc => {
                self.editing_query = false;
                self.query_input = self.draft.native_query.clone().unwrap_or_default();
                self.error = None;
            }
            KeyCode::Backspace => {
                self.query_input.pop();
                self.error = None;
            }
            KeyCode::Enter => {
                let query = self.query_input.trim();
                if query.is_empty() {
                    self.error = Some("native query cannot be empty".to_owned());
                    return;
                }
                self.draft.native_query = Some(query.to_owned());
                self.native_query_changed = true;
                self.editing_query = false;
                self.error = None;
            }
            KeyCode::Char(ch) if accepts_editor_text(event.modifiers) => {
                self.query_input.push(ch);
                self.error = None;
            }
            _ => {}
        }
    }

    fn open_query_editor(&mut self) {
        let Some(query) = &self.draft.native_query else {
            self.error = Some("this source does not expose an editable native query".to_owned());
            return;
        };
        self.query_input.clone_from(query);
        self.editing_query = true;
        self.error = None;
    }

    fn clear_source_column(&mut self, column: crate::table::ColumnId) {
        self.draft.filters.retain(|filter| {
            !matches!(filter.scope, crate::table::SourceFilterScope::Column(id) if id == column)
        });
        self.draft.order_by.retain(|sort| sort.column != column);
    }

    fn adjust_limit(&mut self, delta: isize) {
        use std::num::NonZeroUsize;

        let next = if self.draft.limit == NonZeroUsize::MAX {
            1_000
        } else {
            self.draft.limit.get().saturating_add_signed(delta).max(1)
        };
        self.draft.limit = NonZeroUsize::new(next).expect("positive source limit");
    }
}

fn accepts_editor_text(modifiers: KeyModifiers) -> bool {
    modifiers.is_empty() || modifiers == KeyModifiers::SHIFT
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

pub(super) fn parse_source_operand(value: &str) -> crate::table::SourceOperand {
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
