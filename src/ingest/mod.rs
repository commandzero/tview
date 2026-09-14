mod adapter;
mod delimited;
#[cfg(feature = "elasticsearch")]
mod elasticsearch;
mod json;
mod options;
pub mod source;
#[cfg(feature = "sqlite")]
mod sqlite;
mod streaming_json;

pub use adapter::{
    open_source, FormatResolver, OpenedSource, OpenedTable, ProbeResult, RelationAvailability,
    RelationCatalogEntry, RelationKind, RelationOpener, SourceAdapter,
};
pub use delimited::DelimitedAdapter;
#[cfg(feature = "elasticsearch")]
pub use elasticsearch::{
    ElasticsearchAdapter, ElasticsearchField, ElasticsearchFieldCatalog, ElasticsearchTarget,
    ElasticsearchTargetKind, DEFAULT_ELASTICSEARCH_SOURCE_LIMIT,
};
pub use json::JsonAdapter;
pub use options::{
    resolve_selected_shape, InputFormat, JsonPointer, ObjectMode, ObjectModeOrigin,
    ObjectModeResolution, OpenOptions, ResolvedObjectMode, SchemaScan, SelectedShapeResolution,
    SelectedTableShape, SelectedValueShape, SourceFilterRequest, SourceOptionError,
    SourceOptionOverrides, SourceSortRequest, StructuredPath, DEFAULT_SCHEMA_SCAN_BYTES,
};
#[cfg(feature = "sqlite")]
pub use sqlite::SqliteAdapter;

use std::borrow::Cow;
use std::env;

use csv::ReaderBuilder;
use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8, WINDOWS_1252};

pub const DEFAULT_LAZY_THRESHOLD_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quoting {
    Minimal,
    NonNumeric,
    All,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseOptions {
    pub encoding: Option<String>,
    pub delimiter: Option<u8>,
    pub quoting: Option<Quoting>,
    pub quote_char: u8,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            encoding: None,
            delimiter: None,
            quoting: None,
            quote_char: b'"',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedInput {
    pub text: String,
    pub encoding: String,
}

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("unknown encoding '{0}'")]
    UnknownEncoding(String),
    #[error("input could not be decoded with the compatibility encoding set")]
    DecodeFailed,
    #[error("CSV parse failed: {0}")]
    Csv(#[from] csv::Error),
}

pub fn decode_input(bytes: &[u8], requested: Option<&str>) -> Result<DecodedInput, IngestError> {
    if let Some(label) = requested {
        return decode_with_label(bytes, label);
    }

    for label in compatibility_encoding_labels() {
        if let Ok(decoded) = decode_with_label(bytes, &label) {
            return Ok(decoded);
        }
    }

    Err(IngestError::DecodeFailed)
}

fn compatibility_encoding_labels() -> Vec<String> {
    compatibility_encoding_labels_with_locale(locale_encoding_label())
}

fn compatibility_encoding_labels_with_locale(locale: Option<String>) -> Vec<String> {
    let mut labels = Vec::new();
    for label in ["utf-8", "utf-16"] {
        push_unique_label(&mut labels, label.to_owned());
    }
    if let Some(label) = locale {
        push_unique_label(&mut labels, label);
    }
    for label in ["iso8859-1", "iso8859-2", "cp720", "latin-1"] {
        push_unique_label(&mut labels, label.to_owned());
    }
    labels
}

fn push_unique_label(labels: &mut Vec<String>, label: String) {
    if !labels.iter().any(|existing| existing == &label) {
        labels.push(label);
    }
}

fn locale_encoding_label() -> Option<String> {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .filter_map(|name| env::var(name).ok())
        .find_map(|locale| encoding_label_from_locale(&locale))
}

fn encoding_label_from_locale(locale: &str) -> Option<String> {
    let locale = locale.trim();
    if locale.is_empty() {
        return None;
    }
    let candidate = locale
        .split_once('.')
        .map(|(_, encoding)| encoding)
        .unwrap_or(locale)
        .split('@')
        .next()
        .unwrap_or_default()
        .trim();
    if candidate.is_empty() {
        return None;
    }
    let normalized = normalize_encoding_label(candidate);
    if normalized == "cp720" || encoding_for_label(&normalized).is_some() {
        Some(normalized)
    } else {
        None
    }
}

fn decode_with_label(bytes: &[u8], label: &str) -> Result<DecodedInput, IngestError> {
    let normalized = normalize_encoding_label(label);
    if normalized == "cp720" {
        return Ok(DecodedInput {
            text: decode_cp720(bytes),
            encoding: normalized,
        });
    }
    let Some(encoding) = encoding_for_label(&normalized) else {
        return Err(IngestError::UnknownEncoding(label.to_owned()));
    };
    let (text, had_errors) = decode_with_encoding(bytes, encoding);
    if had_errors {
        return Err(IngestError::DecodeFailed);
    }
    Ok(DecodedInput {
        text: text.into_owned(),
        encoding: normalized,
    })
}

fn normalize_encoding_label(label: &str) -> String {
    label.trim().to_ascii_lowercase().replace('_', "-")
}

fn encoding_for_label(label: &str) -> Option<&'static Encoding> {
    match label {
        "utf-16" => Some(UTF_16LE),
        "latin-1" | "latin1" | "iso8859-1" | "iso-8859-1" => Some(WINDOWS_1252),
        "iso8859-2" | "iso-8859-2" => Encoding::for_label(b"iso-8859-2"),
        _ => Encoding::for_label(label.as_bytes()),
    }
}

