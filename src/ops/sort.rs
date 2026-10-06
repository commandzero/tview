use std::cmp::Ordering;
use std::collections::HashSet;

pub(crate) const NUMERIC_PROFILE_SAMPLE_ROWS: usize = 8_192;
const NUMERIC_PROFILE_MAX_SUFFIXES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Lexical,
    Natural,
    Numeric,
    #[cfg(feature = "saved-views")]
    Date,
    #[cfg(feature = "saved-views")]
    SemVer,
    #[cfg(feature = "saved-views")]
    Ip,
    #[cfg(feature = "saved-views")]
    Boolean,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

pub fn sort_rows(
    rows: &mut [Vec<String>],
    column: usize,
    mode: SortMode,
    direction: SortDirection,
) {
    let numeric_profile =
        (mode == SortMode::Numeric).then(|| infer_numeric_column_profile(None, rows, column));
    rows.sort_by(|left, right| {
        let ordering = compare_cells(
            left.get(column).map(String::as_str).unwrap_or_default(),
            right.get(column).map(String::as_str).unwrap_or_default(),
            mode,
            numeric_profile.unwrap_or_default(),
        );
        match direction {
            SortDirection::Ascending => ordering,
            SortDirection::Descending => ordering.reverse(),
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SortSpec {
    pub(crate) column: usize,
    pub(crate) mode: SortMode,
    pub(crate) direction: SortDirection,
    pub(crate) numeric_profile: NumericColumnProfile,
}

pub(crate) fn sort_rows_by_specs(rows: &mut [Vec<String>], specs: &[SortSpec]) {
    if specs.is_empty() {
        return;
    }

    rows.sort_by(|left, right| {
        for spec in specs {
            let ordering = compare_cells(
                left.get(spec.column)
                    .map(String::as_str)
                    .unwrap_or_default(),
                right
                    .get(spec.column)
                    .map(String::as_str)
                    .unwrap_or_default(),
                spec.mode,
                spec.numeric_profile,
            );
            let ordering = match spec.direction {
                SortDirection::Ascending => ordering,
                SortDirection::Descending => ordering.reverse(),
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        Ordering::Equal
    });
}

pub(crate) fn compare_cells(
    left: &str,
    right: &str,
    mode: SortMode,
    numeric_profile: NumericColumnProfile,
) -> Ordering {
    match mode {
        SortMode::Lexical => left.cmp(right),
        SortMode::Natural => natural_tokens(left).cmp(&natural_tokens(right)),
        SortMode::Numeric => compare_numeric_cells(left, right, numeric_profile),
        #[cfg(feature = "saved-views")]
        SortMode::Date => compare_optional_keys(parse_iso8601_key(left), parse_iso8601_key(right)),
        #[cfg(feature = "saved-views")]
        SortMode::SemVer => compare_optional_keys(parse_semver_key(left), parse_semver_key(right)),
        #[cfg(feature = "saved-views")]
        SortMode::Ip => compare_optional_keys(parse_ip_key(left), parse_ip_key(right)),
        #[cfg(feature = "saved-views")]
        SortMode::Boolean => compare_optional_keys(parse_bool_key(left), parse_bool_key(right)),
    }
}

#[cfg(feature = "saved-views")]
fn compare_optional_keys<T: Ord>(left: Option<T>, right: Option<T>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(feature = "saved-views")]
fn parse_iso8601_key(value: &str) -> Option<i64> {
    let value = value.trim();
    let (date, rest) = value.split_once(['T', ' ']).unwrap_or((value, ""));
    let mut date_parts = date.split('-');
    let year = date_parts.next()?.parse::<i32>().ok()?;
    let month = date_parts.next()?.parse::<u32>().ok()?;
    let day = date_parts.next()?.parse::<u32>().ok()?;
    if date_parts.next().is_some() {
        return None;
    }

    let (hour, minute, second, offset_seconds) = if rest.is_empty() {
        (0, 0, 0, 0)
    } else {
        parse_iso8601_time(rest)?
    };

    let days = days_from_civil(year, month, day)?;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second - offset_seconds)
}

#[cfg(feature = "saved-views")]
fn parse_iso8601_time(value: &str) -> Option<(i64, i64, i64, i64)> {
    let (time, offset_seconds) = if let Some(time) = value.strip_suffix('Z') {
        (time, 0)
    } else if let Some((time, offset)) = split_timezone_offset(value) {
        (time, parse_timezone_offset(offset)?)
    } else {
        (value, 0)
    };
    let mut parts = time.split(':');
    let hour = parts.next()?.parse::<i64>().ok()?;
    let minute = parts.next().unwrap_or("0").parse::<i64>().ok()?;
    let second = parts
        .next()
        .unwrap_or("0")
        .split('.')
        .next()
        .unwrap_or("0")
        .parse::<i64>()
        .ok()?;
    Some((hour, minute, second, offset_seconds))
}

#[cfg(feature = "saved-views")]
fn split_timezone_offset(value: &str) -> Option<(&str, &str)> {
    let offset_idx = value
        .char_indices()
        .skip(1)
        .find_map(|(idx, ch)| matches!(ch, '+' | '-').then_some(idx))?;
    Some(value.split_at(offset_idx))
}

#[cfg(feature = "saved-views")]
fn parse_timezone_offset(value: &str) -> Option<i64> {
    let sign = if value.starts_with('-') { -1 } else { 1 };
    let value = value.trim_start_matches(['+', '-']);
    let mut parts = value.split(':');
    let hours = parts.next()?.parse::<i64>().ok()?;
    let minutes = parts.next().unwrap_or("0").parse::<i64>().ok()?;
    Some(sign * (hours * 3_600 + minutes * 60))
}

#[cfg(feature = "saved-views")]
fn days_from_civil(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) as i64)
}

#[cfg(feature = "saved-views")]
fn parse_semver_key(value: &str) -> Option<semver::Version> {
    let value = value.trim().trim_start_matches('v');
    semver::Version::parse(value)
        .or_else(|_| semver::Version::parse(&format!("{value}.0")))
        .or_else(|_| semver::Version::parse(&format!("{value}.0.0")))
        .ok()
}

#[cfg(feature = "saved-views")]
fn parse_ip_key(value: &str) -> Option<[u8; 16]> {
    match value.trim().parse::<std::net::IpAddr>().ok()? {
        std::net::IpAddr::V4(addr) => {
            let mut bytes = [0; 16];
            bytes[10] = 0xff;
            bytes[11] = 0xff;
            bytes[12..].copy_from_slice(&addr.octets());
            Some(bytes)
        }
        std::net::IpAddr::V6(addr) => Some(addr.octets()),
    }
}

pub(crate) fn parse_bool_key(value: &str) -> Option<bool> {
    let value = value.trim();
    if ["true", "yes", "y", "1"]
        .iter()
        .any(|case| value.eq_ignore_ascii_case(case))
    {
        Some(true)
    } else if ["false", "no", "n", "0"]
        .iter()
        .any(|case| value.eq_ignore_ascii_case(case))
    {
        Some(false)
    } else {
        None
    }
}

pub(crate) fn compare_numeric_cells(
    left: &str,
    right: &str,
    profile: NumericColumnProfile,
) -> Ordering {
    match (
        parse_numeric_key(left, profile),
        parse_numeric_key(right, profile),
    ) {
        (Some(NumericKey::MultiPart(left)), Some(NumericKey::MultiPart(right))) => {
            compare_numeric_parts(&left, &right)
        }
        (Some(NumericKey::Scalar(left)), Some(NumericKey::Scalar(right))) => left.total_cmp(&right),
        (Some(NumericKey::MultiPart(left)), Some(NumericKey::Scalar(right))) => {
            compare_parts_to_scalar(&left, right)
        }
        (Some(NumericKey::Scalar(left)), Some(NumericKey::MultiPart(right))) => {
            compare_parts_to_scalar(&right, left).reverse()
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[derive(Debug, Clone, PartialEq)]
enum NumericKey {
    Scalar(f64),
    MultiPart(Vec<u64>),
}

fn parse_numeric_key(value: &str, profile: NumericColumnProfile) -> Option<NumericKey> {
    if is_numeric_placeholder(value) {
        return None;
    }

    parse_multi_dot_number(value)
        .map(NumericKey::MultiPart)
        .or_else(|| parse_suffixed_number_with_profile(value, profile).map(NumericKey::Scalar))
}

pub(crate) fn is_numeric_cell(value: &str, profile: NumericColumnProfile) -> bool {
    parse_numeric_key(value, profile).is_some()
}

pub(crate) fn parse_numeric_scalar(value: &str, profile: NumericColumnProfile) -> Option<f64> {
    if is_numeric_placeholder(value) {
        return None;
    }
    parse_suffixed_number_with_profile(value, profile)
}

pub(crate) fn is_numeric_placeholder(value: &str) -> bool {
    let value = value.trim();
    ["null", "n/a", "na", "none", "nil", "nan"]
        .iter()
        .any(|placeholder| value.eq_ignore_ascii_case(placeholder))
}

fn parse_multi_dot_number(value: &str) -> Option<Vec<u64>> {
    let value = value.trim();
    if value.matches('.').count() < 2 {
        return None;
    }

    value
        .split('.')
        .map(|part| {
            if part.is_empty() || !part.chars().all(|ch| ch.is_ascii_digit()) {
                return None;
            }
            part.parse::<u64>().ok()
        })
        .collect()
}

fn compare_numeric_parts(left: &[u64], right: &[u64]) -> Ordering {
    for idx in 0..left.len().max(right.len()) {
        let left = left.get(idx).copied().unwrap_or(0);
        let right = right.get(idx).copied().unwrap_or(0);
        match left.cmp(&right) {
            Ordering::Equal => {}
            ordering => return ordering,
        }
    }
    Ordering::Equal
}

fn compare_parts_to_scalar(parts: &[u64], scalar: f64) -> Ordering {
    if scalar.is_finite() && scalar >= 0.0 && scalar.fract() == 0.0 && scalar <= u64::MAX as f64 {
        return compare_numeric_parts(parts, &[scalar as u64]);
    }

    let first_part = parts.first().copied().unwrap_or(0) as f64;
    first_part.total_cmp(&scalar)
}

#[cfg(test)]
fn parse_suffixed_number(value: &str) -> Option<f64> {
    parse_suffixed_number_with_profile(value, NumericColumnProfile::default())
}

pub(crate) fn parse_suffixed_number_with_profile(
    value: &str,
    profile: NumericColumnProfile,
) -> Option<f64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(number) = value.parse::<f64>() {
        return Some(number);
    }

    value
        .char_indices()
        .map(|(idx, _)| idx)
        .chain(std::iter::once(value.len()))
        .filter(|idx| *idx > 0)
        .rev()
        .find_map(|idx| {
            let (number, suffix) = value.split_at(idx);
            let number = number.trim_end().parse::<f64>().ok()?;
            let multiplier = suffix_multiplier(suffix.trim_start(), profile)?;
            Some(number * multiplier)
        })
}

fn suffix_multiplier(suffix: &str, profile: NumericColumnProfile) -> Option<f64> {
    percent_suffix_multiplier(suffix)
        .or_else(|| byte_suffix_multiplier(suffix))
        .or_else(|| time_suffix_multiplier(suffix, profile))
        .or_else(|| scientific_suffix_multiplier(suffix, profile))
}

fn percent_suffix_multiplier(suffix: &str) -> Option<f64> {
    (suffix == "%").then_some(1.0)
}

fn byte_suffix_multiplier(suffix: &str) -> Option<f64> {
    const BYTE_SUFFIXES: &[(&[&str], f64)] = &[
        (&["b", "byte", "bytes"], 1.0),
        (&["kb", "kilobyte", "kilobytes"], 1_000.0),
        (&["mb", "megabyte", "megabytes"], 1_000_000.0),
        (&["gb", "gigabyte", "gigabytes"], 1_000_000_000.0),
        (&["tb", "terabyte", "terabytes"], 1_000_000_000_000.0),
        (&["pb", "petabyte", "petabytes"], 1_000_000_000_000_000.0),
        (&["eb", "exabyte", "exabytes"], 1_000_000_000_000_000_000.0),
        (&["kib", "kibibyte", "kibibytes"], 1024.0),
        (&["mib", "mebibyte", "mebibytes"], 1_048_576.0),
        (&["gib", "gibibyte", "gibibytes"], 1_073_741_824.0),
        (&["tib", "tebibyte", "tebibytes"], 1_099_511_627_776.0),
        (&["pib", "pebibyte", "pebibytes"], 1_125_899_906_842_624.0),
        (
            &["eib", "exbibyte", "exbibytes"],
            1_152_921_504_606_846_976.0,
        ),
    ];
    BYTE_SUFFIXES.iter().find_map(|(names, multiplier)| {
        names
            .iter()
            .any(|name| suffix.eq_ignore_ascii_case(name))
            .then_some(*multiplier)
    })
}

fn scientific_suffix_multiplier(suffix: &str, profile: NumericColumnProfile) -> Option<f64> {
    match suffix {
        "n" => Some(0.000_000_001),
        "u" | "mu" | "µ" | "μ" => Some(0.000_001),
        "m" if !profile.bare_m_is_minutes() => Some(0.001),
        "k" => Some(1_000.0),
        "M" => Some(1_000_000.0),
        "g" | "G" => Some(1_000_000_000.0),
        "T" => Some(1_000_000_000_000.0),
        "P" => Some(1_000_000_000_000_000.0),
        "E" => Some(1_000_000_000_000_000_000.0),
        _ => None,
    }
}

fn time_suffix_multiplier(suffix: &str, profile: NumericColumnProfile) -> Option<f64> {
    let is = |name: &str| suffix.eq_ignore_ascii_case(name);
    if is("ns") || is("nanosecond") || is("nanoseconds") {
        Some(0.000_000_001)
    } else if ["us", "µs", "μs", "mus", "microsecond", "microseconds"]
        .iter()
        .any(|name| is(name))
    {
        Some(0.000_001)
    } else if is("ms") || is("millisecond") || is("milliseconds") {
        Some(0.001)
    } else if ["s", "sec", "secs", "second", "seconds"]
        .iter()
        .any(|name| is(name))
    {
        Some(1.0)
    } else if (is("m") && profile.bare_m_is_minutes())
        || ["min", "mins", "minute", "minutes"]
            .iter()
            .any(|name| is(name))
    {
        Some(60.0)
    } else if ["h", "hr", "hrs", "hour", "hours"]
        .iter()
        .any(|name| is(name))
    {
        Some(60.0 * 60.0)
    } else if ["d", "day", "days"].iter().any(|name| is(name)) {
        Some(60.0 * 60.0 * 24.0)
    } else if ["y", "yr", "yrs", "year", "years"]
        .iter()
        .any(|name| is(name))
    {
        Some(60.0 * 60.0 * 24.0 * 365.25)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct NumericColumnProfile {
    kind: NumericColumnKind,
}

impl NumericColumnProfile {
    pub(crate) fn time() -> Self {
        Self {
            kind: NumericColumnKind::Time,
        }
    }

    pub(crate) fn is_time(self) -> bool {
        self.kind == NumericColumnKind::Time
    }

    fn bare_m_is_minutes(self) -> bool {
        self.kind == NumericColumnKind::Time
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum NumericColumnKind {
    #[default]
    Default,
    Time,
}

pub(crate) fn infer_numeric_column_profile(
    header: Option<&str>,
    rows: &[Vec<String>],
    column: usize,
) -> NumericColumnProfile {
    infer_numeric_profile_from_values(
        header,
        rows.iter()
            .map(|row| row.get(column).map_or("", String::as_str)),
    )
}

pub(crate) fn infer_numeric_profile_from_values<'a>(
    header: Option<&str>,
    values: impl Iterator<Item = &'a str>,
) -> NumericColumnProfile {
    if header.is_some_and(header_suggests_time) {
        return NumericColumnProfile::time();
    }

    let mut evidence = SuffixEvidence::default();
    for cell in values.take(NUMERIC_PROFILE_SAMPLE_ROWS).map(str::trim) {
        if cell.is_empty() {
            continue;
        }
        if let Some(suffix) = numeric_suffix(cell) {
            evidence.record(suffix);
        }
        if evidence.is_confident() {
            break;
        }
    }

    if evidence.has_time {
        NumericColumnProfile::time()
    } else {
        NumericColumnProfile::default()
    }
}

#[derive(Debug, Default)]
struct SuffixEvidence {
    suffixes: HashSet<String>,
    has_time: bool,
}

impl SuffixEvidence {
    fn record(&mut self, suffix: &str) {
        if suffix.is_empty() {
            return;
        }
        self.suffixes.insert(normalize_suffix(suffix));
        if is_time_context_suffix(suffix) {
            self.has_time = true;
        }
    }

    fn is_confident(&self) -> bool {
        self.has_time || self.suffixes.len() >= NUMERIC_PROFILE_MAX_SUFFIXES
    }
}

fn numeric_suffix(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() || value.parse::<f64>().is_ok() {
        return None;
    }

    value
        .char_indices()
        .map(|(idx, _)| idx)
        .chain(std::iter::once(value.len()))
        .filter(|idx| *idx > 0)
        .rev()
        .find_map(|idx| {
            let (number, suffix) = value.split_at(idx);
            number
                .trim_end()
                .parse::<f64>()
                .ok()
                .map(|_| suffix.trim_start())
        })
}

fn normalize_suffix(suffix: &str) -> String {
    suffix.trim().to_ascii_lowercase()
}

fn is_time_context_suffix(suffix: &str) -> bool {
    let suffix = suffix.to_ascii_lowercase();
    matches!(
        suffix.as_str(),
        "ns" | "nanosecond"
            | "nanoseconds"
            | "us"
            | "µs"
            | "μs"
            | "mus"
            | "microsecond"
            | "microseconds"
            | "ms"
            | "millisecond"
            | "milliseconds"
            | "s"
            | "sec"
            | "secs"
            | "second"
            | "seconds"
            | "min"
            | "mins"
            | "minute"
            | "minutes"
            | "h"
            | "hr"
            | "hrs"
            | "hour"
            | "hours"
            | "d"
            | "day"
            | "days"
            | "y"
            | "yr"
            | "yrs"
            | "year"
            | "years"
    )
}

fn header_suggests_time(header: &str) -> bool {
    let header = header.to_ascii_lowercase();
    let tokens = header
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != 'µ' && ch != 'μ')
        .filter(|token| !token.is_empty());

    tokens.clone().any(is_time_header_token)
        || [
            "duration", "latency", "elapsed", "runtime", "uptime", "timeout", "interval",
        ]
        .iter()
        .any(|hint| header.contains(hint))
}

fn is_time_header_token(token: &str) -> bool {
    matches!(
        token,
        "ns" | "nanosecond"
            | "nanoseconds"
            | "us"
            | "µs"
            | "μs"
            | "microsecond"
            | "microseconds"
            | "ms"
            | "millisecond"
            | "milliseconds"
            | "s"
            | "sec"
            | "secs"
            | "second"
            | "seconds"
            | "min"
            | "mins"
            | "minute"
            | "minutes"
            | "h"
            | "hr"
            | "hrs"
            | "hour"
            | "hours"
            | "d"
            | "day"
            | "days"
            | "y"
            | "yr"
            | "yrs"
            | "year"
            | "years"
    )
}

fn natural_tokens(value: &str) -> Vec<NaturalToken> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut current_is_digit = None;

    for ch in value.chars() {
        let is_digit = ch.is_ascii_digit();
        if current_is_digit == Some(is_digit) || current_is_digit.is_none() {
            current.push(ch);
            current_is_digit = Some(is_digit);
        } else {
            tokens.push(NaturalToken::from_part(
                &current,
                current_is_digit.unwrap_or(false),
            ));
            current.clear();
            current.push(ch);
            current_is_digit = Some(is_digit);
        }
    }

    if !current.is_empty() {
        tokens.push(NaturalToken::from_part(
            &current,
            current_is_digit.unwrap_or(false),
        ));
    }
    tokens
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum NaturalToken {
    Text(String),
    Number(u64),
}

impl NaturalToken {
    fn from_part(part: &str, is_digit: bool) -> Self {
        if is_digit {
            Self::Number(part.parse().unwrap_or(0))
        } else {
            Self::Text(part.to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_lexically() {
        let mut rows = vec![vec!["b".to_owned()], vec!["a".to_owned()]];
        sort_rows(&mut rows, 0, SortMode::Lexical, SortDirection::Ascending);
        assert_eq!(rows, vec![vec!["a".to_owned()], vec!["b".to_owned()]]);
    }

    #[test]
    fn sorts_naturally() {
        let mut rows = vec![vec!["item10".to_owned()], vec!["item2".to_owned()]];
        sort_rows(&mut rows, 0, SortMode::Natural, SortDirection::Ascending);
        assert_eq!(
            rows,
            vec![vec!["item2".to_owned()], vec!["item10".to_owned()]]
        );
    }

    #[test]
    fn sorts_numerically() {
        let mut rows = vec![vec!["10".to_owned()], vec!["2".to_owned()]];
        sort_rows(&mut rows, 0, SortMode::Numeric, SortDirection::Ascending);
        assert_eq!(rows, vec![vec!["2".to_owned()], vec!["10".to_owned()]]);
    }

    #[test]
    fn sorts_ip_like_values_by_numeric_components() {
        let mut rows = vec![
            vec!["10.0.0.10".to_owned()],
            vec!["10.0.0.2".to_owned()],
            vec!["192.168.0.1".to_owned()],
            vec!["10.0.0.1".to_owned()],
        ];
        sort_rows(&mut rows, 0, SortMode::Numeric, SortDirection::Ascending);
        assert_eq!(
            rows,
            vec![
                vec!["10.0.0.1".to_owned()],
                vec!["10.0.0.2".to_owned()],
                vec!["10.0.0.10".to_owned()],
                vec!["192.168.0.1".to_owned()],
            ]
        );
    }

    #[test]
    fn sorts_semver_like_values_by_numeric_components() {
        let mut rows = vec![
            vec!["1.10.0".to_owned()],
            vec!["1.2.10".to_owned()],
            vec!["1.2.3".to_owned()],
            vec!["2.0.0".to_owned()],
            vec!["1.2.0".to_owned()],
        ];
        sort_rows(&mut rows, 0, SortMode::Numeric, SortDirection::Ascending);
        assert_eq!(
            rows,
            vec![
                vec!["1.2.0".to_owned()],
                vec!["1.2.3".to_owned()],
                vec!["1.2.10".to_owned()],
                vec!["1.10.0".to_owned()],
                vec!["2.0.0".to_owned()],
            ]
        );
    }

    #[test]
    fn keeps_single_dot_values_as_decimals() {
        let mut rows = vec![
            vec!["1.10".to_owned()],
            vec!["1.2".to_owned()],
            vec!["1.02".to_owned()],
        ];
        sort_rows(&mut rows, 0, SortMode::Numeric, SortDirection::Ascending);
        assert_eq!(
            rows,
            vec![
                vec!["1.02".to_owned()],
                vec!["1.10".to_owned()],
                vec!["1.2".to_owned()],
            ]
        );
    }

    #[test]
    fn sorts_numerically_with_scientific_suffixes() {
        let mut rows = vec![
            vec!["2M".to_owned()],
            vec!["3k".to_owned()],
            vec!["2u".to_owned()],
            vec!["2m".to_owned()],
            vec!["1g".to_owned()],
            vec!["1".to_owned()],
        ];
        sort_rows(&mut rows, 0, SortMode::Numeric, SortDirection::Ascending);
        assert_eq!(
            rows,
            vec![
                vec!["2u".to_owned()],
                vec!["2m".to_owned()],
                vec!["1".to_owned()],
                vec!["3k".to_owned()],
                vec!["2M".to_owned()],
                vec!["1g".to_owned()],
            ]
        );
    }

    #[test]
    fn sorts_numerically_with_byte_suffixes() {
        let mut rows = vec![
            vec!["1GiB".to_owned()],
            vec!["1GB".to_owned()],
            vec!["2mb".to_owned()],
            vec!["512KiB".to_owned()],
            vec!["512kb".to_owned()],
            vec!["1MiB".to_owned()],
            vec!["1mb".to_owned()],
        ];
        sort_rows(&mut rows, 0, SortMode::Numeric, SortDirection::Ascending);
        assert_eq!(
            rows,
            vec![
                vec!["512kb".to_owned()],
                vec!["512KiB".to_owned()],
                vec!["1mb".to_owned()],
                vec!["1MiB".to_owned()],
                vec!["2mb".to_owned()],
                vec!["1GB".to_owned()],
                vec!["1GiB".to_owned()],
            ]
        );
    }

    #[test]
    fn byte_suffix_takes_precedence_over_single_letter_suffix() {
        assert_eq!(parse_suffixed_number("1mb"), Some(1_000_000.0));
        assert_eq!(parse_suffixed_number("1m"), Some(0.001));
    }

    #[test]
    fn sorts_numerically_with_expanded_scientific_suffixes() {
        let mut rows = vec![
            vec!["1E".to_owned()],
            vec!["1T".to_owned()],
            vec!["1mu".to_owned()],
            vec!["1P".to_owned()],
            vec!["1n".to_owned()],
            vec!["1µ".to_owned()],
            vec!["1M".to_owned()],
            vec!["1m".to_owned()],
            vec!["1k".to_owned()],
            vec!["1G".to_owned()],
        ];
        sort_rows(&mut rows, 0, SortMode::Numeric, SortDirection::Ascending);
        assert_eq!(
            rows,
            vec![
                vec!["1n".to_owned()],
                vec!["1mu".to_owned()],
                vec!["1µ".to_owned()],
                vec!["1m".to_owned()],
                vec!["1k".to_owned()],
                vec!["1M".to_owned()],
                vec!["1G".to_owned()],
                vec!["1T".to_owned()],
                vec!["1P".to_owned()],
                vec!["1E".to_owned()],
            ]
        );
    }

    #[test]
    fn sorts_numerically_with_time_suffixes() {
        let mut rows = vec![
            vec!["1year".to_owned()],
            vec!["1day".to_owned()],
            vec!["1h".to_owned()],
            vec!["1min".to_owned()],
            vec!["1s".to_owned()],
            vec!["1ms".to_owned()],
            vec!["1us".to_owned()],
            vec!["1ns".to_owned()],
        ];
        sort_rows(&mut rows, 0, SortMode::Numeric, SortDirection::Ascending);
        assert_eq!(
            rows,
            vec![
                vec!["1ns".to_owned()],
                vec!["1us".to_owned()],
                vec!["1ms".to_owned()],
                vec!["1s".to_owned()],
                vec!["1min".to_owned()],
                vec!["1h".to_owned()],
                vec!["1day".to_owned()],
                vec!["1year".to_owned()],
            ]
        );
    }

    #[test]
    fn sorts_numerically_with_percent_suffixes() {
        let mut rows = vec![
            vec!["100%".to_owned()],
            vec!["2%".to_owned()],
            vec!["2.5%".to_owned()],
            vec!["10%".to_owned()],
            vec!["1".to_owned()],
        ];
        sort_rows(&mut rows, 0, SortMode::Numeric, SortDirection::Ascending);
        assert_eq!(
            rows,
            vec![
                vec!["1".to_owned()],
                vec!["2%".to_owned()],
                vec!["2.5%".to_owned()],
                vec!["10%".to_owned()],
                vec!["100%".to_owned()],
            ]
        );
    }

    #[test]
    fn sorts_numeric_placeholders_after_numbers() {
        let mut rows = vec![
            vec!["n/a".to_owned()],
            vec!["2.5%".to_owned()],
            vec!["null".to_owned()],
            vec!["1".to_owned()],
        ];
        sort_rows(&mut rows, 0, SortMode::Numeric, SortDirection::Ascending);
        assert_eq!(
            rows,
            vec![
                vec!["1".to_owned()],
                vec!["2.5%".to_owned()],
                vec!["n/a".to_owned()],
                vec!["null".to_owned()],
            ]
        );
    }

    #[test]
    fn treats_bare_m_as_minutes_in_time_context() {
        let mut rows = vec![
            vec!["1h".to_owned()],
            vec!["2m".to_owned()],
            vec!["30s".to_owned()],
        ];
        sort_rows(&mut rows, 0, SortMode::Numeric, SortDirection::Ascending);
        assert_eq!(
            rows,
            vec![
                vec!["30s".to_owned()],
                vec!["2m".to_owned()],
                vec!["1h".to_owned()],
            ]
        );
    }

    #[test]
    fn header_hint_treats_bare_m_as_minutes() {
        let mut rows = vec![
            vec!["2m".to_owned()],
            vec!["30".to_owned()],
            vec!["1".to_owned()],
        ];
        let profile = infer_numeric_column_profile(Some("duration"), &rows, 0);
        sort_rows_by_specs(
            &mut rows,
            &[SortSpec {
                column: 0,
                mode: SortMode::Numeric,
                direction: SortDirection::Ascending,
                numeric_profile: profile,
            }],
        );
        assert_eq!(
            rows,
            vec![
                vec!["1".to_owned()],
                vec!["30".to_owned()],
                vec!["2m".to_owned()],
            ]
        );
    }

    #[test]
    fn treats_bare_m_as_milli_in_scientific_context() {
        let mut rows = vec![
            vec!["3k".to_owned()],
            vec!["1".to_owned()],
            vec!["2m".to_owned()],
        ];
        sort_rows(&mut rows, 0, SortMode::Numeric, SortDirection::Ascending);
        assert_eq!(
            rows,
            vec![
                vec!["2m".to_owned()],
                vec!["1".to_owned()],
                vec!["3k".to_owned()],
            ]
        );
    }

    #[test]
    fn leaves_bare_m_as_milli_when_ambiguous() {
        let mut rows = vec![
            vec!["2m".to_owned()],
            vec!["1".to_owned()],
            vec!["1m".to_owned()],
        ];
        sort_rows(&mut rows, 0, SortMode::Numeric, SortDirection::Ascending);
        assert_eq!(
            rows,
            vec![
                vec!["1m".to_owned()],
                vec!["2m".to_owned()],
                vec!["1".to_owned()],
            ]
        );
    }

    #[test]
    fn byte_suffixes_are_case_insensitive() {
        assert_eq!(parse_suffixed_number("1kb"), parse_suffixed_number("1KB"));
        assert_eq!(parse_suffixed_number("1mib"), parse_suffixed_number("1MiB"));
        assert_eq!(
            parse_suffixed_number("1mebibyte"),
            parse_suffixed_number("1MEBIBYTE")
        );
    }
}
