use super::App;
use crate::{ui, view};
use crossterm::event::{KeyCode, KeyEvent};

impl App {
    pub(super) fn open_column_info_modal(&mut self) {
        if let Some(info) = self.view.current_column_info() {
            self.column_info = Some(ColumnInfoModal::from_info(info));
            self.popup = Some(ui::Popup::ColumnInfo);
        }
    }

    pub(super) fn handle_column_info_key(&mut self, event: KeyEvent) {
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

    pub(super) fn label(self) -> &'static str {
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
pub(super) struct ColumnInfoModal {
    pub(super) column_name: String,
    pub(super) visible_column: usize,
    pub(super) source_column: usize,
    pub(super) filter_details: Vec<String>,
    pub(super) filter_indicator: Option<&'static str>,
    pub(super) active_group: usize,
    pub(super) visibility: usize,
    pub(super) alignment: usize,
    pub(super) column_type: usize,
    pub(super) format: usize,
    pub(super) sort: usize,
    pub(super) nulls: view::ColumnNullPlacementChoice,
    pub(super) canonical_source: Option<String>,
    pub(super) source_type: Option<String>,
    pub(super) source_declared_type: Option<String>,
    pub(super) source_operations: Vec<String>,
    pub(super) filters: usize,
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
            column_type: option_index(&COLUMN_TYPES, info.column_type),
            format: option_index(&COLUMN_FORMATS, info.format),
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

    pub(super) fn popup(&self) -> ui::ColumnInfoPopup {
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
        self.move_option(|index, count| (index + 1) % count);
    }

    fn previous_option(&mut self) {
        self.move_option(|index, count| index.checked_sub(1).unwrap_or(count - 1));
    }

    fn move_option(&mut self, advance: impl Fn(usize, usize) -> usize) {
        let group = self.active_group();
        let options = self.option_count(group);
        let mut index = self.selected_index(group);
        for _ in 0..options {
            index = advance(index, options);
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
            column_type: COLUMN_TYPES
                .get(self.column_type)
                .copied()
                .unwrap_or(COLUMN_TYPES[0]),
            format: COLUMN_FORMATS
                .get(self.format)
                .copied()
                .unwrap_or(COLUMN_FORMATS[0]),
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

const COLUMN_TYPES: [view::ColumnTypeChoice; 7] = {
    use view::ColumnTypeChoice::*;
    [Text, Date, Ip, Float, Integer, SemVer, Boolean]
};

const COLUMN_FORMATS: [view::ColumnFormatChoice; 7] = {
    use view::ColumnFormatChoice::*;
    [Plain, Locale, Uppercase, Lowercase, Char, Bit, Word]
};

fn option_index<T: PartialEq>(options: &[T], choice: T) -> usize {
    options
        .iter()
        .position(|option| *option == choice)
        .expect("supported column option")
}