fn decode_with_encoding(bytes: &[u8], encoding: &'static Encoding) -> (Cow<'static, str>, bool) {
    if encoding == UTF_8 {
        let (text, had_errors) = UTF_8.decode_without_bom_handling(bytes);
        return (text.into_owned().into(), had_errors);
    }

    if encoding == UTF_16LE && bytes.starts_with(&[0xFE, 0xFF]) {
        let (text, had_errors) = UTF_16BE.decode_without_bom_handling(&bytes[2..]);
        return (text.into_owned().into(), had_errors);
    }

    let (text, _, had_errors) = encoding.decode(bytes);
    (text.into_owned().into(), had_errors)
}

fn decode_cp720(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| cp720_char(*byte)).collect()
}

fn cp720_char(byte: u8) -> char {
    if byte < 0x80 {
        return byte as char;
    }
    CP720_HIGH[(byte - 0x80) as usize]
}

const CP720_HIGH: [char; 128] = [
    '\u{0080}', '\u{0081}', 'é', 'â', '\u{0084}', 'à', '\u{0086}', 'ç', 'ê', 'ë', 'è', 'ï', 'î',
    '\u{008D}', '\u{008E}', '\u{008F}', '\u{0090}', 'ّ', 'ْ', 'ô', '¤', 'ـ', 'û', 'ù', 'ء', 'آ', 'أ',
    'ؤ', '£', 'إ', 'ئ', 'ا', 'ب', 'ة', 'ت', 'ث', 'ج', 'ح', 'خ', 'د', 'ذ', 'ر', 'ز', 'س', 'ش', 'ص',
    '«', '»', '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐', '└',
    '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧', '╨', '╤', '╥', '╙',
    '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀', 'ض', 'ط', 'ظ', 'ع', 'غ', 'ف', 'µ',
    'ق', 'ك', 'ل', 'م', 'ن', 'ه', 'و', 'ى', 'ي', '≡', 'ً', 'ٌ', 'ٍ', 'َ', 'ُ', 'ِ', '≈', '°', '∙', '·',
    '√', 'ⁿ', '²', '■', '\u{00A0}',
];

pub fn parse_rows(bytes: &[u8], options: &ParseOptions) -> Result<Vec<Vec<String>>, IngestError> {
    let decoded = decode_input(bytes, options.encoding.as_deref())?;
    parse_decoded_rows(&decoded.text, options)
}

