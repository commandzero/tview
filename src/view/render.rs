use crate::ops::sort::{parse_bool_key, parse_numeric_scalar, NumericColumnProfile};

use super::{format_number_parts, ColumnDisplayMetadata, DisplayFormatMetadata, LocaleMetadata};

/// The type-sort interpretation is shared by live binding and source-backed
/// projections. Explicit metadata takes precedence over a declared or
/// all-result inferred numeric source type.
#[cfg(feature = "saved-views")]
pub(crate) fn type_sort_mode(
    metadata: ColumnDisplayMetadata,
    source: crate::table::LogicalType,
    inferred_numeric: bool,
) -> crate::ops::sort::SortMode {
    use crate::ops::sort::SortMode;
    match metadata.column_type {
        super::ColumnTypeMetadata::Date => SortMode::Date,
        super::ColumnTypeMetadata::Ip => SortMode::Ip,
        super::ColumnTypeMetadata::SemVer => SortMode::SemVer,
        super::ColumnTypeMetadata::BooleanWord
        | super::ColumnTypeMetadata::BooleanChar
        | super::ColumnTypeMetadata::BooleanBit => SortMode::Boolean,
        super::ColumnTypeMetadata::Float | super::ColumnTypeMetadata::Int => SortMode::Numeric,
        super::ColumnTypeMetadata::Text if metadata.type_explicit => SortMode::Lexical,
        super::ColumnTypeMetadata::Text => match source {
            crate::table::LogicalType::Boolean => SortMode::Boolean,
            crate::table::LogicalType::Integer | crate::table::LogicalType::Float => {
                SortMode::Numeric
            }
            _ if inferred_numeric => SortMode::Numeric,
            _ => SortMode::Lexical,
        },
    }
}

/// Format a source value using only captured column metadata and a numeric
/// interpretation profile. Both live rendering and frozen projection use this
/// function; neither needs a source reader to format already selected cells.
pub(crate) fn cell(
    metadata: ColumnDisplayMetadata,
    profile: NumericColumnProfile,
    raw: &str,
) -> String {
    match metadata.format {
        DisplayFormatMetadata::Plain => raw.to_owned(),
        DisplayFormatMetadata::Uppercase => raw.to_uppercase(),
        DisplayFormatMetadata::Lowercase => raw.to_lowercase(),
        DisplayFormatMetadata::Locale => {
            format_locale_number(raw, profile, metadata.locale).unwrap_or_else(|| raw.to_owned())
        }
        DisplayFormatMetadata::Mask => metadata
            .mask
            .and_then(|mask| {
                parse_numeric_scalar(raw, profile).map(|value| {
                    format_number_parts(
                        value,
                        mask.decimal_places,
                        mask.grouped,
                        LocaleMetadata::en_us(),
                    )
                })
            })
            .unwrap_or_else(|| raw.to_owned()),
        DisplayFormatMetadata::BooleanChar => parse_bool_key(raw)
            .map(|value| if value { "y" } else { "n" }.to_owned())
            .unwrap_or_else(|| raw.to_owned()),
        DisplayFormatMetadata::BooleanBit => parse_bool_key(raw)
            .map(|value| if value { "1" } else { "0" }.to_owned())
            .unwrap_or_else(|| raw.to_owned()),
        DisplayFormatMetadata::BooleanWord => parse_bool_key(raw)
            .map(|value| if value { "true" } else { "false" }.to_owned())
            .unwrap_or_else(|| raw.to_owned()),
    }
}

fn format_locale_number(
    raw: &str,
    profile: NumericColumnProfile,
    locale: LocaleMetadata,
) -> Option<String> {
    let value = parse_numeric_scalar(raw, profile)?;
    let decimal_places = raw
        .split_once('.')
        .map(|(_, fraction)| {
            fraction
                .chars()
                .take_while(|ch| ch.is_ascii_digit())
                .count()
        })
        .unwrap_or(0);
    Some(format_number_parts(value, decimal_places, true, locale))
}

#[cfg(feature = "saved-views")]
pub(crate) fn saved_column(
    column: &crate::saved_views::ColumnView,
    locale: Option<&str>,
    numeric: bool,
) -> ColumnDisplayMetadata {
    ColumnDisplayMetadata {
        type_explicit: column.column_type.is_some(),
        column_type: column
            .column_type
            .map(super::column_type_metadata)
            .unwrap_or(if numeric {
                super::ColumnTypeMetadata::Float
            } else {
                super::ColumnTypeMetadata::Text
            }),
        format: column
            .format
            .map(super::display_format_metadata)
            .or_else(|| column.mask.as_ref().map(|_| DisplayFormatMetadata::Mask))
            .unwrap_or(DisplayFormatMetadata::Plain),
        mask: column.mask.as_ref().map(|mask| super::NumberMaskMetadata {
            grouped: mask.grouped,
            decimal_places: mask.decimal_places,
        }),
        locale: LocaleMetadata::from_posix(locale),
    }
}
