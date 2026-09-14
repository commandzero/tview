use std::fmt;
use std::num::NonZeroUsize;
use std::str::FromStr;

use super::ParseOptions;
use crate::table::{SortDirection, SourceFilterOperator, SourceOperand};

pub const DEFAULT_SCHEMA_SCAN_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ObjectMode {
    #[default]
    Auto,
    Record,
    Entries,
}

impl FromStr for ObjectMode {
    type Err = SourceOptionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "record" => Ok(Self::Record),
            "entries" => Ok(Self::Entries),
            _ => Err(SourceOptionError::InvalidObjectMode(value.to_owned())),
        }
    }
}

impl fmt::Display for ObjectMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Auto => "auto",
            Self::Record => "record",
            Self::Entries => "entries",
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ObjectModeOrigin {
    #[default]
    Default,
    SavedView,
    Cli,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedObjectMode {
    Record,
    Entries,
}

impl fmt::Display for ResolvedObjectMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Record => "record",
            Self::Entries => "entries",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectModeResolution {
    pub requested: ObjectMode,
    pub resolved: ResolvedObjectMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectedValueShape {
    Object,
    Array,
    Scalar,
}

impl fmt::Display for SelectedValueShape {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Object => "object",
            Self::Array => "array",
            Self::Scalar => "scalar",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectedTableShape {
    ArrayRows,
    ObjectRecord,
    ObjectEntries,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedShapeResolution {
    pub table_shape: Option<SelectedTableShape>,
    pub object_mode: Option<ObjectModeResolution>,
    pub warning: Option<String>,
}

pub fn resolve_selected_shape(
    selected: SelectedValueShape,
    requested: ObjectMode,
    origin: ObjectModeOrigin,
    auto_entries: bool,
) -> Result<SelectedShapeResolution, SourceOptionError> {
    if selected == SelectedValueShape::Object {
        let resolved = match requested {
            ObjectMode::Auto if auto_entries => ResolvedObjectMode::Entries,
            ObjectMode::Auto | ObjectMode::Record => ResolvedObjectMode::Record,
            ObjectMode::Entries => ResolvedObjectMode::Entries,
        };
        return Ok(SelectedShapeResolution {
            table_shape: Some(match resolved {
                ResolvedObjectMode::Record => SelectedTableShape::ObjectRecord,
                ResolvedObjectMode::Entries => SelectedTableShape::ObjectEntries,
            }),
            object_mode: Some(ObjectModeResolution {
                requested,
                resolved,
            }),
            warning: None,
        });
    }

    if requested != ObjectMode::Auto {
        if origin != ObjectModeOrigin::SavedView {
            return Err(SourceOptionError::ObjectModeRequiresObject { mode: requested });
        }
        return Ok(SelectedShapeResolution {
            table_shape: (selected == SelectedValueShape::Array)
                .then_some(SelectedTableShape::ArrayRows),
            object_mode: None,
            warning: Some(format!(
                "saved object_mode '{requested}' is incompatible with the selected {selected} and was ignored"
            )),
        });
    }

    Ok(SelectedShapeResolution {
        table_shape: (selected == SelectedValueShape::Array)
            .then_some(SelectedTableShape::ArrayRows),
        object_mode: None,
        warning: None,
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InputFormat {
    #[default]
    Auto,
    Delimited,
    Json,
    Ndjson,
    #[cfg(feature = "sqlite")]
    Sqlite,
    #[cfg(feature = "elasticsearch")]
    Elasticsearch,
}

impl FromStr for InputFormat {
    type Err = SourceOptionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "delimited" => Ok(Self::Delimited),
            "json" => Ok(Self::Json),
            "ndjson" => Ok(Self::Ndjson),
            #[cfg(feature = "sqlite")]
            "sqlite" => Ok(Self::Sqlite),
            #[cfg(feature = "elasticsearch")]
            "elasticsearch" => Ok(Self::Elasticsearch),
            _ => Err(SourceOptionError::InvalidFormat(value.to_owned())),
        }
    }
}

impl fmt::Display for InputFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Auto => "auto",
            Self::Delimited => "delimited",
            Self::Json => "json",
            Self::Ndjson => "ndjson",
            #[cfg(feature = "sqlite")]
            Self::Sqlite => "sqlite",
            #[cfg(feature = "elasticsearch")]
            Self::Elasticsearch => "elasticsearch",
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SchemaScan {
    #[default]
    Default,
    Full,
}

impl FromStr for SchemaScan {
    type Err = SourceOptionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "default" => Ok(Self::Default),
            "full" => Ok(Self::Full),
            _ => Err(SourceOptionError::InvalidSchemaScan(value.to_owned())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StructuredPath {
    encoded: String,
    segments: Vec<String>,
}

impl StructuredPath {
    pub fn root() -> Self {
        Self {
            encoded: String::new(),
            segments: Vec::new(),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.encoded
    }

    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    pub fn encode_segment(segment: &str) -> String {
        segment.replace('~', "~0").replace('/', "~1")
    }
}

impl FromStr for StructuredPath {
    type Err = SourceOptionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() {
            return Ok(Self::root());
        }
        if !value.starts_with('/') {
            return Err(SourceOptionError::InvalidJsonPointer(value.to_owned()));
        }
        let segments = value[1..]
            .split('/')
            .map(|segment| decode_pointer_segment(segment, value))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            encoded: value.to_owned(),
            segments,
        })
    }
}

pub type JsonPointer = StructuredPath;

#[derive(Debug, Clone, PartialEq)]
pub struct SourceFilterRequest {
    pub column: String,
    pub operator: SourceFilterOperator,
    pub operand: Option<SourceOperand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSortRequest {
    pub column: String,
    pub direction: SortDirection,
}

fn decode_pointer_segment(segment: &str, full: &str) -> Result<String, SourceOptionError> {
    let mut decoded = String::new();
    let mut chars = segment.chars();
    while let Some(ch) = chars.next() {
        if ch != '~' {
            decoded.push(ch);
            continue;
        }
        match chars.next() {
            Some('0') => decoded.push('~'),
            Some('1') => decoded.push('/'),
            _ => return Err(SourceOptionError::InvalidJsonPointer(full.to_owned())),
        }
    }
    Ok(decoded)
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenOptions {
    pub format: InputFormat,
    pub delimited: ParseOptions,
    pub json_path: Option<JsonPointer>,
    pub object_mode: ObjectMode,
    pub object_mode_origin: ObjectModeOrigin,
    pub schema_scan: SchemaScan,
    pub table: Option<String>,
    pub native_query: Option<String>,
    pub limit: Option<NonZeroUsize>,
    pub source_filters: Vec<SourceFilterRequest>,
    pub source_sort: Vec<SourceSortRequest>,
    /// Prepare rows incrementally for a direct table preview.
    pub preview: bool,
    pub lazy_threshold_bytes: u64,
    pub schema_scan_bytes: u64,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            format: InputFormat::Auto,
            delimited: ParseOptions::default(),
            json_path: None,
            object_mode: ObjectMode::Auto,
            object_mode_origin: ObjectModeOrigin::Default,
            schema_scan: SchemaScan::Default,
            table: None,
            native_query: None,
            limit: None,
            source_filters: Vec::new(),
            source_sort: Vec::new(),
            preview: false,
            lazy_threshold_bytes: super::DEFAULT_LAZY_THRESHOLD_BYTES,
            schema_scan_bytes: DEFAULT_SCHEMA_SCAN_BYTES,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceOptionOverrides {
    pub format: Option<InputFormat>,
    pub json_path: Option<JsonPointer>,
    pub object_mode: Option<ObjectMode>,
    pub schema_scan: Option<SchemaScan>,
    pub table: Option<String>,
    pub native_query: Option<String>,
    pub limit: Option<NonZeroUsize>,
    pub source_filters: Option<Vec<SourceFilterRequest>>,
    pub source_sort: Option<Vec<SourceSortRequest>>,
}

impl OpenOptions {
    pub fn merge(
        defaults: Self,
        saved: &SourceOptionOverrides,
        cli: &SourceOptionOverrides,
    ) -> Self {
        let (object_mode, object_mode_origin) = if let Some(mode) = cli.object_mode {
            (mode, ObjectModeOrigin::Cli)
        } else if let Some(mode) = saved.object_mode {
            (mode, ObjectModeOrigin::SavedView)
        } else {
            (defaults.object_mode, defaults.object_mode_origin)
        };
        let (table, native_query) = if let Some(query) = &cli.native_query {
            (None, Some(query.clone()))
        } else if let Some(table) = &cli.table {
            (Some(table.clone()), None)
        } else if saved.native_query.is_some() || saved.table.is_some() {
            (saved.table.clone(), saved.native_query.clone())
        } else {
            (defaults.table.clone(), defaults.native_query.clone())
        };
        Self {
            format: cli.format.or(saved.format).unwrap_or(defaults.format),
            json_path: cli
                .json_path
                .clone()
                .or_else(|| saved.json_path.clone())
                .or(defaults.json_path),
            object_mode,
            object_mode_origin,
            schema_scan: cli
                .schema_scan
                .or(saved.schema_scan)
                .unwrap_or(defaults.schema_scan),
            table,
            native_query,
            limit: cli.limit.or(saved.limit).or(defaults.limit),
            source_filters: cli
                .source_filters
                .clone()
                .or_else(|| saved.source_filters.clone())
                .unwrap_or(defaults.source_filters),
            source_sort: cli
                .source_sort
                .clone()
                .or_else(|| saved.source_sort.clone())
                .unwrap_or(defaults.source_sort),
            ..defaults
        }
    }

    pub fn validate(&self) -> Result<(), SourceOptionError> {
        if self.format == InputFormat::Delimited && self.json_path.is_some() {
            return Err(SourceOptionError::JsonPathRequiresStructuredFormat);
        }
        if self.table.is_some() && self.native_query.is_some() {
            return Err(SourceOptionError::TableQueryConflict);
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum SourceOptionError {
    #[cfg(feature = "sqlite")]
    #[error("invalid input format '{0}' (expected auto, delimited, json, ndjson, or sqlite)")]
    InvalidFormat(String),
    #[cfg(not(feature = "sqlite"))]
    #[error("invalid input format '{0}' (expected auto, delimited, json, or ndjson)")]
    InvalidFormat(String),
    #[error("invalid schema scan policy '{0}' (expected default or full)")]
    InvalidSchemaScan(String),
    #[error("invalid object mode '{0}' (expected auto, record, or entries)")]
    InvalidObjectMode(String),
    #[error("invalid RFC 6901 JSON Pointer '{0}'")]
    InvalidJsonPointer(String),
    #[error("JSON starting paths require JSON or NDJSON input")]
    JsonPathRequiresStructuredFormat,
    #[error("source table and native query are mutually exclusive")]
    TableQueryConflict,
    #[error("object mode '{mode}' requires an input with a selected object/map")]
    ObjectModeRequiresObject { mode: ObjectMode },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_and_decodes_json_pointers() {
        let pointer: JsonPointer = "/hits/hits/a~1b/~0meta".parse().expect("pointer");
        assert_eq!(pointer.segments(), ["hits", "hits", "a/b", "~meta"]);
        assert!("hits/hits".parse::<JsonPointer>().is_err());
        assert!("/bad~2escape".parse::<JsonPointer>().is_err());
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn sqlite_is_an_available_input_format() {
        assert_eq!("sqlite".parse::<InputFormat>(), Ok(InputFormat::Sqlite));
    }

    #[cfg(not(feature = "sqlite"))]
    #[test]
    fn sqlite_is_not_an_available_input_format() {
        let error = "sqlite"
            .parse::<InputFormat>()
            .expect_err("disabled format");
        assert_eq!(
            error.to_string(),
            "invalid input format 'sqlite' (expected auto, delimited, json, or ndjson)"
        );
    }

    #[test]
    fn merges_source_options_in_cli_saved_default_order() {
        let defaults = OpenOptions::default();
        let saved = SourceOptionOverrides {
            format: Some(InputFormat::Json),
            object_mode: Some(ObjectMode::Record),
            schema_scan: Some(SchemaScan::Full),
            ..SourceOptionOverrides::default()
        };
        let cli = SourceOptionOverrides {
            object_mode: Some(ObjectMode::Entries),
            schema_scan: Some(SchemaScan::Default),
            ..SourceOptionOverrides::default()
        };
        let merged = OpenOptions::merge(defaults, &saved, &cli);
        assert_eq!(merged.format, InputFormat::Json);
        assert_eq!(merged.object_mode, ObjectMode::Entries);
        assert_eq!(merged.object_mode_origin, ObjectModeOrigin::Cli);
        assert_eq!(merged.schema_scan, SchemaScan::Default);
        assert_ne!(merged.lazy_threshold_bytes, 0);
        assert_ne!(merged.schema_scan_bytes, 0);
    }

    #[test]
    fn resolves_selected_shapes_without_adapter_specific_types() {
        let object = resolve_selected_shape(
            SelectedValueShape::Object,
            ObjectMode::Auto,
            ObjectModeOrigin::Default,
            true,
        )
        .expect("object");
        assert_eq!(object.table_shape, Some(SelectedTableShape::ObjectEntries));
        assert_eq!(
            object.object_mode.unwrap().resolved,
            ResolvedObjectMode::Entries
        );

        let array = resolve_selected_shape(
            SelectedValueShape::Array,
            ObjectMode::Record,
            ObjectModeOrigin::SavedView,
            false,
        )
        .expect("saved array");
        assert_eq!(array.table_shape, Some(SelectedTableShape::ArrayRows));
        assert!(array.warning.unwrap().contains("selected array"));

        assert!(matches!(
            resolve_selected_shape(
                SelectedValueShape::Scalar,
                ObjectMode::Entries,
                ObjectModeOrigin::Cli,
                false,
            ),
            Err(SourceOptionError::ObjectModeRequiresObject {
                mode: ObjectMode::Entries
            })
        ));
    }
}
