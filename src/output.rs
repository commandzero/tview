use std::io::{self, BufWriter, Write};

use clap::ValueEnum;
use ratatui::style::{Color, Modifier, Style};
use unicode_width::UnicodeWidthChar;

use crate::theme::ResolvedTheme;
use crate::view::{ColumnAlignment, TableView};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Table,
    Json,
    Jsonl,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum ColorOutput {
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    Interactive { emit_on_exit: Option<OutputFormat> },
    Batch(OutputFormat),
}

pub fn resolve_execution_mode(
    interactive: bool,
    output: Option<OutputFormat>,
    stdout_is_terminal: bool,
) -> ExecutionMode {
    if interactive {
        ExecutionMode::Interactive {
            emit_on_exit: output,
        }
    } else if let Some(format) = output {
        ExecutionMode::Batch(format)
    } else if stdout_is_terminal {
        ExecutionMode::Interactive { emit_on_exit: None }
    } else {
        ExecutionMode::Batch(OutputFormat::Table)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutputRequirements {
    pub complete_rows: bool,
    pub stable_widths: bool,
    pub rendered_values: bool,
    pub conditional_styles: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedColumn {
    pub alignment: ColumnAlignment,
    pub width_override: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedCell {
    pub text: String,
    pub style: Style,
}

pub trait PreparedRows {
    fn len(&self) -> usize;
    fn row(&self, index: usize) -> Vec<PreparedCell>;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct PreparedOutput<'a> {
    pub header_visible: bool,
    pub header: Vec<PreparedCell>,
    pub rows: Box<dyn PreparedRows + 'a>,
    pub columns: Vec<PreparedColumn>,
    pub gap: usize,
}

pub trait OutputAdapter {
    fn requirements(&self) -> OutputRequirements;
    fn supports_color(&self) -> bool;
    fn write(
        &self,
        prepared: &PreparedOutput<'_>,
        color: ColorOutput,
        writer: &mut dyn Write,
    ) -> io::Result<()>;
}

#[derive(Debug, Default)]
pub struct FixedWidthTableAdapter {
    trim_trailing: bool,
}

impl OutputAdapter for FixedWidthTableAdapter {
    fn requirements(&self) -> OutputRequirements {
        OutputRequirements {
            complete_rows: true,
            stable_widths: true,
            rendered_values: true,
            conditional_styles: true,
        }
    }

    fn supports_color(&self) -> bool {
        true
    }

    fn write(
        &self,
        prepared: &PreparedOutput<'_>,
        color: ColorOutput,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        let normalized_header = prepared
            .header
            .iter()
            .map(|cell| PreparedCell {
                text: normalize_controls(&cell.text),
                style: cell.style,
            })
            .collect::<Vec<_>>();
        let widths = resolved_widths(&normalized_header, prepared);
        let gap = vec![b' '; prepared.gap];

        if prepared.header_visible && !normalized_header.is_empty() {
            write_line(
                writer,
                &normalized_header,
                &prepared.columns,
                &widths,
                &gap,
                color,
                self.trim_trailing,
            )?;
        }
        for row_index in 0..prepared.rows.len() {
            let row = normalize_row(prepared.rows.row(row_index));
            write_line(
                writer,
                &row,
                &prepared.columns,
                &widths,
                &gap,
                color,
                self.trim_trailing,
            )?;
        }
        Ok(())
    }
}

fn adapter(format: OutputFormat) -> Box<dyn OutputAdapter> {
    match format {
        OutputFormat::Table => Box::<FixedWidthTableAdapter>::default(),
        OutputFormat::Json => Box::new(JsonAdapter { lines: false }),
        OutputFormat::Jsonl => Box::new(JsonAdapter { lines: true }),
    }
}

/// Serializes the displayed values without terminal styling or width clipping.
/// Positional cells preserve duplicate labels and headerless input.
struct JsonAdapter {
    lines: bool,
}

impl OutputAdapter for JsonAdapter {
    fn requirements(&self) -> OutputRequirements {
        OutputRequirements {
            complete_rows: true,
            rendered_values: true,
            ..OutputRequirements::default()
        }
    }

    fn supports_color(&self) -> bool {
        false
    }

    fn write(
        &self,
        prepared: &PreparedOutput<'_>,
        _color: ColorOutput,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        let columns: Vec<&str> = if prepared.header_visible {
            prepared
                .header
                .iter()
                .map(|cell| cell.text.as_str())
                .collect()
        } else {
            Vec::new()
        };
        if !self.lines {
            writer.write_all(b"{\"columns\":")?;
            serde_json::to_writer(&mut *writer, &columns)?;
            writer.write_all(b",\"rows\":[")?;
        }
        for index in 0..prepared.rows.len() {
            let row: Vec<String> = prepared
                .rows
                .row(index)
                .into_iter()
                .map(|cell| cell.text)
                .collect();
            if self.lines {
                #[derive(serde::Serialize)]
                struct Record<'a> {
                    columns: &'a [&'a str],
                    values: &'a [String],
                }
                serde_json::to_writer(
                    &mut *writer,
                    &Record {
                        columns: &columns,
                        values: &row,
                    },
                )?;
                writer.write_all(b"\n")?;
            } else {
                if index != 0 {
                    writer.write_all(b",")?;
                }
                serde_json::to_writer(&mut *writer, &row)?;
            }
        }
        if !self.lines {
            writer.write_all(b"]}\n")?;
        }
        Ok(())
    }
}

pub fn write_view(
    format: OutputFormat,
    color: ColorOutput,
    view: &mut TableView,
    theme: &ResolvedTheme,
    writer: &mut dyn Write,
) -> anyhow::Result<()> {
    let adapter = adapter(format);
    if color == ColorOutput::Always && !adapter.supports_color() {
        anyhow::bail!("output format does not support --color always");
    }
    let mut requirements = adapter.requirements();
    requirements.conditional_styles &= color == ColorOutput::Always;
    let prepared = prepare(view, theme, requirements)?;
    match adapter.write(&prepared, color, writer) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub fn write_view_to_stdout(
    format: OutputFormat,
    color: ColorOutput,
    view: &mut TableView,
    theme: &ResolvedTheme,
) -> anyhow::Result<()> {
    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());
    write_view(format, color, view, theme, &mut writer)?;
    flush_output(&mut writer)
}

pub(crate) fn write_preview_to_stdout(
    color: ColorOutput,
    view: &mut TableView,
    theme: &ResolvedTheme,
    remaining: Option<crate::table::RowCount>,
) -> anyhow::Result<()> {
    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());
    let requirements = OutputRequirements {
        conditional_styles: color == ColorOutput::Always,
        ..OutputRequirements::default()
    };
    let prepared = prepare(view, theme, requirements)?;
    let result = (|| -> io::Result<()> {
        FixedWidthTableAdapter {
            trim_trailing: true,
        }
        .write(&prepared, color, &mut writer)?;
        match remaining {
            Some(crate::table::RowCount::Exact(count)) => writeln!(writer, "{count} more rows..."),
            Some(_) => writeln!(writer, "more rows..."),
            None => Ok(()),
        }
    })();
    match result {
        Ok(()) => flush_output(&mut writer),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn flush_output(writer: &mut dyn Write) -> anyhow::Result<()> {
    match writer.flush() {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn prepare<'a>(
    view: &'a mut TableView,
    theme: &'a ResolvedTheme,
    requirements: OutputRequirements,
) -> anyhow::Result<PreparedOutput<'a>> {
    if requirements.complete_rows {
        view.complete_for_output_with_color(requirements.conditional_styles)?;
    }
    let header = view
        .output_header()
        .unwrap_or_default()
        .into_iter()
        .map(|text| PreparedCell {
            text,
            style: theme.style("table.header"),
        })
        .collect::<Vec<_>>();
    let columns = (0..view.column_count())
        .zip(view.output_column_width_overrides())
        .map(|(column, width_override)| PreparedColumn {
            alignment: view.column_alignment(column),
            width_override,
        })
        .collect();
    Ok(PreparedOutput {
        header_visible: view.header_visible(),
        header,
        rows: Box::new(ViewPreparedRows {
            view,
            theme,
            conditional_styles: requirements.conditional_styles,
        }),
        columns,
        gap: view.column_gap(),
    })
}

struct ViewPreparedRows<'a> {
    view: &'a TableView,
    theme: &'a ResolvedTheme,
    conditional_styles: bool,
}

impl PreparedRows for ViewPreparedRows<'_> {
    fn len(&self) -> usize {
        self.view.row_count()
    }

    fn row(&self, row_index: usize) -> Vec<PreparedCell> {
        self.view
            .rendered_visible_row(row_index)
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(column_index, text)| {
                let mut style = self.theme.style("table.cell");
                if self.conditional_styles {
                    if let Some(conditional) = self
                        .view
                        .output_conditional_color(row_index, column_index)
                        .and_then(|color_ref| self.theme.conditional_style(&color_ref))
                    {
                        style = overlay_style(style, conditional);
                    }
                }
                PreparedCell { text, style }
            })
            .collect()
    }
}

