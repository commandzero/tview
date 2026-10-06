pub mod terminal;

use std::sync::OnceLock;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::command::KeyBinding;
use crate::ingest::{RelationAvailability, RelationCatalogEntry};
use crate::ops::filter::{FilterKind, FilterMode};
use crate::ops::search::CaseInsensitiveQuery;
use crate::theme::ResolvedTheme;
#[cfg(not(test))]
use crate::theme::{default_theme_for_terminal, terminal_color_mode_from_env};
use crate::view::{ColumnAlignment, TableView};
use crate::FilterPromptView;

#[cfg(test)]
use crate::theme::default_theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Popup {
    Cell,
    Info,
    Help,
    Search,
    Filter,
    ColumnInfo,
    SourceConfig,
    ViewConfig,
    Query,
    #[cfg(feature = "saved-views")]
    SavedView,
}

pub fn render_table(view: &mut TableView, area: Rect, buffer: &mut Buffer) {
    let theme = default_ui_theme();
    render_table_with_theme(view, area, buffer, theme, None);
}

fn default_ui_theme() -> &'static ResolvedTheme {
    static DEFAULT_THEME: OnceLock<ResolvedTheme> = OnceLock::new();
    DEFAULT_THEME.get_or_init(default_ui_theme_value)
}

#[cfg(test)]
fn default_ui_theme_value() -> ResolvedTheme {
    default_theme()
}

#[cfg(not(test))]
fn default_ui_theme_value() -> ResolvedTheme {
    default_theme_for_terminal(terminal_color_mode_from_env())
}

pub fn render_table_with_theme(
    view: &mut TableView,
    area: Rect,
    buffer: &mut Buffer,
    theme: &ResolvedTheme,
    search_query: Option<&str>,
) {
    let search_query = search_query.and_then(CaseInsensitiveQuery::new);
    view.set_terminal_width(usize::from(area.width));
    let viewport_height = visible_row_capacity(view, area);
    let viewport_width = visible_column_capacity(view, area);
    view.resize_viewport(viewport_height, viewport_width);
    view.prepare_conditional_colors(theme);

    let cursor = view.cursor();
    let viewport = view.viewport();
    let row_progress = match view.row_count_state() {
        crate::table::RowCount::Exact(_) => String::new(),
        crate::table::RowCount::AtLeast(count) => format!(" {count}+ rows"),
        crate::table::RowCount::Unknown => " ? rows".to_owned(),
    };
    let location = format!(
        " ({},{}){} ",
        cursor.row + 1,
        cursor.column + 1,
        row_progress
    );
    buffer.set_string(area.x, area.y, &location, theme.style("table.location"));

    if let Some(cell) = view.current_cell() {
        buffer.set_string(
            area.x + location.len() as u16 + 1,
            area.y,
            cell,
            theme.style("table.current_cell"),
        );
    }

    if area.height <= 1 {
        return;
    }

    for x in area.x..area.x + area.width {
        buffer[(x, area.y + 1)].set_symbol("─");
        buffer[(x, area.y + 1)].set_style(theme.style("table.divider"));
    }

    let mut row_y = area.y + 2;
    let widths = view.effective_column_widths_cached();
    let source_columns = view.visible_source_columns_vec();
    let alignments = column_alignments(view);
    let left_alignments = vec![Alignment::Left; widths.len()];

    if view.header_visible() {
        if let Some(header) = view.rendered_header() {
            render_row(
                buffer,
                &header,
                RowRender {
                    area,
                    y: row_y,
                    widths: &widths,
                    style: theme.style("table.header"),
                    selected_style: theme.style_or("table.header_selected", "table.header"),
                    selected_column: Some(cursor.column),
                    column_offset: viewport.origin.column,
                    visible_column_count: viewport.width,
                    column_gap: view.column_gap(),
                    alignments: &left_alignments,
                    hidden_boundaries: Some(&hidden_boundaries(view)),
                    marker_style: theme.style("table.hidden_marker"),
                    prefix_style: Some(theme.style_or("table.header_glyph", "table.divider")),
                    cell_styles: None,
                    preserve_selected_fg: None,
                    search_matches: None,
                    search_query: None,
                    search_style: Style::default(),
                },
            );
            row_y += 1;
        }
    }

    let row_end = viewport
        .origin
        .row
        .saturating_add(viewport.height)
        .min(view.row_count());
    let mut cell_styles = Vec::with_capacity(viewport.width);
    let mut preserve_selected_fg = Vec::with_capacity(viewport.width);
    let mut search_matches = Vec::with_capacity(viewport.width);
    for idx in viewport.origin.row..row_end {
        if row_y >= area.y + area.height {
            break;
        }
        let Some(row) = view.rendered_visible_row(idx) else {
            break;
        };
        cell_styles.clear();
        preserve_selected_fg.clear();
        search_matches.clear();
        for (column, cell) in row
            .iter()
            .enumerate()
            .skip(viewport.origin.column)
            .take(viewport.width)
        {
            let context = source_columns.get(column).copied().map(|source_column| {
                (
                    source_column,
                    view.source_cell_style_context(idx, source_column, cell, search_query.as_ref()),
                )
            });
            let search_match = context
                .as_ref()
                .is_some_and(|(_, context)| context.search_match);
            let mut style = theme.style("table.cell");
            let mut should_preserve_fg = false;
            if let Some(foreground) = context
                .as_ref()
                .and_then(|(_, context)| context.conditional_color)
            {
                style = style.fg(foreground);
                should_preserve_fg = true;
            }
            cell_styles.push(style);
            preserve_selected_fg.push(should_preserve_fg);
            search_matches.push(search_match);
        }
        let selected_column = (idx == cursor.row).then_some(cursor.column);
        render_row(
            buffer,
            &row,
            RowRender {
                area,
                y: row_y,
                widths: &widths,
                style: theme.style("table.cell"),
                selected_style: theme.style("table.selected"),
                selected_column,
                column_offset: viewport.origin.column,
                visible_column_count: viewport.width,
                column_gap: view.column_gap(),
                alignments: &alignments,
                hidden_boundaries: None,
                marker_style: theme.style("table.hidden_marker"),
                prefix_style: None,
                cell_styles: Some(&cell_styles),
                preserve_selected_fg: Some(&preserve_selected_fg),
                search_matches: Some(&search_matches),
                search_query: search_query.as_ref(),
                search_style: theme.style("search.highlight"),
            },
        );
        row_y += 1;
    }
}

pub fn render_footer(message: Option<&str>, area: Rect, buffer: &mut Buffer) {
    render_footer_with_theme(message, area, buffer, default_ui_theme());
}

pub fn render_footer_with_theme(
    message: Option<&str>,
    area: Rect,
    buffer: &mut Buffer,
    theme: &ResolvedTheme,
) {
    if area.height == 0 {
        return;
    }
    let y = area.y + area.height - 1;
    for x in area.x..area.x + area.width {
        buffer[(x, y)].set_symbol(" ");
    }
    if let Some(message) = message {
        let text = truncate_cell(message, area.width as usize, "…");
        buffer.set_stringn(
            area.x,
            y,
            &text,
            area.width as usize,
            theme.style("message.footer"),
        );
    }
}

pub fn render_popup(title: &str, body: &str, area: Rect, buffer: &mut Buffer) {
    render_popup_with_actions(title, body, &[], area, buffer, default_ui_theme());
}

