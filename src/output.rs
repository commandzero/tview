use std::io::{self, BufWriter, Write};

use clap::ValueEnum;
use ratatui::style::{Color, Modifier, Style};
use unicode_width::UnicodeWidthChar;

use crate::view::ColumnAlignment;

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
    pub stable_widths: bool,
    pub conditional_styles: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedColumn {
    pub alignment: ColumnAlignment,
    pub width: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedCell {
    pub text: String,
    pub foreground: Option<Color>,
}

/// Frozen, owned presentation. No row source, view, or profiling callback is
/// retained by the serializer.
pub struct PreparedOutput {
    pub header_visible: bool,
    pub header: Vec<PreparedCell>,
    pub rows: Vec<Vec<PreparedCell>>,
    pub columns: Vec<PreparedColumn>,
    pub gap: usize,
    pub header_style: Style,
    pub cell_style: Style,
}

pub trait OutputAdapter {
    fn requirements(&self) -> OutputRequirements;
    fn supports_color(&self) -> bool;
    fn write(
        &self,
        prepared: &PreparedOutput,
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
            stable_widths: true,
            conditional_styles: true,
        }
    }

    fn supports_color(&self) -> bool {
        true
    }

    fn write(
        &self,
        prepared: &PreparedOutput,
        color: ColorOutput,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        let gap = prepared.gap;
        if prepared.header_visible && !prepared.header.is_empty() {
            write_line(
                writer,
                &prepared.header,
                &prepared.columns,
                prepared.header_style,
                gap,
                color,
                self.trim_trailing,
            )?;
        }
        for row in &prepared.rows {
            write_line(
                writer,
                row,
                &prepared.columns,
                prepared.cell_style,
                gap,
                color,
                self.trim_trailing,
            )?;
        }
        Ok(())
    }
}

fn adapter(format: OutputFormat) -> &'static dyn OutputAdapter {
    static TABLE: FixedWidthTableAdapter = FixedWidthTableAdapter {
        trim_trailing: false,
    };
    static JSON: JsonAdapter = JsonAdapter { lines: false };
    static JSONL: JsonAdapter = JsonAdapter { lines: true };
    match format {
        OutputFormat::Table => &TABLE,
        OutputFormat::Json => &JSON,
        OutputFormat::Jsonl => &JSONL,
    }
}

/// Serializes the displayed values without terminal styling or width clipping.
/// Positional cells preserve duplicate labels and headerless input.
struct JsonAdapter {
    lines: bool,
}

/// Serialize frozen presentation strings without copying their contents.
struct PreparedValues<'a>(&'a [PreparedCell]);

impl serde::Serialize for PreparedValues<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for cell in self.0 {
            sequence.serialize_element(&cell.text)?;
        }
        sequence.end()
    }
}

impl OutputAdapter for JsonAdapter {
    fn requirements(&self) -> OutputRequirements {
        OutputRequirements::default()
    }

    fn supports_color(&self) -> bool {
        false
    }

    fn write(
        &self,
        prepared: &PreparedOutput,
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
        for (index, row) in prepared.rows.iter().enumerate() {
            if self.lines {
                #[derive(serde::Serialize)]
                struct Record<'a> {
                    columns: &'a [&'a str],
                    values: PreparedValues<'a>,
                }
                serde_json::to_writer(
                    &mut *writer,
                    &Record {
                        columns: &columns,
                        values: PreparedValues(row),
                    },
                )?;
                writer.write_all(b"\n")?;
            } else {
                if index != 0 {
                    writer.write_all(b",")?;
                }
                serde_json::to_writer(&mut *writer, &PreparedValues(row))?;
            }
        }
        if !self.lines {
            writer.write_all(b"]}\n")?;
        }
        Ok(())
    }
}

/// Reject unsupported adapter capabilities before opening a source.
pub fn requirements(
    format: OutputFormat,
    color: ColorOutput,
) -> anyhow::Result<OutputRequirements> {
    let selected = adapter(format);
    if color == ColorOutput::Always && !selected.supports_color() {
        anyhow::bail!("output format does not support --color always");
    }
    let mut requirements = selected.requirements();
    requirements.conditional_styles &= color == ColorOutput::Always;
    Ok(requirements)
}

/// Serializers receive only the owned presentation supplied by projection.
pub fn write_prepared(
    format: OutputFormat,
    color: ColorOutput,
    prepared: &PreparedOutput,
    preview_remaining: Option<Option<crate::table::RowCount>>,
    writer: &mut dyn Write,
) -> anyhow::Result<()> {
    let result = (|| -> io::Result<()> {
        if let Some(remaining) = preview_remaining {
            FixedWidthTableAdapter {
                trim_trailing: true,
            }
            .write(prepared, color, writer)?;
            match remaining {
                Some(crate::table::RowCount::Exact(count)) => {
                    writeln!(writer, "{count} more rows...")
                }
                Some(_) => writeln!(writer, "more rows..."),
                None => Ok(()),
            }
        } else {
            adapter(format).write(prepared, color, writer)
        }
    })();
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub fn write_prepared_to_stdout(
    format: OutputFormat,
    color: ColorOutput,
    prepared: &PreparedOutput,
    preview_remaining: Option<Option<crate::table::RowCount>>,
) -> anyhow::Result<()> {
    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());
    write_prepared(format, color, prepared, preview_remaining, &mut writer)?;
    flush_output(&mut writer)
}