pub fn parse_decoded_rows(
    text: &str,
    options: &ParseOptions,
) -> Result<Vec<Vec<String>>, IngestError> {
    let delimiter = options
        .delimiter
        .unwrap_or_else(|| sniff_delimiter(text).unwrap_or(b','));
    let normalized;
    let input = if delimiter == b' ' {
        normalized = normalize_space_delimited(text);
        normalized.as_str()
    } else {
        text
    };

    let mut builder = ReaderBuilder::new();
    builder
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .quote(options.quote_char);
    if options.quoting == Some(Quoting::None) {
        builder.quoting(false);
    }

    let mut rows = Vec::new();
    let mut reader = builder.from_reader(input.as_bytes());
    for record in reader.records() {
        rows.push(record?.iter().map(ToOwned::to_owned).collect());
    }
    Ok(pad_rows(rows))
}

pub fn sniff_delimiter(text: &str) -> Option<u8> {
    let candidates = *b",\t;| ";
    let sample: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(20)
        .collect();
    if sample.is_empty() {
        return None;
    }

    candidates
        .into_iter()
        .filter_map(|candidate| score_delimiter(&sample, candidate).map(|score| (candidate, score)))
        .max_by_key(|(_, score)| *score)
        .map(|(candidate, _)| candidate)
}

fn score_delimiter(lines: &[&str], delimiter: u8) -> Option<usize> {
    let counts: Vec<usize> = lines
        .iter()
        .map(|line| count_fields(line, delimiter as char))
        .filter(|count| *count > 1)
        .collect();
    if counts.is_empty() {
        return None;
    }
    let first = counts[0];
    let consistent = counts.iter().filter(|count| **count == first).count();
    Some((consistent * 100) + first)
}

fn count_fields(line: &str, delimiter: char) -> usize {
    let mut fields = 1;
    let mut quote: Option<char> = None;
    let mut previous = '\0';
    for ch in line.chars() {
        if matches!(ch, '"' | '\'') && previous != '\\' {
            quote = if quote == Some(ch) {
                None
            } else if quote.is_none() {
                Some(ch)
            } else {
                quote
            };
        } else if ch == delimiter && quote.is_none() {
            fields += 1;
        }
        previous = ch;
    }
    fields
}