fn render_popup_with_actions(
    title: &str,
    body: &str,
    actions: &[&str],
    area: Rect,
    buffer: &mut Buffer,
    theme: &ResolvedTheme,
) {
    if area.width < 2 || area.height < 2 {
        return;
    }
    let popup_style = theme.style("popup.background");
    let border_style = theme.style("popup.border");
    let x2 = area.x + area.width - 1;
    let y2 = area.y + area.height - 1;

    for y in area.y..=y2 {
        for x in area.x..=x2 {
            let cell = &mut buffer[(x, y)];
            cell.reset();
            cell.set_symbol(" ");
            cell.set_style(popup_style);
        }
    }

    for x in area.x..=x2 {
        buffer[(x, area.y)].set_symbol("─");
        buffer[(x, area.y)].set_style(border_style);
        buffer[(x, y2)].set_symbol("─");
        buffer[(x, y2)].set_style(border_style);
    }
    for y in area.y..=y2 {
        buffer[(area.x, y)].set_symbol("│");
        buffer[(area.x, y)].set_style(border_style);
        buffer[(x2, y)].set_symbol("│");
        buffer[(x2, y)].set_style(border_style);
    }
    buffer[(area.x, area.y)].set_symbol("┌");
    buffer[(x2, area.y)].set_symbol("┐");
    buffer[(area.x, y2)].set_symbol("└");
    buffer[(x2, y2)].set_symbol("┘");
    buffer[(area.x, area.y)].set_style(border_style);
    buffer[(x2, area.y)].set_style(border_style);
    buffer[(area.x, y2)].set_style(border_style);
    buffer[(x2, y2)].set_style(border_style);

    let title_width = area.width.saturating_sub(4) as usize;
    if title_width > 0 {
        let title = truncate_cell(title, title_width, "…").trim_end().to_owned();
        let title = format!("─ {title} ");
        buffer.set_stringn(
            area.x + 1,
            area.y,
            &title,
            area.width.saturating_sub(2) as usize,
            theme.style("popup.title"),
        );
    }
    if !actions.is_empty() {
        let footer = actions
            .iter()
            .map(|action| format!("[ {action} ]"))
            .collect::<Vec<_>>()
            .join(" ");
        let width = UnicodeWidthStr::width(footer.as_str()) as u16;
        let available = area.width.saturating_sub(3) as usize;
        if available > 0 {
            let text = truncate_cell(&footer, available, "…").trim_end().to_owned();
            let width = width.min(available as u16);
            let x = x2.saturating_sub(width).saturating_sub(1);
            buffer.set_stringn(x, y2, &text, available, theme.style("popup.action"));
        }
    }
    for (offset, line) in body
        .lines()
        .take(area.height.saturating_sub(4) as usize)
        .enumerate()
    {
        let content_width = area.width.saturating_sub(4) as usize;
        if content_width == 0 {
            break;
        }
        buffer.set_stringn(
            area.x + 2,
            area.y + 2 + offset as u16,
            truncate_cell(line, content_width, "…"),
            content_width,
            theme.style("popup.body"),
        );
    }
}

struct RowRender<'a> {
    area: Rect,
    y: u16,
    widths: &'a [usize],
    style: Style,
    selected_style: Style,
    selected_column: Option<usize>,
    column_offset: usize,
    visible_column_count: usize,
    column_gap: usize,
    alignments: &'a [Alignment],
    hidden_boundaries: Option<&'a [bool]>,
    marker_style: Style,
    prefix_style: Option<Style>,
    cell_styles: Option<&'a [Style]>,
    preserve_selected_fg: Option<&'a [bool]>,
    search_matches: Option<&'a [bool]>,
    search_query: Option<&'a CaseInsensitiveQuery<'a>>,
    search_style: Style,
}

fn render_row(buffer: &mut Buffer, row: &[String], render: RowRender<'_>) {
    let mut x = render.area.x;
    let area_end = render.area.x.saturating_add(render.area.width);
    for (column, cell) in row
        .iter()
        .enumerate()
        .skip(render.column_offset)
        .take(render.visible_column_count)
    {
        if x >= area_end {
            break;
        }
        let remaining_width = area_end.saturating_sub(x) as usize;
        if remaining_width == 0 {
            break;
        }
        let width = render
            .widths
            .get(column)
            .copied()
            .unwrap_or(1)
            .min(remaining_width);
        let base_style = render
            .cell_styles
            .and_then(|styles| styles.get(column - render.column_offset))
            .copied()
            .unwrap_or(render.style);
        let style = if render.selected_column == Some(column) {
            let preserve_fg = render
                .preserve_selected_fg
                .and_then(|preserve| preserve.get(column - render.column_offset))
                .copied()
                .unwrap_or(false);
            if preserve_fg {
                overlay_style_without_fg(base_style, render.selected_style)
            } else {
                overlay_style(base_style, render.selected_style)
            }
        } else {
            base_style
        };
        let alignment = render.alignments.get(column).copied().unwrap_or_default();
        let cell = align_cell(cell, width, "…", alignment);
        buffer.set_stringn(x, render.y, &cell, width, style);
        if let Some(query) = render.search_query {
            let search_matched = render
                .search_matches
                .and_then(|matches| matches.get(column - render.column_offset))
                .copied()
                .unwrap_or(false);
            if search_matched {
                let highlighted = highlight_search_matches(
                    buffer,
                    x,
                    render.y,
                    &cell,
                    width,
                    query,
                    render.search_style,
                );
                if !highlighted {
                    highlight_visible_cell_content(
                        buffer,
                        x,
                        render.y,
                        &cell,
                        width,
                        render.search_style,
                    );
                }
            }
        }
        if let Some(prefix_style) = render.prefix_style {
            let prefix_width = header_prefix_width(&cell).min(width);
            for offset in 0..prefix_width {
                buffer[(x + offset as u16, render.y)].set_style(overlay_style(style, prefix_style));
            }
        }
        let gap_start = x.saturating_add(width as u16);
        let marker_x = gap_start.saturating_add(render.column_gap.saturating_sub(1) as u16);
        if render.column_gap > 0
            && render
                .hidden_boundaries
                .and_then(|boundaries| boundaries.get(column + 1))
                .copied()
                .unwrap_or(false)
            && marker_x < render.area.x + render.area.width
        {
            buffer.set_stringn(marker_x, render.y, "|", 1, render.marker_style);
        }
        x = x
            .saturating_add(width as u16)
            .saturating_add(render.column_gap as u16);
    }
}

fn highlight_search_matches(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    cell: &str,
    width: usize,
    query: &CaseInsensitiveQuery<'_>,
    search_style: Style,
) -> bool {
    let search_style = style_without_bg(search_style);
    let mut highlighted = false;
    for range in query.find_iter(cell) {
        highlighted = true;
        let mut offset = UnicodeWidthStr::width(&cell[..range.start]);
        for ch in cell[range].chars() {
            let ch_width = ch.width().unwrap_or(0);
            for cell_offset in offset..offset.saturating_add(ch_width).min(width) {
                let cell = &mut buffer[(x + cell_offset as u16, y)];
                cell.set_style(overlay_style(cell.style(), search_style));
            }
            offset = offset.saturating_add(ch_width);
            if offset >= width {
                break;
            }
        }
    }
    highlighted
}

fn highlight_visible_cell_content(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    cell: &str,
    width: usize,
    search_style: Style,
) {
    let content = cell.trim();
    if content.is_empty() {
        return;
    }
    let search_style = style_without_bg(search_style);
    let start = cell
        .chars()
        .take_while(|ch| *ch == ' ')
        .map(|ch| ch.width().unwrap_or(0))
        .sum::<usize>();
    let end = start
        .saturating_add(UnicodeWidthStr::width(content))
        .min(width);
    for cell_offset in start..end {
        let cell = &mut buffer[(x + cell_offset as u16, y)];
        cell.set_style(overlay_style(cell.style(), search_style));
    }
}

fn header_prefix_width(cell: &str) -> usize {
    cell.chars()
        .take_while(|ch| matches!(ch, '▲' | '▼' | '+' | '-' | '±'))
        .map(|ch| ch.width().unwrap_or(0))
        .sum()
}

fn overlay_style(mut base: Style, overlay: Style) -> Style {
    if let Some(fg) = overlay.fg {
        base = base.fg(fg);
    }
    if let Some(bg) = overlay.bg {
        base = base.bg(bg);
    }
    base = base.add_modifier(overlay.add_modifier);
    base = base.remove_modifier(overlay.sub_modifier);
    base
}

fn overlay_style_without_fg(mut base: Style, overlay: Style) -> Style {
    if let Some(bg) = overlay.bg {
        base = base.bg(bg);
    }
    base = base.add_modifier(overlay.add_modifier);
    base = base.remove_modifier(overlay.sub_modifier);
    base
}