fn flush_output(writer: &mut dyn Write) -> anyhow::Result<()> {
    match writer.flush() {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Table escaping is frozen with the selected presentation, not recomputed by
/// the serializer. Ordinary cells reuse their owned string without copying.
pub(crate) fn freeze_table_cell(text: String) -> String {
    if text.chars().any(char::is_control) {
        normalize_controls(&text)
    } else {
        text
    }
}

fn write_line(
    writer: &mut dyn Write,
    cells: &[PreparedCell],
    columns: &[PreparedColumn],
    base_style: Style,
    gap: usize,
    color: ColorOutput,
    trim_trailing: bool,
) -> io::Result<()> {
    let count = if trim_trailing {
        cells
            .iter()
            .take(columns.len())
            .rposition(|cell| !cell.text.trim_end_matches(' ').is_empty())
            .map_or(0, |last| last + 1)
    } else {
        columns.len()
    };
    for (index, column) in columns.iter().enumerate().take(count) {
        if index > 0 {
            write_spaces(writer, gap)?;
        }
        let (cell, style) = cells
            .get(index)
            .map(|cell| {
                (
                    cell.text.as_str(),
                    cell.foreground.map_or(base_style, |fg| base_style.fg(fg)),
                )
            })
            .unwrap_or(("", Style::default()));
        let is_last = index + 1 == count;
        let text = align_cell(cell, column.width, column.alignment, is_last);
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

fn write_spaces(writer: &mut dyn Write, mut count: usize) -> io::Result<()> {
    const SPACES: [u8; 64] = [b' '; 64];
    while count > 0 {
        let chunk = count.min(SPACES.len());
        writer.write_all(&SPACES[..chunk])?;
        count -= chunk;
    }
    Ok(())
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

pub(crate) fn display_width(value: &str) -> usize {
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

    fn frozen() -> PreparedOutput {
        PreparedOutput {
            header_visible: true,
            header: vec![PreparedCell {
                text: "Value".to_owned(),
                foreground: None,
            }],
            rows: vec![vec![PreparedCell {
                text: "a\nb".to_owned(),
                foreground: None,
            }]],
            columns: vec![PreparedColumn {
                alignment: ColumnAlignment::Left,
                width: 6,
            }],
            gap: 2,
            header_style: Style::default(),
            cell_style: Style::default(),
        }
    }

    #[test]
    fn complete_formats_serialize_frozen_strings_without_source_access() {
        let prepared = frozen();
        let mut first = Vec::new();
        let mut second = Vec::new();
        write_prepared(
            OutputFormat::Json,
            ColorOutput::Never,
            &prepared,
            None,
            &mut first,
        )
        .unwrap();
        write_prepared(
            OutputFormat::Json,
            ColorOutput::Never,
            &prepared,
            None,
            &mut second,
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&first).unwrap(),
            serde_json::json!({"columns": ["Value"], "rows": [["a\nb"]]}),
        );
    }

    #[test]
    fn colored_cells_preserve_base_attributes_and_reset_before_gaps_and_newlines() {
        let prepared = PreparedOutput {
            header_visible: false,
            header: Vec::new(),
            rows: vec![vec![
                PreparedCell {
                    text: "宽abc".to_owned(),
                    foreground: Some(Color::Rgb(255, 0, 0)),
                },
                PreparedCell {
                    text: "z".to_owned(),
                    foreground: None,
                },
            ]],
            columns: vec![
                PreparedColumn {
                    alignment: ColumnAlignment::Left,
                    width: 4,
                },
                PreparedColumn {
                    alignment: ColumnAlignment::Right,
                    width: 3,
                },
            ],
            gap: 2,
            header_style: Style::default(),
            cell_style: Style::default()
                .fg(Color::Green)
                .bg(Color::Indexed(25))
                .add_modifier(Modifier::BOLD | Modifier::ITALIC | Modifier::UNDERLINED),
        };
        let mut colored = Vec::new();
        write_prepared(
            OutputFormat::Table,
            ColorOutput::Always,
            &prepared,
            None,
            &mut colored,
        )
        .unwrap();
        assert_eq!(
            colored,
            "\x1b[38;2;255;0;0;48;5;25;1;3;4m宽ab\x1b[0m  \x1b[32;48;5;25;1;3;4m  z\x1b[0m\n"
                .as_bytes()
        );
        let mut plain = Vec::new();
        write_prepared(
            OutputFormat::Table,
            ColorOutput::Never,
            &prepared,
            None,
            &mut plain,
        )
        .unwrap();
        assert_eq!(plain, "宽ab    z\n".as_bytes());
    }

    struct FailingWriter(io::ErrorKind);
    impl Write for FailingWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(self.0))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::from(self.0))
        }
    }

    #[test]
    fn broken_pipe_is_clean_but_other_errors_propagate() {
        for format in [OutputFormat::Table, OutputFormat::Json, OutputFormat::Jsonl] {
            write_prepared(
                format,
                ColorOutput::Never,
                &frozen(),
                None,
                &mut FailingWriter(io::ErrorKind::BrokenPipe),
            )
            .unwrap();
            assert!(write_prepared(
                format,
                ColorOutput::Never,
                &frozen(),
                None,
                &mut FailingWriter(io::ErrorKind::Other)
            )
            .is_err());
        }
        flush_output(&mut FailingWriter(io::ErrorKind::BrokenPipe)).unwrap();
        assert!(flush_output(&mut FailingWriter(io::ErrorKind::Other)).is_err());
    }
}