fn normalize_row(row: Vec<PreparedCell>) -> Vec<PreparedCell> {
    row.into_iter()
        .map(|cell| PreparedCell {
            text: normalize_controls(&cell.text),
            style: cell.style,
        })
        .collect()
}

fn resolved_widths(header: &[PreparedCell], prepared: &PreparedOutput<'_>) -> Vec<usize> {
    let mut widths = vec![1; prepared.columns.len()];
    for (index, cell) in header.iter().enumerate().take(widths.len()) {
        widths[index] = widths[index].max(display_width(&cell.text));
    }
    for row_index in 0..prepared.rows.len() {
        let row = prepared.rows.row(row_index);
        for (index, cell) in row.iter().enumerate().take(widths.len()) {
            widths[index] = widths[index].max(display_width(&normalize_controls(&cell.text)));
        }
    }
    for (width, column) in widths.iter_mut().zip(&prepared.columns) {
        if let Some(override_width) = column.width_override {
            *width = override_width.max(1);
        }
    }
    widths
}

fn write_line(
    writer: &mut dyn Write,
    cells: &[PreparedCell],
    columns: &[PreparedColumn],
    widths: &[usize],
    gap: &[u8],
    color: ColorOutput,
    trim_trailing: bool,
) -> io::Result<()> {
    let count = if trim_trailing {
        cells
            .iter()
            .take(columns.len().min(widths.len()))
            .rposition(|cell| !cell.text.trim_end_matches(' ').is_empty())
            .map_or(0, |last| last + 1)
    } else {
        columns.len().min(widths.len())
    };
    for index in 0..count {
        if index > 0 {
            writer.write_all(gap)?;
        }
        let (cell, style) = cells
            .get(index)
            .map(|cell| (cell.text.as_str(), cell.style))
            .unwrap_or(("", Style::default()));
        let is_last = index + 1 == count;
        let text = align_cell(cell, widths[index], columns[index].alignment, is_last);
        let text = if is_last && trim_trailing {
            text.trim_end_matches(' ')
        } else {
            &text
        };
        if color == ColorOutput::Always {
            let ansi = ansi_start(style);
            if !ansi.is_empty() {
                writer.write_all(ansi.as_bytes())?;
                writer.write_all(text.as_bytes())?;
                writer.write_all(b"\x1b[0m")?;
            } else {
                writer.write_all(text.as_bytes())?;
            }
        } else {
            writer.write_all(text.as_bytes())?;
        }
    }
    writer.write_all(b"\n")
}