fn style_without_bg(mut style: Style) -> Style {
    style.bg = None;
    style
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Alignment {
    #[default]
    Left,
    Right,
}

fn column_alignments(view: &TableView) -> Vec<Alignment> {
    (0..view.column_count())
        .map(|column| match view.column_alignment(column) {
            ColumnAlignment::Left => Alignment::Left,
            ColumnAlignment::Right => Alignment::Right,
        })
        .collect()
}

fn hidden_boundaries(view: &TableView) -> Vec<bool> {
    let mut boundaries = (0..view.column_count())
        .map(|column| view.hidden_boundary_before(column))
        .collect::<Vec<_>>();
    boundaries.push(view.hidden_boundary_after_last());
    boundaries
}

fn visible_row_capacity(view: &TableView, area: Rect) -> usize {
    let header_height = usize::from(view.header_visible() && view.header().is_some());
    usize::from(area.height)
        .saturating_sub(2)
        .saturating_sub(header_height)
        .max(1)
}

fn visible_column_capacity(view: &mut TableView, area: Rect) -> usize {
    let widths = view.effective_column_widths_cached();
    let area_width = usize::from(area.width);
    let mut used = 0usize;
    let mut columns = 0usize;
    for width in widths.iter().skip(view.viewport().origin.column) {
        let gap = usize::from(columns > 0) * view.column_gap();
        let column_start = used.saturating_add(gap);
        if column_start >= area_width {
            break;
        }
        columns += 1;
        let column_end = column_start.saturating_add(*width);
        if column_end >= area_width {
            break;
        }
        used = column_end;
    }
    columns.max(1).min(view.column_count().max(1))
}

pub fn render_cell_popup(cell: &str, title: &str, area: Rect, buffer: &mut Buffer) -> bool {
    render_cell_popup_with_theme(cell, title, area, buffer, default_ui_theme())
}

pub fn render_cell_popup_with_theme(
    cell: &str,
    title: &str,
    area: Rect,
    buffer: &mut Buffer,
    theme: &ResolvedTheme,
) -> bool {
    if cell.is_empty() {
        return false;
    }
    render_popup_with_actions(title, cell, &["Close"], area, buffer, theme);
    true
}

pub fn render_info_popup(info: &str, area: Rect, buffer: &mut Buffer) {
    render_info_popup_with_theme(info, area, buffer, default_ui_theme());
}

pub fn render_info_popup_with_theme(
    info: &str,
    area: Rect,
    buffer: &mut Buffer,
    theme: &ResolvedTheme,
) {
    render_popup_with_actions("Info", info, &["Close"], area, buffer, theme);
}

pub fn render_configuration_popup_with_theme(
    title: &str,
    body: &str,
    actions: &[&str],
    area: Rect,
    buffer: &mut Buffer,
    theme: &ResolvedTheme,
) {
    render_popup_with_actions(title, body, actions, area, buffer, theme);
}

pub fn render_relation_picker_with_theme(
    relations: &[RelationCatalogEntry],
    selected: usize,
    area: Rect,
    buffer: &mut Buffer,
    theme: &ResolvedTheme,
) {
    let elasticsearch = relations.iter().any(|relation| {
        matches!(
            relation.kind,
            crate::ingest::RelationKind::Index | crate::ingest::RelationKind::DataStream
        )
    });
    render_popup_with_actions(
        if elasticsearch {
            "Select Elasticsearch target"
        } else {
            "Select table"
        },
        "",
        &["Open", "Cancel"],
        area,
        buffer,
        theme,
    );
    if area.width < 4 || area.height < 4 {
        return;
    }
    let width = area.width.saturating_sub(4) as usize;
    let height = area.height.saturating_sub(4) as usize;
    let selected = selected.min(relations.len().saturating_sub(1));
    let mut display_rows: Vec<(Option<usize>, String)> = Vec::new();
    if elasticsearch {
        for (kind, heading) in [
            (crate::ingest::RelationKind::Index, "Indices"),
            (crate::ingest::RelationKind::DataStream, "Data streams"),
        ] {
            let entries = relations
                .iter()
                .enumerate()
                .filter(|(_, relation)| relation.kind == kind)
                .collect::<Vec<_>>();
            if entries.is_empty() {
                continue;
            }
            display_rows.push((None, heading.to_owned()));
            display_rows.extend(
                entries
                    .into_iter()
                    .map(|(index, relation)| (Some(index), relation.metadata.display_name.clone())),
            );
        }
    } else {
        display_rows.extend(
            relations
                .iter()
                .enumerate()
                .map(|(index, relation)| (Some(index), relation.metadata.display_name.clone())),
        );
    }
    let selected_row = display_rows
        .iter()
        .position(|(index, _)| *index == Some(selected))
        .unwrap_or_default();
    let start = selected_row
        .saturating_add(1)
        .saturating_sub(height)
        .min(display_rows.len().saturating_sub(height));
    for (row, (offset, label)) in display_rows.iter().skip(start).take(height).enumerate() {
        let Some(offset) = *offset else {
            buffer.set_stringn(
                area.x + 2,
                area.y + 2 + row as u16,
                label,
                width,
                theme.style("popup.label"),
            );
            continue;
        };
        let relation = &relations[offset];
        let (suffix, style) = match &relation.availability {
            RelationAvailability::Selectable if offset == selected => {
                ("".to_owned(), theme.style("popup.active"))
            }
            RelationAvailability::Selectable => ("".to_owned(), theme.style("popup.body")),
            RelationAvailability::Unavailable { reason } => {
                (format!(" — {reason}"), theme.style("popup.disabled"))
            }
        };
        let prefix = if offset == selected && relation.is_selectable() {
            "> "
        } else {
            "  "
        };
        buffer.set_stringn(
            area.x + 2,
            area.y + 2 + row as u16,
            format!("{prefix}{label}{suffix}"),
            width,
            style,
        );
    }
}

#[cfg(feature = "saved-views")]
pub fn render_saved_view_popup(
    filename: &str,
    yaml: &str,
    scroll: usize,
    confirming_overwrite: bool,
    area: Rect,
    buffer: &mut Buffer,
) {
    render_saved_view_popup_with_theme(
        filename,
        yaml,
        scroll,
        confirming_overwrite,
        area,
        buffer,
        default_ui_theme(),
    );
}

#[cfg(feature = "saved-views")]
pub fn render_saved_view_popup_with_theme(
    filename: &str,
    yaml: &str,
    scroll: usize,
    confirming_overwrite: bool,
    area: Rect,
    buffer: &mut Buffer,
    theme: &ResolvedTheme,
) {
    let actions: &[&str] = if confirming_overwrite {
        &["Yes", "No"]
    } else {
        &["Save", "Cancel"]
    };
    let content_height = area.height.saturating_sub(4) as usize;
    let yaml_body = yaml
        .lines()
        .skip(scroll)
        .take(content_height)
        .collect::<Vec<_>>()
        .join("\n");
    let body = if confirming_overwrite {
        format!("Overwrite existing file?\n{filename}\n{yaml_body}")
    } else {
        format!("{filename}\n{yaml_body}")
    };
    render_popup_with_actions("View", &body, actions, area, buffer, theme);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ColumnInfoPopup {
    pub title: String,
    pub summary: String,
    pub sections: Vec<ColumnInfoSection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ColumnInfoSection {
    pub header: String,
    pub active: bool,
    pub options: Vec<ColumnInfoOption>,
    pub details: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ColumnInfoOption {
    pub label: String,
    pub selected: bool,
    pub enabled: bool,
}

#[allow(
    dead_code,
    reason = "default-theme wrapper is used by tests and callers"
)]
pub(crate) fn render_column_info_popup(popup: &ColumnInfoPopup, area: Rect, buffer: &mut Buffer) {
    render_column_info_popup_with_theme(popup, area, buffer, default_ui_theme());
}

pub(crate) fn render_column_info_popup_with_theme(
    popup: &ColumnInfoPopup,
    area: Rect,
    buffer: &mut Buffer,
    theme: &ResolvedTheme,
) {
    render_popup_with_actions(&popup.title, "", &["Save", "Cancel"], area, buffer, theme);
    if area.width < 4 || area.height < 4 {
        return;
    }

    let popup_style = theme.style("popup.body");
    let disabled_style = theme.style("popup.disabled");
    let active_style = theme.style("popup.active");
    let section_title_style = theme.style_or("popup.section_title", "popup.active");
    let option_style = theme.style("popup.action");
    let selected_option_style = theme.style_or("popup.option_selected", "popup.action");
    let content_x = area.x + 2;
    let content_y = area.y + 2;
    let content_width = area.width.saturating_sub(4) as usize;
    let content_height = area.height.saturating_sub(4) as usize;

    buffer.set_stringn(
        content_x,
        content_y,
        truncate_cell(&popup.summary, content_width, "…"),
        content_width,
        popup_style,
    );

    let column_gap = 2usize;
    let column_width = content_width.saturating_sub(column_gap) / 2;
    if column_width == 0 || content_height <= 2 {
        return;
    }
    let mut left_y = content_y + 2;
    let mut right_y = content_y + 2;
    let max_y = area.y + area.height.saturating_sub(2);

    for (idx, section) in popup.sections.iter().enumerate() {
        let column = idx % 2;
        let x = if column == 0 {
            content_x
        } else {
            content_x + column_width as u16 + column_gap as u16
        };
        let y = if column == 0 {
            &mut left_y
        } else {
            &mut right_y
        };
        render_column_info_section(
            section,
            x,
            y,
            column_width,
            max_y,
            buffer,
            active_style,
            section_title_style,
            option_style,
            selected_option_style,
            popup_style,
            disabled_style,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_column_info_section(
    section: &ColumnInfoSection,
    x: u16,
    y: &mut u16,
    width: usize,
    max_y: u16,
    buffer: &mut Buffer,
    active_style: Style,
    section_title_style: Style,
    option_style: Style,
    selected_option_style: Style,
    popup_style: Style,
    disabled_style: Style,
) {
    if *y >= max_y {
        return;
    }
    let marker = if section.active { "> " } else { "  " };
    let header = format!("{marker}{}", section.header);
    buffer.set_stringn(
        x,
        *y,
        truncate_cell(&header, width, "…"),
        width,
        if section.active {
            active_style
        } else {
            section_title_style
        },
    );
    *y += 1;

    for option in &section.options {
        if *y >= max_y {
            return;
        }
        let selected = if option.selected { "*" } else { " " };
        let text = format!("  ({selected})  {}", option.label);
        buffer.set_stringn(
            x,
            *y,
            truncate_cell(&text, width, "…"),
            width,
            if !option.enabled {
                disabled_style
            } else if option.selected {
                selected_option_style
            } else {
                option_style
            },
        );
        *y += 1;
    }

    for detail in &section.details {
        if *y >= max_y {
            return;
        }
        let text = format!("    {detail}");
        buffer.set_stringn(x, *y, truncate_cell(&text, width, "…"), width, popup_style);
        *y += 1;
    }
    *y = y.saturating_add(1);
}

pub fn render_help_popup(bindings: &[KeyBinding], area: Rect, buffer: &mut Buffer) {
    render_help_popup_with_theme(bindings, area, buffer, default_ui_theme());
}

pub fn render_help_popup_with_theme(
    bindings: &[KeyBinding],
    area: Rect,
    buffer: &mut Buffer,
    theme: &ResolvedTheme,
) {
    let content_width = area.width.saturating_sub(4) as usize;
    let key_width = bindings
        .iter()
        .map(|binding| UnicodeWidthStr::width(binding.keys))
        .max()
        .unwrap_or(1)
        .min(16);
    let body = if content_width >= 74 && bindings.len() > 12 {
        let split_at = bindings.len().div_ceil(2);
        let column_gap = 2;
        let column_width = (content_width - column_gap) / 2;
        (0..split_at)
            .map(|idx| {
                let left = format_binding(&bindings[idx], key_width, column_width);
                let right = bindings
                    .get(idx + split_at)
                    .map(|binding| format_binding(binding, key_width, column_width))
                    .unwrap_or_else(|| " ".repeat(column_width));
                format!("{left}{}{}", " ".repeat(column_gap), right)
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        let column_width = content_width.max(1);
        bindings
            .iter()
            .map(|binding| format_binding(binding, key_width, column_width))
            .collect::<Vec<_>>()
            .join("\n")
    };
    render_popup_with_actions("Help", &body, &["Close"], area, buffer, theme);
}

fn format_binding(binding: &KeyBinding, key_width: usize, column_width: usize) -> String {
    let separator_width = 1;
    if column_width <= key_width + separator_width {
        return fit_text(binding.keys, column_width);
    }
    let desc_width = column_width
        .saturating_sub(key_width)
        .saturating_sub(separator_width);
    format!(
        "{} {}",
        fit_text(binding.keys, key_width),
        fit_text(binding.description, desc_width)
    )
}

fn fit_text(value: &str, width: usize) -> String {
    truncate_cell(value, width, "…")
}

fn align_cell(cell: &str, width: usize, truncation: &str, alignment: Alignment) -> String {
    if UnicodeWidthStr::width(cell) > width {
        return truncate_cell(cell, width, truncation);
    }

    match alignment {
        Alignment::Left => format!("{cell:<width$}"),
        Alignment::Right => {
            let padding = width.saturating_sub(UnicodeWidthStr::width(cell));
            format!("{}{}", " ".repeat(padding), cell)
        }
    }
}

pub fn render_search_prompt(query: &str, area: Rect, buffer: &mut Buffer) {
    render_search_prompt_with_theme(query, area, buffer, default_ui_theme());
}

pub fn render_search_prompt_with_theme(
    query: &str,
    area: Rect,
    buffer: &mut Buffer,
    theme: &ResolvedTheme,
) {
    render_popup_with_actions(
        "Search",
        &format!("Search: {query}"),
        &["Close", "Cancel"],
        area,
        buffer,
        theme,
    );
}

#[allow(
    dead_code,
    reason = "default-theme wrapper is used by tests and callers"
)]
pub(crate) fn render_filter_prompt(prompt: &FilterPromptView<'_>, area: Rect, buffer: &mut Buffer) {
    render_filter_prompt_with_theme(prompt, area, buffer, default_ui_theme());
}

pub(crate) fn render_filter_prompt_with_theme(
    prompt: &FilterPromptView<'_>,
    area: Rect,
    buffer: &mut Buffer,
    theme: &ResolvedTheme,
) {
    let mode = match prompt.mode {
        FilterMode::In => "Filter in",
        FilterMode::Out => "Filter out",
    };
    render_popup_with_actions("Filter", "", &["Apply", "Cancel"], area, buffer, theme);
    if area.width < 5 || area.height < 5 {
        return;
    }

    let popup_style = theme.style("popup.body");
    let option_style = theme.style("popup.action");
    let disabled_style = theme.style("popup.disabled");
    let content_x = area.x + 2;
    let content_width = area.width.saturating_sub(4) as usize;
    let mut y = area.y + 2;
    let max_y = area.y + area.height.saturating_sub(2);

    buffer.set_stringn(
        content_x,
        y,
        truncate_cell(
            &format!("{mode} column {}", prompt.column + 1),
            content_width,
            "…",
        ),
        content_width,
        popup_style,
    );
    y += 1;
    if y >= max_y {
        return;
    }

    render_filter_kind_radios(
        prompt,
        content_x,
        y,
        content_width,
        buffer,
        option_style,
        disabled_style,
    );
    y += 1;
    if y >= max_y {
        return;
    }

    buffer.set_stringn(
        content_x,
        y,
        truncate_cell(&format!("Condition: {}", prompt.input), content_width, "…"),
        content_width,
        popup_style,
    );
    if let Some(error) = prompt.error {
        y += 1;
        if y < max_y {
            buffer.set_stringn(
                content_x,
                y,
                truncate_cell(error, content_width, "…"),
                content_width,
                popup_style,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_filter_kind_radios(
    prompt: &FilterPromptView<'_>,
    x: u16,
    y: u16,
    width: usize,
    buffer: &mut Buffer,
    popup_style: Style,
    disabled_style: Style,
) {
    let mut used = 0usize;
    for (idx, kind) in FilterKind::all().into_iter().enumerate() {
        let label = match kind {
            FilterKind::Text => "text",
            FilterKind::Regex => "regex",
            FilterKind::Numeric => "numeric",
        };
        let selected = if prompt.selected_kind == kind {
            "*"
        } else {
            " "
        };
        let text = format!("({selected}) {label}");
        let prefix = usize::from(idx > 0) * 2;
        let item_width = prefix + UnicodeWidthStr::width(text.as_str());
        if used + item_width > width {
            break;
        }
        let style = if prompt.enabled_kinds.contains(&kind) {
            popup_style
        } else {
            disabled_style
        };
        let item_x = x + used as u16 + prefix as u16;
        buffer.set_string(item_x, y, &text, style);
        used += item_width;
    }
}

pub fn truncate_cell(cell: &str, width: usize, truncation: &str) -> String {
    if width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(cell) <= width {
        return format!("{cell:<width$}");
    }

    let truncation_width = UnicodeWidthStr::width(truncation);
    if truncation_width >= width {
        return truncation.chars().take(1).collect();
    }

    let target = width - truncation_width;
    let mut rendered = String::new();
    let mut used = 0;
    for ch in cell.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if used + ch_width > target {
            break;
        }
        rendered.push(ch);
        used += ch_width;
    }
    rendered.push_str(&" ".repeat(target.saturating_sub(used)));
    rendered.push_str(truncation);
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::filter::{FilterKind, FilterMode};
    use crate::view::Viewport;
    use ratatui::style::Color;

    fn rows(values: &[&[&str]]) -> Vec<Vec<String>> {
        values
            .iter()
            .map(|row| row.iter().map(|cell| (*cell).to_owned()).collect())
            .collect()
    }

    fn buffer_text(buffer: &Buffer) -> String {
        let area = buffer.area;
        (area.y..area.y + area.height)
            .map(|y| {
                (area.x..area.x + area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_initial_header_layout() {
        let mut view = TableView::classify(
            rows(&[&["Name", "Value"], &["alpha", "1"]]),
            Viewport::new(10, 4),
        );
        let area = Rect::new(0, 0, 24, 6);
        let mut buffer = Buffer::empty(area);
        render_table(&mut view, area, &mut buffer);
        let text = buffer_text(&buffer);
        assert!(text.contains("(1,1)"));
        assert!(text.contains("Name"));
        assert!(text.contains("alpha"));
    }

    #[test]
    fn default_theme_uses_plain_text_for_string_and_number_cells() {
        let mut view = TableView::classify(
            rows(&[&["Name", "Value"], &["alpha", "1"]]),
            Viewport::new(10, 2),
        );
        let area = Rect::new(0, 0, 24, 6);
        let mut buffer = Buffer::empty(area);
        render_table(&mut view, area, &mut buffer);

        assert_eq!(buffer[(0, 2)].style().fg, Some(Color::Rgb(0, 255, 255)));
        assert_eq!(buffer[(7, 2)].style().fg, Some(Color::Rgb(0, 192, 192)));
        assert_eq!(buffer[(0, 3)].style().fg, Some(Color::Rgb(175, 175, 175)));
        assert_eq!(buffer[(7, 3)].style().fg, Some(Color::Rgb(175, 175, 175)));
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn default_theme_uses_plain_text_for_boolean_columns() {
        let mut view = TableView::classify(
            rows(&[&["Flag"], &["true"], &["false"]]),
            Viewport::new(10, 1),
        );
        let parsed = crate::saved_views::parse_saved_view_yaml(
            r#"
name: flags
filenames: [flags.csv]
source: {}
view:
  columns:
    Flag:
      type: boolean
"#,
        )
        .expect("parse");
        view.install_saved_binding(parsed.view.view, true);

        let area = Rect::new(0, 0, 16, 5);
        let mut buffer = Buffer::empty(area);
        render_table(&mut view, area, &mut buffer);

        assert_eq!(buffer[(0, 3)].style().fg, Some(Color::Rgb(175, 175, 175)));
    }

    #[test]
    fn renders_without_header_when_not_classified() {
        let mut view = TableView::classify(rows(&[&["1", "2"], &["3", "4"]]), Viewport::new(10, 4));
        let area = Rect::new(0, 0, 20, 5);
        let mut buffer = Buffer::empty(area);
        render_table(&mut view, area, &mut buffer);
        let text = buffer_text(&buffer);
        assert!(text.contains("1"));
        assert!(text.contains("3"));
    }

    #[test]
    fn renders_from_viewport_origin() {
        let mut view = TableView::classify(
            rows(&[
                &["A", "B", "C"],
                &["r1c1", "r1c2", "r1c3"],
                &["r2c1", "r2c2", "r2c3"],
                &["r3c1", "r3c2", "r3c3"],
            ]),
            Viewport::new(1, 1),
        );
        view.goto(2, 2);
        let area = Rect::new(0, 0, 16, 4);
        let mut buffer = Buffer::empty(area);
        render_table(&mut view, area, &mut buffer);
        let text = buffer_text(&buffer);
        assert!(text.contains("(3,3)"));
        assert!(text.contains("r3c3"));
        assert!(!text.contains("r1c1"));
    }

    #[test]
    fn renders_only_filtered_visible_rows() {
        let mut view = TableView::classify(
            rows(&[&["Name"], &["alpha"], &["beta"], &["gamma"]]),
            Viewport::new(10, 1),
        );
        view.apply_filter(0, FilterMode::In, FilterKind::Text, "alpha".to_owned())
            .expect("apply filter");
        let area = Rect::new(0, 0, 24, 6);
        let mut buffer = Buffer::empty(area);
        render_table(&mut view, area, &mut buffer);

        let text = buffer_text(&buffer);
        assert!(text.contains("+Name"));
        assert!(text.contains("alpha"));
        assert!(!text.contains("beta"));
        assert!(!text.contains("gamma"));
    }

    #[test]
    fn renders_header_when_filter_hides_every_data_row() {
        let mut view = TableView::classify(
            rows(&[&["Name"], &["alpha"], &["beta"]]),
            Viewport::new(10, 1),
        );
        view.apply_filter(0, FilterMode::In, FilterKind::Text, "zzz".to_owned())
            .expect("apply filter");
        let area = Rect::new(0, 0, 24, 6);
        let mut buffer = Buffer::empty(area);
        render_table(&mut view, area, &mut buffer);

        let text = buffer_text(&buffer);
        assert!(text.contains("+Name"));
        assert!(!text.contains("alpha"));
        assert!(!text.contains("beta"));
    }

    #[test]
    fn header_truncation_keeps_sort_and_filter_prefixes() {
        let mut view = TableView::classify(
            rows(&[&["Name"], &["alpha"], &["beta"]]),
            Viewport::new(10, 1),
        );
        view.set_all_column_widths(4);
        view.apply_filter(0, FilterMode::In, FilterKind::Text, "a".to_owned())
            .expect("apply filter");
        view.sort_current_column(
            crate::ops::sort::SortMode::Lexical,
            crate::ops::sort::SortDirection::Ascending,
        );

        let area = Rect::new(0, 0, 16, 6);
        let mut buffer = Buffer::empty(area);
        render_table(&mut view, area, &mut buffer);

        assert_eq!(buffer[(0, 2)].symbol(), "▲");
        assert_eq!(buffer[(1, 2)].symbol(), "+");
        assert_eq!(buffer[(2, 2)].symbol(), "N");
        assert_eq!(buffer[(3, 2)].symbol(), "…");
        assert_eq!(buffer[(0, 2)].style().fg, Some(Color::Indexed(242)));
        assert_eq!(buffer[(1, 2)].style().fg, Some(Color::Indexed(242)));
        assert_eq!(buffer[(2, 2)].style().fg, Some(Color::Rgb(0, 255, 255)));
    }

    #[test]
    fn hidden_column_marker_does_not_shift_header_columns() {
        let mut view = TableView::classify(
            rows(&[&["AA", "BB", "CC"], &["aa", "bb", "cc"]]),
            Viewport::new(10, 3),
        );
        view.set_all_column_widths(2);
        view.goto(0, 1);
        view.hide_current_column();

        let area = Rect::new(0, 0, 16, 5);
        let mut buffer = Buffer::empty(area);
        render_table(&mut view, area, &mut buffer);

        assert_eq!(buffer[(3, 2)].symbol(), "|");
        assert_eq!(buffer[(4, 2)].symbol(), "C");
        assert_eq!(buffer[(5, 2)].symbol(), "C");
        assert_eq!(buffer[(4, 3)].symbol(), "c");
        assert_eq!(buffer[(5, 3)].symbol(), "c");
    }

    #[test]
    fn right_aligns_numeric_columns() {
        let mut view = TableView::classify(
            rows(&[&["Name", "Value"], &["alpha", "2"], &["beta", "100"]]),
            Viewport::new(10, 2),
        );
        view.set_all_column_widths(5);
        let area = Rect::new(0, 0, 20, 6);
        let mut buffer = Buffer::empty(area);
        render_table(&mut view, area, &mut buffer);

        assert_eq!(buffer[(7, 2)].symbol(), "V");
        assert_eq!(buffer[(8, 2)].symbol(), "a");
        assert_eq!(buffer[(7, 3)].symbol(), " ");
        assert_eq!(buffer[(8, 3)].symbol(), " ");
        assert_eq!(buffer[(9, 3)].symbol(), " ");
        assert_eq!(buffer[(10, 3)].symbol(), " ");
        assert_eq!(buffer[(11, 3)].symbol(), "2");
    }

    #[test]
    fn right_aligns_suffixed_numeric_columns() {
        let mut view = TableView::classify(
            rows(&[&["Name", "Size"], &["alpha", "2MiB"], &["beta", "512kb"]]),
            Viewport::new(10, 2),
        );
        view.set_all_column_widths(6);
        let area = Rect::new(0, 0, 24, 6);
        let mut buffer = Buffer::empty(area);
        render_table(&mut view, area, &mut buffer);

        assert_eq!(buffer[(8, 3)].symbol(), " ");
        assert_eq!(buffer[(9, 3)].symbol(), " ");
        assert_eq!(buffer[(10, 3)].symbol(), "2");
        assert_eq!(buffer[(11, 3)].symbol(), "M");
        assert_eq!(buffer[(12, 3)].symbol(), "i");
        assert_eq!(buffer[(13, 3)].symbol(), "B");
    }

    #[test]
    fn right_aligns_time_hint_numeric_columns() {
        let mut view = TableView::classify(
            rows(&[&["Name", "Duration"], &["alpha", "2m"], &["beta", "30"]]),
            Viewport::new(10, 2),
        );
        view.set_all_column_widths(5);
        let area = Rect::new(0, 0, 20, 6);
        let mut buffer = Buffer::empty(area);
        render_table(&mut view, area, &mut buffer);

        assert_eq!(buffer[(9, 3)].symbol(), " ");
        assert_eq!(buffer[(10, 3)].symbol(), "2");
        assert_eq!(buffer[(11, 3)].symbol(), "m");
    }

    #[test]
    fn right_aligns_percent_numeric_columns() {
        let mut view = TableView::classify(
            rows(&[&["Name", "Rate"], &["alpha", "2.5%"], &["beta", "100%"]]),
            Viewport::new(10, 2),
        );
        view.set_all_column_widths(6);
        let area = Rect::new(0, 0, 22, 6);
        let mut buffer = Buffer::empty(area);
        render_table(&mut view, area, &mut buffer);

        assert_eq!(buffer[(8, 3)].symbol(), " ");
        assert_eq!(buffer[(9, 3)].symbol(), " ");
        assert_eq!(buffer[(10, 3)].symbol(), "2");
        assert_eq!(buffer[(11, 3)].symbol(), ".");
        assert_eq!(buffer[(12, 3)].symbol(), "5");
        assert_eq!(buffer[(13, 3)].symbol(), "%");
    }

    #[test]
    fn ignores_placeholders_when_aligning_numeric_columns() {
        let mut view = TableView::classify(
            rows(&[
                &["Name", "Value"],
                &["alpha", "2.5%"],
                &["beta", "null"],
                &["gamma", "N/A"],
            ]),
            Viewport::new(10, 4),
        );
        view.set_all_column_widths(6);
        let area = Rect::new(0, 0, 22, 8);
        let mut buffer = Buffer::empty(area);
        render_table(&mut view, area, &mut buffer);

        assert_eq!(buffer[(8, 3)].symbol(), " ");
        assert_eq!(buffer[(9, 3)].symbol(), " ");
        assert_eq!(buffer[(10, 3)].symbol(), "2");
        assert_eq!(buffer[(11, 3)].symbol(), ".");
        assert_eq!(buffer[(12, 3)].symbol(), "5");
        assert_eq!(buffer[(13, 3)].symbol(), "%");
        assert_eq!(buffer[(8, 4)].symbol(), " ");
        assert_eq!(buffer[(10, 4)].symbol(), "n");
        assert_eq!(buffer[(13, 5)].symbol(), "A");
    }

    #[test]
    fn renders_popup_box() {
        let area = Rect::new(0, 0, 20, 5);
        let mut buffer = Buffer::empty(area);
        render_popup("Cell", "contents", area, &mut buffer);
        let text = buffer_text(&buffer);
        assert!(text.contains("Cell"));
        assert!(text.contains("contents"));
        assert!(text.contains("┌"));
        assert_eq!(buffer[(1, 1)].style().bg, Some(Color::Indexed(19)));
        assert_eq!(buffer[(0, 0)].style().fg, Some(Color::Rgb(0, 255, 255)));
        assert_eq!(buffer[(2, 0)].style().fg, Some(Color::Rgb(160, 160, 160)));
        assert_eq!(buffer[(1, 1)].symbol(), " ");
        assert_eq!(buffer[(1, 2)].symbol(), " ");
        assert_eq!(buffer[(2, 2)].symbol(), "c");
    }

    #[test]
    fn renders_column_info_disabled_options_dim() {
        let area = Rect::new(0, 0, 48, 12);
        let mut buffer = Buffer::empty(area);
        let popup = ColumnInfoPopup {
            title: "Column Info".to_owned(),
            summary: "Name  visible:1 source:1".to_owned(),
            sections: vec![ColumnInfoSection {
                header: "Format".to_owned(),
                active: true,
                options: vec![
                    ColumnInfoOption {
                        label: "plain".to_owned(),
                        selected: true,
                        enabled: true,
                    },
                    ColumnInfoOption {
                        label: "locale".to_owned(),
                        selected: false,
                        enabled: false,
                    },
                ],
                details: Vec::new(),
            }],
        };

        render_column_info_popup(&popup, area, &mut buffer);

        assert_eq!(buffer[(2, 4)].style().fg, Some(Color::Rgb(160, 160, 160)));
        assert_eq!(buffer[(4, 5)].style().fg, Some(Color::Rgb(0, 255, 255)));
        assert_eq!(buffer[(4, 5)].style().bg, Some(Color::Indexed(19)));
        assert_eq!(buffer[(2, 6)].style().fg, Some(Color::Indexed(240)));
        assert_eq!(buffer[(4, 6)].symbol(), "(");
        assert!(buffer_text(&buffer).contains("[ Save ] [ Cancel ]"));
        assert_eq!(buffer[(46, 11)].symbol(), "─");
    }

    #[test]
    fn empty_cell_popup_is_noop() {
        let area = Rect::new(0, 0, 20, 5);
        let mut buffer = Buffer::empty(area);
        assert!(!render_cell_popup("", "Cell", area, &mut buffer));
    }

    #[test]
    fn renders_help_from_bindings_and_search_prompt() {
        let area = Rect::new(0, 0, 40, 8);
        let mut buffer = Buffer::empty(area);
        render_help_popup(&crate::command::default_key_bindings(), area, &mut buffer);
        let text = buffer_text(&buffer);
        assert!(text.contains("Move selection"));

        let mut buffer = Buffer::empty(area);
        render_search_prompt("abc", area, &mut buffer);
        let text = buffer_text(&buffer);
        assert!(text.contains("Search: abc"));
    }

    #[test]
    fn renders_filter_prompt_with_radios_disabled_numeric_and_error() {
        let area = Rect::new(0, 0, 56, 8);
        let mut buffer = Buffer::empty(area);
        let enabled = [FilterKind::Text, FilterKind::Regex];
        let prompt = crate::FilterPromptView {
            mode: FilterMode::In,
            column: 0,
            selected_kind: FilterKind::Regex,
            enabled_kinds: &enabled,
            input: "^foo",
            error: Some("invalid regex"),
        };

        render_filter_prompt(&prompt, area, &mut buffer);
        let text = buffer_text(&buffer);
        assert!(text.contains("Filter in column 1"));
        assert!(text.contains("( ) text"));
        assert!(text.contains("(*) regex"));
        assert!(text.contains("( ) numeric"));
        assert_eq!(buffer[(23, 3)].style().fg, Some(Color::Indexed(240)));
        assert_eq!(buffer[(4, 3)].style().fg, Some(Color::Rgb(0, 255, 255)));
        assert!(text.contains("Condition: ^foo"));
        assert!(text.contains("invalid regex"));
    }

    #[test]
    fn renders_footer_message_on_last_line() {
        let area = Rect::new(0, 0, 24, 4);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "table", Style::default());

        render_footer(Some("saved view warning"), area, &mut buffer);

        let text = buffer_text(&buffer);
        assert!(text.lines().next().expect("first line").contains("table"));
        assert!(text
            .lines()
            .last()
            .expect("last line")
            .contains("saved view warning"));
    }

    #[test]
    fn renders_loading_then_operation_progress_on_the_status_line() {
        let area = Rect::new(0, 0, 48, 4);
        let mut loading = Buffer::empty(area);
        render_footer(Some("Loading response.json"), area, &mut loading);
        assert!(buffer_text(&loading)
            .lines()
            .last()
            .unwrap()
            .contains("Loading response.json"));

        let mut progress = Buffer::empty(area);
        render_footer(Some("Indexed 4096 rows"), area, &mut progress);
        assert!(buffer_text(&progress)
            .lines()
            .last()
            .unwrap()
            .contains("Indexed 4096 rows"));
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn renders_provisional_json_partial_count_and_late_overridden_label() {
        use crate::ingest::SourceAdapter;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("late.json");
        std::fs::write(
            &path,
            r#"[{"a":1},{"a":2},{"a":3},{"a":4},{"a":5,"late":6}]"#,
        )
        .expect("write");
        let options = crate::ingest::OpenOptions {
            format: crate::ingest::InputFormat::Json,
            lazy_threshold_bytes: 1,
            schema_scan_bytes: 1,
            ..crate::ingest::OpenOptions::default()
        };
        let opened = crate::ingest::JsonAdapter::json()
            .open(crate::ingest::source::InputSource::Path(path), &options)
            .expect("open")
            .into_implicit_table()
            .expect("table");
        let mut view = TableView::from_opened_table(opened, Viewport::new(1, 3)).expect("view");
        let saved = crate::saved_views::parse_saved_view_yaml(
            r#"
name: late
filenames: [late.json]
source: {}
view:
  columns:
    /late:
      label: Later
"#,
        )
        .expect("saved");
        view.install_saved_binding(saved.view.view, true);

        let area = Rect::new(0, 0, 48, 6);
        let mut initial = Buffer::empty(area);
        render_table(&mut view, area, &mut initial);
        let initial = buffer_text(&initial);
        assert!(initial.contains("a"));
        assert!(
            initial.contains('+'),
            "partial row count should be explicit"
        );
        assert!(!initial.contains("Later"));

        view.goto(4, 0);
        let mut completed = Buffer::empty(area);
        render_table(&mut view, area, &mut completed);
        let completed = buffer_text(&completed);
        assert!(completed.contains("Later"));
        assert!(completed.contains("6"));
    }

    #[test]
    fn renders_source_defined_delimited_header() {
        use crate::ingest::SourceAdapter;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data.csv");
        std::fs::write(&path, "Server,Requests\napi-1,42\n").expect("write");
        let opened = crate::ingest::DelimitedAdapter
            .open(
                crate::ingest::source::InputSource::Path(path),
                &crate::ingest::OpenOptions::default(),
            )
            .expect("open")
            .into_implicit_table()
            .expect("table");
        let mut view = TableView::from_opened_table(opened, Viewport::new(4, 3)).expect("view");
        let area = Rect::new(0, 0, 48, 6);
        let mut buffer = Buffer::empty(area);
        render_table(&mut view, area, &mut buffer);
        let text = buffer_text(&buffer);
        assert_eq!(
            view.rendered_header().expect("rendered header"),
            ["Server", "Requests"]
        );
        assert!(text.contains("Server"));
        assert!(text.contains("api-1"));
    }

    #[test]
    fn first_render_fills_resized_store_backed_viewport() {
        use crate::ingest::SourceAdapter;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rows.json");
        let json = (0..20)
            .map(|row| format!(r#"{{"value":"row-{row:02}"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        std::fs::write(&path, format!("[{json}]")).expect("write");
        let opened = crate::ingest::JsonAdapter::json()
            .open(
                crate::ingest::source::InputSource::Path(path),
                &crate::ingest::OpenOptions::default(),
            )
            .expect("open")
            .into_implicit_table()
            .expect("table");
        let mut view = TableView::from_opened_table(opened, Viewport::new(2, 10)).expect("view");
        assert_eq!(view.rows().len(), 2);

        let area = Rect::new(0, 0, 40, 9);
        let mut buffer = Buffer::empty(area);
        render_table(&mut view, area, &mut buffer);

        assert_eq!(view.rows().len(), 6);
        assert!(buffer_text(&buffer).contains("row-05"));
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn themed_rendering_applies_conditional_colors_and_search_highlight() {
        let mut view = TableView::classify(
            rows(&[&["Status"], &["active"], &["idle"]]),
            Viewport::new(10, 1),
        );
        let parsed = crate::saved_views::parse_saved_view_yaml(
            r#"
name: colors
filenames: [data.csv]
source: {}
view:
  columns:
    Status:
      colors:
        - match:
            active: green
"#,
        )
        .expect("parse");
        view.install_saved_binding(parsed.view.view, true);

        let area = Rect::new(0, 0, 20, 5);
        let mut buffer = Buffer::empty(area);
        let theme = crate::theme::default_theme();
        render_table_with_theme(&mut view, area, &mut buffer, &theme, Some("idle"));

        assert_eq!(buffer[(0, 3)].style().fg, Some(Color::Rgb(0, 192, 0)));
        let unchanged_bg = buffer[(4, 4)].style().bg;
        for x in 0..4 {
            assert_eq!(buffer[(x, 4)].style().fg, Some(Color::Rgb(255, 255, 0)));
            assert_eq!(buffer[(x, 4)].style().bg, unchanged_bg);
            assert!(buffer[(x, 4)]
                .style()
                .add_modifier
                .contains(ratatui::style::Modifier::UNDERLINED));
        }
        assert_eq!(buffer[(4, 4)].style().fg, Some(Color::Rgb(175, 175, 175)));
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn selected_conditional_foreground_survives_outside_search_substring() {
        let mut view = TableView::classify(
            rows(&[&["Status"], &["active"], &["idle"]]),
            Viewport::new(10, 1),
        );
        let saved = crate::saved_views::parse_saved_view_yaml(
            "name: selected\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    Status:\n      colors:\n        - match:\n            active: green\n",
        )
        .expect("saved view");
        view.install_saved_binding(saved.view.view, true);
        let theme = crate::theme::default_theme();
        let area = Rect::new(0, 0, 20, 5);
        let mut buffer = Buffer::empty(area);
        render_table_with_theme(&mut view, area, &mut buffer, &theme, Some("ct"));

        let selected_bg = theme.style("table.selected").bg;
        for x in [0, 3, 4, 5] {
            assert_eq!(buffer[(x, 3)].style().fg, Some(Color::Rgb(0, 192, 0)));
            assert_eq!(buffer[(x, 3)].style().bg, selected_bg);
            assert!(!buffer[(x, 3)]
                .style()
                .add_modifier
                .contains(ratatui::style::Modifier::UNDERLINED));
        }
        for x in [1, 2] {
            assert_eq!(
                buffer[(x, 3)].style().fg,
                theme.style("search.highlight").fg
            );
            assert_eq!(buffer[(x, 3)].style().bg, selected_bg);
            assert!(buffer[(x, 3)]
                .style()
                .add_modifier
                .contains(ratatui::style::Modifier::UNDERLINED));
        }
        assert_eq!(buffer[(0, 4)].style().fg, theme.style("table.cell").fg);
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn search_highlight_marks_rendered_content_for_raw_only_matches() {
        let mut view = TableView::classify(rows(&[&["Count"], &["1000"]]), Viewport::new(10, 1));
        let parsed = crate::saved_views::parse_saved_view_yaml(
            r##"
name: counts
filenames: [counts.csv]
source: {}
view:
  columns:
    Count:
      type: integer
      format: mask
      mask: "#,##0"
"##,
        )
        .expect("parse");
        view.install_saved_binding(parsed.view.view, true);

        let area = Rect::new(0, 0, 12, 5);
        let mut buffer = Buffer::empty(area);
        let theme = crate::theme::default_theme();
        render_table_with_theme(&mut view, area, &mut buffer, &theme, Some("1000"));

        assert_eq!(buffer_text(&buffer).lines().nth(3), Some("1,000       "));
        for x in 0..5 {
            assert_eq!(buffer[(x, 3)].style().fg, Some(Color::Rgb(255, 255, 0)));
            assert!(buffer[(x, 3)]
                .style()
                .add_modifier
                .contains(ratatui::style::Modifier::UNDERLINED));
        }
        assert_ne!(buffer[(5, 3)].style().fg, Some(Color::Rgb(255, 255, 0)));
    }

    #[test]
    fn search_highlight_ignores_padding_when_cell_does_not_match() {
        let mut view = TableView::classify(rows(&[&["N"], &["a"]]), Viewport::new(10, 1));
        view.set_all_column_widths(4);
        let area = Rect::new(0, 0, 8, 5);
        let mut buffer = Buffer::empty(area);
        let theme = default_theme();

        render_table_with_theme(&mut view, area, &mut buffer, &theme, Some(" "));

        assert_eq!(buffer_text(&buffer).lines().nth(3), Some("a       "));
        for x in 1..4 {
            assert_ne!(buffer[(x, 3)].style().fg, Some(Color::Rgb(255, 255, 0)));
        }
    }

    #[test]
    fn rendering_clamps_wide_columns_to_remaining_area() {
        let mut view = TableView::classify(rows(&[&["Name"], &["alphabet"]]), Viewport::new(10, 1));
        view.set_all_column_widths(20);
        let area = Rect::new(0, 0, 6, 5);
        let mut buffer = Buffer::empty(area);

        render_table_with_theme(
            &mut view,
            area,
            &mut buffer,
            &default_theme(),
            Some("alphabet"),
        );

        assert_eq!(buffer_text(&buffer).lines().nth(3), Some("alpha…"));
    }

    #[test]
    fn rendering_clips_partial_final_column_instead_of_skipping_it() {
        let mut view =
            TableView::classify(rows(&[&["A", "B"], &["aaa", "bbb"]]), Viewport::new(10, 2));
        view.set_all_column_widths(3);
        let area = Rect::new(0, 0, 6, 5);
        let mut buffer = Buffer::empty(area);

        render_table_with_theme(&mut view, area, &mut buffer, &default_theme(), Some("bbb"));

        let text = buffer_text(&buffer);
        assert_eq!(text.lines().nth(2), Some("A    B"));
        assert_eq!(text.lines().nth(3), Some("aaa  …"));
        assert_eq!(buffer[(5, 3)].symbol(), "…");
    }

    #[test]
    fn renders_help_columns_with_stable_alignment() {
        let area = Rect::new(0, 0, 78, 22);
        let mut buffer = Buffer::empty(area);
        render_help_popup(&crate::command::default_key_bindings(), area, &mut buffer);
        let text = buffer_text(&buffer);

        assert!(text.contains("PgUp/PgDn/J/K"));
        assert!(text.contains("Move a page"));
        assert!(!text.contains("PgUp/PgDn/J/KMove"));

        assert!(text.contains("r"));
        assert!(text.contains("s/S"));
        assert!(text.contains("Lexical sort"));
        assert!(text.contains("a/A"));
        assert!(text.contains("Natural sort"));
        assert!(text.contains("#/@"));
        assert!(text.contains("Numeric sort"));
    }

    #[test]
    fn relation_picker_distinguishes_selectable_and_disabled_entries() {
        let relations = vec![
            RelationCatalogEntry {
                metadata: crate::table::RelationMetadata {
                    name: "users".to_owned(),
                    display_name: "users".to_owned(),
                    header_visible: true,
                },
                kind: crate::ingest::RelationKind::Table,
                availability: RelationAvailability::Selectable,
            },
            RelationCatalogEntry {
                metadata: crate::table::RelationMetadata {
                    name: "search".to_owned(),
                    display_name: "search".to_owned(),
                    header_visible: true,
                },
                kind: crate::ingest::RelationKind::VirtualTable,
                availability: RelationAvailability::Unavailable {
                    reason: "virtual tables are unsupported".to_owned(),
                },
            },
        ];
        let area = Rect::new(0, 0, 64, 10);
        let mut buffer = Buffer::empty(area);
        let theme = default_theme();

        render_relation_picker_with_theme(&relations, 0, area, &mut buffer, &theme);

        let text = buffer_text(&buffer);
        assert!(text.contains("Select table"));
        assert!(text.contains("> users"));
        assert!(text.contains("search — virtual tables are unsupported"));
        assert_eq!(buffer[(2, 3)].style().fg, theme.style("popup.disabled").fg);
        assert_eq!(buffer[(2, 3)].style().bg, theme.style("popup.disabled").bg);
    }

    #[test]
    fn relation_picker_scrolls_to_keep_the_selection_visible() {
        let relations = (0..8)
            .map(|index| RelationCatalogEntry {
                metadata: crate::table::RelationMetadata {
                    name: format!("table-{index}"),
                    display_name: format!("table-{index}"),
                    header_visible: true,
                },
                kind: crate::ingest::RelationKind::Table,
                availability: RelationAvailability::Selectable,
            })
            .collect::<Vec<_>>();
        let area = Rect::new(0, 0, 32, 10);
        let mut buffer = Buffer::empty(area);
        let theme = default_theme();

        render_relation_picker_with_theme(&relations, 7, area, &mut buffer, &theme);

        let text = buffer_text(&buffer);
        assert!(!text.contains("table-0"));
        assert!(text.contains("> table-7"));
        assert_eq!(buffer[(2, 7)].style().fg, theme.style("popup.active").fg);
        assert_eq!(buffer[(2, 7)].style().bg, theme.style("popup.active").bg);
    }

    #[cfg(feature = "elasticsearch")]
    #[test]
    fn elasticsearch_picker_renders_index_and_data_stream_sections() {
        let relations = vec![
            RelationCatalogEntry {
                metadata: crate::table::RelationMetadata::implicit("logs-a", true),
                kind: crate::ingest::RelationKind::Index,
                availability: RelationAvailability::Selectable,
            },
            RelationCatalogEntry {
                metadata: crate::table::RelationMetadata::implicit("events-prod", true),
                kind: crate::ingest::RelationKind::DataStream,
                availability: RelationAvailability::Selectable,
            },
        ];
        let area = Rect::new(0, 0, 64, 12);
        let mut buffer = Buffer::empty(area);

        render_relation_picker_with_theme(&relations, 1, area, &mut buffer, &default_theme());

        let text = buffer_text(&buffer);
        assert!(text.contains("Select Elasticsearch target"));
        assert!(text.contains("Indices"));
        assert!(text.contains("logs-a"));
        assert!(text.contains("Data streams"));
        assert!(text.contains("> events-prod"));
    }

    #[cfg(feature = "elasticsearch")]
    #[test]
    fn elasticsearch_picker_handles_tiny_terminal_without_render_overflow() {
        let relations = vec![RelationCatalogEntry {
            metadata: crate::table::RelationMetadata::implicit("logs-a", true),
            kind: crate::ingest::RelationKind::Index,
            availability: RelationAvailability::Selectable,
        }];
        for area in [Rect::new(0, 0, 1, 1), Rect::new(0, 0, 3, 3)] {
            let mut buffer = Buffer::empty(area);
            render_relation_picker_with_theme(&relations, 0, area, &mut buffer, &default_theme());
        }
    }

    #[test]
    fn truncates_unicode_aware_cells() {
        assert_eq!(truncate_cell("abcdef", 4, "…"), "abc…");
        assert_eq!(truncate_cell("中abcdef", 4, "…"), "中a…");
    }
}