pub fn normalize_space_delimited(text: &str) -> String {
    text.lines()
        .enumerate()
        .map(|(idx, line)| {
            let line = if idx == 0 {
                line.strip_prefix('#')
                    .or_else(|| line.strip_prefix('%'))
                    .unwrap_or(line)
            } else {
                line
            };
            split_shell_like(line).join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn split_shell_like(line: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '"' | '\'' if quote == Some(ch) => quote = None,
            '"' | '\'' if quote.is_none() => quote = Some(ch),
            '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            ch if ch.is_whitespace() && quote.is_none() => {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

pub fn pad_rows(mut rows: Vec<Vec<String>>) -> Vec<Vec<String>> {
    let max_len = rows.iter().map(Vec::len).max().unwrap_or(0);
    for row in &mut rows {
        row.resize(max_len, String::new());
    }
    rows
}

#[cfg(test)]
pub(crate) struct CountingReader<R> {
    inner: R,
    count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
#[cfg(test)]
impl<R> CountingReader<R> {
    pub(crate) fn new(inner: R) -> (Self, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        (
            Self {
                inner,
                count: count.clone(),
            },
            count,
        )
    }
}
#[cfg(test)]
impl<R: std::io::Read> std::io::Read for CountingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(buffer)?;
        self.count
            .fetch_add(count, std::sync::atomic::Ordering::Relaxed);
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(name))
            .expect("fixture bytes")
    }

    #[test]
    fn pads_rows_to_rectangular_shape() {
        let rows = pad_rows(vec![
            vec!["a".to_owned(), "b".to_owned()],
            vec!["c".to_owned()],
        ]);
        assert_eq!(rows[1], vec!["c".to_owned(), String::new()]);
    }

    #[test]
    fn sniffs_common_delimiters() {
        assert_eq!(sniff_delimiter("a,b,c\n1,2,3\n"), Some(b','));
        assert_eq!(sniff_delimiter("a\tb\tc\n1\t2\t3\n"), Some(b'\t'));
    }

    #[test]
    fn normalizes_annotated_space_delimited_input() {
        let normalized = normalize_space_delimited("# A   B \"C D\"\n1   2  3\n");
        assert_eq!(normalized, "A B C D\n1 2 3");
    }

    #[test]
    fn decodes_cp720_label() {
        let decoded = decode_input(&[0x9F, 0xA0, 0xEF], Some("cp720")).expect("cp720");
        assert_eq!(decoded.text, "ابي");
        assert_eq!(decoded.encoding, "cp720");
    }

    #[test]
    fn automatic_decoding_uses_latin1_as_late_fallback() {
        let decoded = decode_input("plain utf8".as_bytes(), None).expect("utf-8");
        assert_eq!(decoded.encoding, "utf-8");
    }

    #[test]
    fn extracts_encoding_label_from_posix_locale() {
        assert_eq!(
            encoding_label_from_locale("en_US.ISO8859-2@euro"),
            Some("iso8859-2".to_owned())
        );
        assert_eq!(
            encoding_label_from_locale("C.UTF-8"),
            Some("utf-8".to_owned())
        );
        assert_eq!(encoding_label_from_locale("C"), None);
    }

    #[test]
    fn compatibility_encoding_set_tries_locale_before_latin1_fallbacks() {
        assert_eq!(
            compatibility_encoding_labels_with_locale(Some("windows-1250".to_owned())),
            vec![
                "utf-8",
                "utf-16",
                "windows-1250",
                "iso8859-1",
                "iso8859-2",
                "cp720",
                "latin-1"
            ]
        );
    }

    #[test]
    fn parses_utf8_sample_rows() {
        let rows = parse_rows(
            &fixture("sample/unicode-example-utf8.txt"),
            &ParseOptions::default(),
        )
        .expect("utf8 sample rows");

        assert_eq!(
            rows.last().expect("last row"),
            &vec![
                "Yugoslavia (Latin)".to_owned(),
                "Djordje Balasevic".to_owned(),
                "Jugoslavija".to_owned(),
                "Đorđe Balašević".to_owned(),
            ]
        );
    }

    #[test]
    fn parses_latin1_sample_with_explicit_encoding() {
        let rows = parse_rows(
            &fixture("sample/test_latin-1.csv"),
            &ParseOptions {
                encoding: Some("latin-1".to_owned()),
                ..ParseOptions::default()
            },
        )
        .expect("latin1 sample rows");
        let row = rows.last().expect("last row");

        assert_eq!(row[0], "ALP");
        assert_eq!(row[1], "B34130005");
        assert_eq!(
            row[2],
            "Ladies' 7 oz. ComfortSoft® Cotton Piqué Polo - WHITE - L"
        );
        assert_eq!(row[8], "L");
        assert_eq!(row[20], "00766369145683");
    }

    #[test]
    fn parses_annotated_space_delimited_sample() {
        let rows = parse_rows(
            &fixture("sample/commented_annotated_numeric.txt"),
            &ParseOptions {
                encoding: Some("utf-8".to_owned()),
                ..ParseOptions::default()
            },
        )
        .expect("space-delimited sample rows");

        assert_eq!(rows.first().expect("header row"), &vec!["A", "B", "C", "D"]);
        assert_eq!(
            rows.last().expect("last row"),
            &vec![
                "-0.000103903949401458218".to_owned(),
                "-0.687995654231882803".to_owned(),
                "+3".to_owned(),
                "+40.9029683683568948".to_owned(),
            ]
        );
    }

    #[test]
    fn parses_windows_newline_sample() {
        let rows = parse_rows(
            &fixture("sample/windows_newlines.csv"),
            &ParseOptions::default(),
        )
        .expect("windows newline sample rows");

        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0], vec!["Col1", "Col2", "Col3"]);
        assert!(rows.iter().skip(1).all(|row| row == &vec!["1", "2", "3"]));
    }
}