pub fn normalize_controls(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\n' => normalized.push_str("\\n"),
            '\r' => normalized.push_str("\\r"),
            '\t' => normalized.push_str("\\t"),
            '\u{1b}' => normalized.push_str("\\e"),
            ch if ch.is_control() => normalized.push_str(&format!("\\u{{{:04X}}}", ch as u32)),
            ch => normalized.push(ch),
        }
    }
    normalized
}

fn display_width(value: &str) -> usize {
    value
        .chars()
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum()
}

fn clip_display_width(value: &str, width: usize) -> String {
    let mut clipped = String::new();
    let mut used = 0_usize;
    for ch in value.chars() {
        let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used.saturating_add(char_width) > width {
            break;
        }
        clipped.push(ch);
        used = used.saturating_add(char_width);
    }
    clipped
}

fn align_cell(value: &str, width: usize, alignment: ColumnAlignment, final_column: bool) -> String {
    let clipped = clip_display_width(value, width);
    let padding = width.saturating_sub(display_width(&clipped));
    match alignment {
        ColumnAlignment::Right => format!("{}{}", " ".repeat(padding), clipped),
        ColumnAlignment::Left if !final_column => format!("{}{}", clipped, " ".repeat(padding)),
        ColumnAlignment::Left => clipped,
    }
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

fn ansi_start(style: Style) -> String {
    let mut codes = Vec::<String>::new();
    if let Some(fg) = style.fg {
        codes.push(ansi_color(fg, false));
    }
    if let Some(bg) = style.bg {
        codes.push(ansi_color(bg, true));
    }
    let modifiers = [
        (Modifier::BOLD, "1"),
        (Modifier::DIM, "2"),
        (Modifier::ITALIC, "3"),
        (Modifier::UNDERLINED, "4"),
        (Modifier::SLOW_BLINK, "5"),
        (Modifier::RAPID_BLINK, "6"),
        (Modifier::REVERSED, "7"),
        (Modifier::HIDDEN, "8"),
        (Modifier::CROSSED_OUT, "9"),
    ];
    for (modifier, code) in modifiers {
        if style.add_modifier.contains(modifier) {
            codes.push(code.to_owned());
        }
    }
    if codes.is_empty() {
        String::new()
    } else {
        format!("\x1b[{}m", codes.join(";"))
    }
}

fn ansi_color(color: Color, background: bool) -> String {
    let named = match color {
        Color::Reset => return if background { "49" } else { "39" }.to_owned(),
        Color::Black => 30,
        Color::Red => 31,
        Color::Green => 32,
        Color::Yellow => 33,
        Color::Blue => 34,
        Color::Magenta => 35,
        Color::Cyan => 36,
        Color::Gray => 37,
        Color::DarkGray => 90,
        Color::LightRed => 91,
        Color::LightGreen => 92,
        Color::LightYellow => 93,
        Color::LightBlue => 94,
        Color::LightMagenta => 95,
        Color::LightCyan => 96,
        Color::White => 97,
        Color::Rgb(r, g, b) => {
            return format!("{};2;{r};{g};{b}", if background { 48 } else { 38 })
        }
        Color::Indexed(index) => return format!("{};5;{index}", if background { 48 } else { 38 }),
    };
    (named + if background { 10 } else { 0 }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::SourceAdapter;
    use crate::ops::filter::{FilterKind, FilterMode};
    use crate::table::RowCount;
    use crate::view::Viewport;

    fn rows(values: &[&[&str]]) -> Vec<Vec<String>> {
        values
            .iter()
            .map(|row| row.iter().map(|cell| (*cell).to_owned()).collect())
            .collect()
    }

    fn render(view: &mut TableView, color: ColorOutput) -> String {
        let mut output = Vec::new();
        write_view(
            OutputFormat::Table,
            color,
            view,
            &crate::theme::default_theme(),
            &mut output,
        )
        .expect("render");
        String::from_utf8(output).expect("utf8")
    }

    #[test]
    fn resolves_composable_execution_modes() {
        assert_eq!(
            resolve_execution_mode(false, None, true),
            ExecutionMode::Interactive { emit_on_exit: None }
        );
        assert_eq!(
            resolve_execution_mode(false, None, false),
            ExecutionMode::Batch(OutputFormat::Table)
        );
        assert_eq!(
            resolve_execution_mode(true, None, false),
            ExecutionMode::Interactive { emit_on_exit: None }
        );
        assert_eq!(
            resolve_execution_mode(true, Some(OutputFormat::Table), false),
            ExecutionMode::Interactive {
                emit_on_exit: Some(OutputFormat::Table)
            }
        );
        assert_eq!(
            resolve_execution_mode(false, Some(OutputFormat::Table), true),
            ExecutionMode::Batch(OutputFormat::Table)
        );
    }

    #[test]
    fn normalizes_controls_and_clips_unicode_by_display_width() {
        assert_eq!(
            normalize_controls("a\n\tb\u{1b}\u{7}"),
            "a\\n\\tb\\e\\u{0007}"
        );
        assert_eq!(clip_display_width("a界b", 3), "a界");
        assert_eq!(clip_display_width("e\u{301}x", 1), "e\u{301}");
    }

    #[test]
    fn renders_aligned_plain_rows_without_chrome_or_final_padding() {
        let mut view = TableView::classify(
            rows(&[&["Name", "Count"], &["alpha", "2"], &["b", "10"]]),
            Viewport::new(10, 4),
        );
        assert_eq!(
            render(&mut view, ColorOutput::Never),
            "Name   Count\nalpha      2\nb         10\n"
        );
    }

    #[test]
    fn output_completes_incremental_sources_independent_of_viewport() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("incremental.csv");
        let mut contents = String::from("id,value\n");
        for index in 0..1_000 {
            contents.push_str(&format!("{index},row-{index}\n"));
        }
        std::fs::write(&path, contents).expect("fixture");
        let opened = crate::ingest::DelimitedAdapter
            .open(
                crate::ingest::source::InputSource::Path(path),
                &crate::ingest::OpenOptions {
                    lazy_threshold_bytes: 0,
                    ..crate::ingest::OpenOptions::default()
                },
            )
            .expect("open")
            .into_implicit_table()
            .expect("table");
        let mut view = TableView::from_opened_table(opened, Viewport::new(2, 1)).expect("view");
        assert!(matches!(view.row_count_state(), RowCount::AtLeast(_)));

        let rendered = render(&mut view, ColorOutput::Never);
        assert!(rendered.starts_with(" id  value\n"));
        assert!(rendered.ends_with("999  row-999\n"));
        assert_eq!(rendered.lines().count(), 1_001);
        assert_eq!(view.row_count_state(), RowCount::Exact(1_000));
    }

    #[test]
    fn final_output_uses_live_view_state_but_ignores_cursor_and_viewport() {
        let mut view = TableView::classify(
            rows(&[&["A", "B"], &["one", "2"], &["three", "1"]]),
            Viewport::new(1, 1),
        );
        view.goto(1, 1);
        view.hide_current_column();
        view.goto(0, 0);
        view.sort_current_column(
            crate::ops::sort::SortMode::Lexical,
            crate::ops::sort::SortDirection::Descending,
        );
        assert_eq!(render(&mut view, ColorOutput::Never), "A\nthree\none\n");
    }

    #[test]
    fn hidden_header_and_empty_result_emit_zero_bytes() {
        let mut view = TableView::classify(Vec::new(), Viewport::new(10, 4));
        assert_eq!(render(&mut view, ColorOutput::Never), "");
    }

    #[test]
    fn empty_filtered_result_emits_header_only_or_zero_bytes() {
        let mut view = TableView::classify(rows(&[&["Name"], &["alpha"]]), Viewport::new(10, 4));
        view.apply_filter(0, FilterMode::In, FilterKind::Text, "missing".to_owned())
            .expect("filter");
        assert_eq!(render(&mut view, ColorOutput::Never), "Name\n");
        view.toggle_header();
        assert_eq!(render(&mut view, ColorOutput::Never), "");
    }

    #[test]
    fn controls_are_visible_and_unicode_alignment_is_display_width_aware() {
        let mut view = TableView::classify(
            rows(&[
                &["Text", "Number"],
                &["a\nb\t\u{1b}", "界"],
                &["e\u{301}", "2"],
            ]),
            Viewport::new(10, 4),
        );
        assert_eq!(
            render(&mut view, ColorOutput::Never),
            "Text      Number\na\\nb\\t\\e  界\ne\u{301}         2\n"
        );
    }

    #[test]
    fn explicit_width_clips_and_color_resets_before_gaps_and_newlines() {
        let mut view = TableView::classify(
            rows(&[&["Header", "B"], &["abcdef", "x"]]),
            Viewport::new(10, 4),
        );
        view.set_all_column_widths(3);
        assert_eq!(render(&mut view, ColorOutput::Never), "Hea  B\nabc  x\n");
        let colored = render(&mut view, ColorOutput::Always);
        assert!(colored.contains("\x1b["));
        assert!(colored.contains("\x1b[0m  "));
        assert!(colored.ends_with("\x1b[0m\n"));
    }

    #[test]
    fn explicit_width_pads_narrow_cells_to_match_the_live_view() {
        let mut view = TableView::classify(rows(&[&["A", "B"], &["x", "y"]]), Viewport::new(10, 4));
        view.set_all_column_widths(5);
        assert_eq!(
            render(&mut view, ColorOutput::Never),
            "A      B\nx      y\n"
        );
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn conditional_colors_overlay_cells_without_leaking_ansi() {
        let mut view = TableView::classify(
            rows(&[&["Active", "Name"], &["true", "alpha"]]),
            Viewport::new(10, 4),
        );
        let saved = crate::saved_views::parse_saved_view_yaml(
            r#"
name: colors
filenames: ["*"]
source: {}
view:
  columns:
    Active:
      colors:
        - match:
            true: red
"#,
        )
        .expect("saved view");
        let resolved =
            crate::saved_views::resolve_columns(&saved.view, view.header().expect("header"));
        view.apply_saved_columns(&resolved, None);

        let theme = crate::theme::default_theme();
        let expected_style = overlay_style(
            theme.style("table.cell"),
            theme.conditional_style("red").expect("red"),
        );
        let expected_start = ansi_start(expected_style);
        let rendered = render(&mut view, ColorOutput::Always);
        assert!(
            rendered.contains(&format!("{expected_start}true  \x1b[0m  ")),
            "{rendered:?}"
        );
        assert!(rendered.ends_with("\x1b[0m\n"));
    }

    struct FailingWriter {
        kind: io::ErrorKind,
    }

    impl Write for FailingWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(self.kind))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn broken_pipe_is_clean_but_other_writer_errors_fail() {
        for format in [OutputFormat::Table, OutputFormat::Json, OutputFormat::Jsonl] {
            let mut view = TableView::classify(rows(&[&["A"], &["1"]]), Viewport::new(10, 4));
            write_view(
                format,
                ColorOutput::Never,
                &mut view,
                &crate::theme::default_theme(),
                &mut FailingWriter {
                    kind: io::ErrorKind::BrokenPipe,
                },
            )
            .expect("broken pipe");

            let error = write_view(
                format,
                ColorOutput::Never,
                &mut view,
                &crate::theme::default_theme(),
                &mut FailingWriter {
                    kind: io::ErrorKind::Other,
                },
            )
            .expect_err("other error");
            assert!(error.downcast_ref::<io::Error>().is_some());
        }
    }

    #[test]
    fn ansi_conversion_covers_colors_and_modifiers() {
        let style = Style::default()
            .fg(Color::Rgb(1, 2, 3))
            .bg(Color::Indexed(42))
            .add_modifier(Modifier::BOLD | Modifier::ITALIC | Modifier::UNDERLINED);
        assert_eq!(ansi_start(style), "\x1b[38;2;1;2;3;48;5;42;1;3;4m");
        assert_eq!(ansi_start(Style::default()), "");
    }

    struct FlushWriter {
        kind: Option<io::ErrorKind>,
        flushed: bool,
    }

    impl Write for FlushWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushed = true;
            match self.kind {
                Some(kind) => Err(io::Error::from(kind)),
                None => Ok(()),
            }
        }
    }

    #[test]
    fn output_flushes_and_applies_broken_pipe_policy() {
        let mut successful = FlushWriter {
            kind: None,
            flushed: false,
        };
        flush_output(&mut successful).expect("flush");
        assert!(successful.flushed);

        let mut broken = FlushWriter {
            kind: Some(io::ErrorKind::BrokenPipe),
            flushed: false,
        };
        flush_output(&mut broken).expect("broken pipe flush");
        assert!(broken.flushed);

        let error = flush_output(&mut FlushWriter {
            kind: Some(io::ErrorKind::Other),
            flushed: false,
        })
        .expect_err("other flush error");
        assert!(error.downcast_ref::<io::Error>().is_some());
    }
}
