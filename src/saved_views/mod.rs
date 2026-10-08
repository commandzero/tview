pub mod binding;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::Deserialize;
use yaml_serde::{Mapping, Value};

use crate::ingest::{
    InputFormat, JsonPointer, ObjectMode, OpenOptions, SchemaScan, SourceFilterRequest,
    SourceOptionOverrides, SourceSortRequest,
};
#[cfg(test)]
use crate::table::ColumnSourceIdentity;
use crate::table::{
    NullPlacement, SortDirection as TableSortDirection,
    SourceFilterOperator as TableSourceFilterOperator, SourceOperand,
};
use crate::theme::{
    ConditionalColorRule, ConditionalValue, GradientStop, IdentifierColors, MatchEntry, RangeEntry,
};

pub const MAX_SORT_KEYS: usize = 3;
const VIEW_DIR: &str = "tview/views";

#[derive(Debug, Clone, PartialEq)]
pub struct SavedView {
    pub name: String,
    pub filenames: Vec<FilenamePattern>,
    pub source: SavedSourceConfig,
    pub view: SavedViewConfig,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SavedSourceConfig {
    pub format: Option<InputFormat>,
    pub json_path: Option<JsonPointer>,
    pub object_mode: Option<ObjectMode>,
    pub schema_scan: Option<SchemaScan>,
    pub table: Option<String>,
    pub query: Option<String>,
    pub limit: Option<usize>,
    pub filters: Vec<SavedSourceFilter>,
    pub sort: Vec<SavedSourceSort>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SavedViewConfig {
    pub locale: Option<String>,
    pub nulls: Option<NullPlacement>,
    pub columns: BTreeMap<String, ColumnView>,
    pub sort: Vec<SortKey>,
    pub filters: Vec<SavedFilter>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SavedSourceFilter {
    pub column: String,
    pub operator: SourceFilterOperator,
    pub value: Option<SourceFilterValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFilterOperator {
    Equal,
    NotEqual,
    LessThan,
    LessOrEqual,
    GreaterThan,
    GreaterOrEqual,
    Contains,
    Prefix,
    IsNull,
    IsNotNull,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SourceFilterValue {
    Boolean(bool),
    Integer(i64),
    Float(f64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedSourceSort {
    pub column: String,
    pub direction: SortDirection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedSavedView {
    pub view: SavedView,
    pub warnings: Vec<SavedViewWarning>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SavedViewFile {
    pub path: PathBuf,
    pub canonical_name: String,
    pub view: SavedView,
    pub warnings: Vec<SavedViewWarning>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SavedViewDiscovery {
    pub views: Vec<SavedViewFile>,
    pub warnings: Vec<SavedViewWarning>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectedSavedView<'a> {
    pub view: &'a SavedViewFile,
    pub warnings: Vec<SavedViewWarning>,
}
/// The validated value selected for an invocation, independent of later filesystem edits.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectedSavedViewSnapshot {
    pub path: PathBuf,
    pub canonical_name: String,
    pub view: SavedView,
}

#[expect(
    clippy::large_enum_variant,
    reason = "One invocation-owned snapshot needs no extra heap allocation for its disabled variant"
)]
#[derive(Debug, Clone, PartialEq)]
pub enum SavedViewInvocation {
    Disabled,
    Enabled {
        selected: Option<SelectedSavedViewSnapshot>,
        target_path: Option<PathBuf>,
        view_name: String,
        warnings: Vec<SavedViewWarning>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedColumnView {
    pub column_index: usize,
    pub source_key: String,
    pub view: ColumnView,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedViewSelection<'a> {
    Auto {
        input_path: &'a Path,
    },
    /// Match the complete non-secret source identity, not a filesystem basename.
    AutoSource {
        identity: &'a str,
    },
    Force {
        name: &'a str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilenamePattern {
    pub raw: String,
    pub kind: FilenamePatternKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilenamePatternKind {
    Exact,
    Glob,
    Regex,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ColumnView {
    pub label: Option<String>,
    pub nulls: Option<NullPlacement>,
    pub column_type: Option<ColumnType>,
    pub format: Option<DisplayFormat>,
    pub mask: Option<NumberMask>,
    pub width: Option<ColumnWidth>,
    pub align: Option<ColumnAlign>,
    pub visible: Option<bool>,
    pub colors: Vec<ConditionalColorRule>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    String(StringKind),
    Number(NumberKind),
    Boolean(BooleanKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringKind {
    Text,
    Date,
    Ip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberKind {
    Float,
    Int,
    SemVer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BooleanKind {
    Char,
    Bit,
    Word,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayFormat {
    Plain,
    Locale,
    Mask,
    Uppercase,
    Lowercase,
    Char,
    Bit,
    Word,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumberMask {
    pub raw: String,
    pub grouped: bool,
    pub decimal_places: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnWidth {
    Fixed(u16),
    Header,
    Content,
    Mode,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnAlign {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortKey {
    pub column: String,
    pub direction: SortDirection,
    pub kind: SortKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKind {
    Lexical,
    Natural,
    Numeric,
    Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedFilter {
    pub column: String,
    pub action: FilterAction,
    pub kind: FilterKind,
    pub condition: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterAction {
    In,
    Out,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterKind {
    Text,
    Regex,
    Numeric,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedViewWarning {
    pub field: String,
    pub message: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SavedViewParseError {
    #[error("invalid saved view yaml: {0}")]
    Yaml(String),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSavedView {
    name: String,
    filenames: Vec<String>,
    source: RawSavedSourceConfig,
    view: RawSavedViewConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSavedSourceConfig {
    format: Option<String>,
    json_path: Option<String>,
    object_mode: Option<String>,
    schema_scan: Option<String>,
    table: Option<String>,
    query: Option<String>,
    limit: Option<usize>,
    #[serde(default)]
    filters: Vec<RawSavedSourceFilter>,
    #[serde(default)]
    sort: Vec<RawSavedSourceSort>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSavedViewConfig {
    locale: Option<String>,
    nulls: Option<String>,
    #[serde(default)]
    columns: BTreeMap<String, RawColumnView>,
    #[serde(default)]
    sort: Vec<RawSortKey>,
    #[serde(default)]
    filters: Vec<RawSavedFilter>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawColumnView {
    label: Option<String>,
    nulls: Option<String>,
    #[serde(rename = "type")]
    column_type: Option<String>,
    format: Option<String>,
    mask: Option<String>,
    width: Option<RawColumnWidth>,
    align: Option<String>,
    visible: Option<bool>,
    #[serde(default)]
    colors: Vec<RawColorRule>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RawColumnWidth {
    Fixed(u16),
    Mode(String),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawColorRule {
    gradient: Option<RawGradientRule>,
    #[serde(rename = "match")]
    match_rule: Option<Mapping>,
    range: Option<Mapping>,
    identifiers: Option<RawIdentifiersRule>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIdentifiersRule {
    colors: Option<RawIdentifierColors>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RawIdentifierColors {
    Mode(String),
    Colors(Vec<String>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGradientRule {
    mode: String,
    stops: Option<Mapping>,
    colors: Option<Vec<String>>,
    steps: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSortKey {
    column: String,
    direction: String,
    kind: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSavedFilter {
    column: String,
    action: String,
    kind: String,
    condition: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSavedSourceFilter {
    column: String,
    operator: String,
    #[serde(default, deserialize_with = "deserialize_present_value")]
    value: Option<Value>,
}

fn deserialize_present_value<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSavedSourceSort {
    column: String,
    direction: String,
}

pub fn parse_saved_view_yaml(input: &str) -> Result<ValidatedSavedView, SavedViewParseError> {
    let raw: RawSavedView =
        yaml_serde::from_str(input).map_err(|err| SavedViewParseError::Yaml(err.to_string()))?;
    Ok(validate_raw_view(raw))
}

pub fn saved_view_dir(config_root: Option<&Path>) -> Option<PathBuf> {
    let root = config_root
        .map(Path::to_path_buf)
        .or_else(posix_config_dir)?;
    Some(root.join(VIEW_DIR))
}

fn posix_config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

pub fn discover_saved_views(config_root: Option<&Path>) -> SavedViewDiscovery {
    let Some(view_dir) = saved_view_dir(config_root) else {
        return SavedViewDiscovery::default();
    };
    discover_saved_views_in_dir(&view_dir)
}

pub fn discover_saved_views_in_dir(view_dir: &Path) -> SavedViewDiscovery {
    let mut discovery = SavedViewDiscovery::default();
    let mut candidates = Vec::new();
    let mut directories = vec![view_dir.to_path_buf()];
    while let Some(directory) = directories.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if file_type.is_dir() {
                directories.push(path);
            } else if matches!(
                view_extension(&path),
                Some(ViewExtension::Yml | ViewExtension::Yaml)
            ) {
                candidates.push(path);
            }
        }
    }
    candidates.sort_by(|left, right| {
        view_stem(left)
            .map(view_name_key)
            .cmp(&view_stem(right).map(view_name_key))
            .then_with(|| view_extension_priority(left).cmp(&view_extension_priority(right)))
            .then_with(|| left.cmp(right))
    });

    let mut seen = BTreeSet::new();
    for path in candidates {
        let Some(stem) = view_stem(&path) else {
            continue;
        };
        let canonical_name = stem.to_owned();
        if !seen.insert(view_name_key(&canonical_name)) {
            discovery.warnings.push(warning(
                path.display().to_string(),
                format!(
                    "duplicate saved view '{}': .yml takes precedence over .yaml; otherwise the first path in lexical order wins",
                    canonical_name
                ),
            ));
            continue;
        }
        match fs::read_to_string(&path) {
            Ok(contents) => match parse_saved_view_yaml(&contents) {
                Ok(validated) => discovery.views.push(SavedViewFile {
                    path,
                    canonical_name,
                    view: validated.view,
                    warnings: validated.warnings,
                }),
                Err(err) => discovery.warnings.push(warning(
                    path.display().to_string(),
                    format!("failed to parse saved view: {err}"),
                )),
            },
            Err(err) => discovery.warnings.push(warning(
                path.display().to_string(),
                format!("failed to read saved view: {err}"),
            )),
        }
    }
    discovery
}

pub fn select_saved_view<'a>(
    views: &'a [SavedViewFile],
    selection: SavedViewSelection<'_>,
) -> Option<SelectedSavedView<'a>> {
    let (identity, exact_case_sensitive) = match selection {
        SavedViewSelection::Force { name } => {
            let normalized = normalize_view_name(name);
            return views
                .iter()
                .find(|view| platform_eq(&view.canonical_name, &normalized))
                .map(|view| SelectedSavedView {
                    view,
                    warnings: Vec::new(),
                });
        }
        SavedViewSelection::Auto { input_path } => (input_path.file_name()?.to_str()?, false),
        SavedViewSelection::AutoSource { identity } => (identity, true),
    };
    let mut matches = views
        .iter()
        .filter_map(|view| {
            best_match_rank(view, identity, exact_case_sensitive).map(|rank| (rank, view))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|(left_rank, left), (right_rank, right)| {
        left_rank
            .cmp(right_rank)
            .then_with(|| left.path.cmp(&right.path))
    });
    let (rank, view) = matches.first()?;
    let ambiguous = matches
        .iter()
        .skip(1)
        .any(|(other_rank, _)| other_rank == rank);
    let mut warnings = Vec::new();
    if ambiguous {
        warnings.push(warning(
            identity,
            format!(
                "multiple saved views matched '{}'; using {}",
                identity,
                view.path.display()
            ),
        ));
    }
    Some(SelectedSavedView { view, warnings })
}
/// Discover and select once, retaining only the selected validated document.
pub fn prepare_saved_view(
    selection: Option<SavedViewSelection<'_>>,
    input_path: &Path,
    config_root: Option<&Path>,
) -> Result<SavedViewInvocation, String> {
    let Some(selection) = selection else {
        return Ok(SavedViewInvocation::Disabled);
    };
    let target_path = saved_view_dir(config_root).and_then(|directory| {
        let basename = input_path.file_name()?.to_str()?;
        // A textual source's safe filename has no filesystem extension to strip.
        let stem = match &selection {
            SavedViewSelection::AutoSource { .. } => basename,
            SavedViewSelection::Auto { .. } | SavedViewSelection::Force { .. } => {
                basename.rsplit_once('.').map_or(basename, |(stem, _)| stem)
            }
        };
        Some(directory.join(format!("{stem}.yml")))
    });
    let view_name = target_path
        .as_deref()
        .and_then(Path::file_stem)
        .and_then(|stem| stem.to_str())
        .unwrap_or("view")
        .to_owned();
    let mut discovery = discover_saved_views(config_root);
    let forced_name = match &selection {
        SavedViewSelection::Force { name } => Some(*name),
        SavedViewSelection::Auto { .. } | SavedViewSelection::AutoSource { .. } => None,
    };
    let selected = select_saved_view(&discovery.views, selection).map(|selected| {
        let index = discovery
            .views
            .iter()
            .position(|candidate| std::ptr::eq(candidate, selected.view))
            .expect("selected candidate belongs to discovery");
        (index, selected.warnings)
    });
    if selected.is_none() {
        if let Some(name) = forced_name {
            return Err(format!(
                "saved view '{name}' was requested but was not found"
            ));
        }
    }
    let selected = selected.map(|(index, selection_warnings)| {
        let selected = discovery.views.swap_remove(index);
        discovery.warnings.extend(selected.warnings);
        discovery.warnings.extend(selection_warnings);
        SelectedSavedViewSnapshot {
            path: selected.path,
            canonical_name: selected.canonical_name,
            view: selected.view,
        }
    });
    let warnings = discovery.warnings;
    Ok(SavedViewInvocation::Enabled {
        selected,
        target_path,
        view_name,
        warnings,
    })
}

pub fn normalize_view_name(name: &str) -> String {
    let path = Path::new(name);
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("yml" | "yaml") => path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(name)
            .to_owned(),
        _ => name.to_owned(),
    }
}

fn validate_raw_view(raw: RawSavedView) -> ValidatedSavedView {
    let mut warnings = Vec::new();
    let format = raw.source.format.and_then(|value| match value.parse() {
        Ok(value) => Some(value),
        Err(_) => {
            warnings.push(warning(
                "source.format",
                format!("unknown input format '{value}'"),
            ));
            None
        }
    });
    let json_path = raw.source.json_path.and_then(|value| match value.parse() {
        Ok(value) => Some(value),
        Err(_) => {
            warnings.push(warning(
                "source.json_path",
                format!("invalid RFC 6901 JSON Pointer '{value}'"),
            ));
            None
        }
    });
    let mut object_mode = raw
        .source
        .object_mode
        .and_then(|value| match value.parse() {
            Ok(value) => Some(value),
            Err(_) => {
                warnings.push(warning(
                    "source.object_mode",
                    format!("unknown object mode '{value}'"),
                ));
                None
            }
        });
    if matches!(format, Some(InputFormat::Delimited | InputFormat::Ndjson))
        && matches!(object_mode, Some(ObjectMode::Record | ObjectMode::Entries))
    {
        warnings.push(warning(
            "source.object_mode",
            "object mode is incompatible with the selected row-stream format",
        ));
        object_mode = None;
    }
    let schema_scan = raw
        .source
        .schema_scan
        .and_then(|value| match value.parse() {
            Ok(value) => Some(value),
            Err(_) => {
                warnings.push(warning(
                    "source.schema_scan",
                    format!("unknown schema scan policy '{value}'"),
                ));
                None
            }
        });
    let table = raw.source.table.and_then(|value| {
        if value.is_empty() {
            warnings.push(warning("source.table", "table cannot be empty"));
            None
        } else {
            Some(value)
        }
    });
    let query = raw.source.query.and_then(|value| {
        if value.trim().is_empty() {
            warnings.push(warning("source.query", "query cannot be empty"));
            None
        } else {
            Some(value)
        }
    });
    if table.is_some() && query.is_some() {
        warnings.push(warning("source", "table and query are mutually exclusive"));
    }
    let limit = raw.source.limit.and_then(|value| {
        if value == 0 {
            warnings.push(warning("source.limit", "limit must be greater than zero"));
            None
        } else {
            Some(value)
        }
    });
    let source_filters = raw
        .source
        .filters
        .into_iter()
        .filter_map(|filter| validate_source_filter(filter, &mut warnings))
        .collect();
    let source_sort = raw
        .source
        .sort
        .into_iter()
        .filter_map(|sort| validate_source_sort(sort, &mut warnings))
        .collect();
    let nulls = raw.view.nulls.and_then(|value| {
        parse_null_placement(&value).or_else(|| {
            warnings.push(warning(
                "view.nulls",
                format!("unknown null placement '{value}'"),
            ));
            None
        })
    });
    let locale = raw.view.locale.and_then(|locale| {
        if is_posix_locale(&locale) {
            Some(locale)
        } else {
            warnings.push(warning(
                "view.locale",
                format!("unsupported POSIX-style locale '{locale}', falling back to en_US"),
            ));
            None
        }
    });
    let filenames = raw
        .filenames
        .into_iter()
        .filter_map(|pattern| validate_filename_pattern(pattern, &mut warnings))
        .collect();
    let columns = raw
        .view
        .columns
        .into_iter()
        .filter_map(|(key, raw_column)| validate_column(key, raw_column, &mut warnings))
        .collect();
    let sort_count = raw.view.sort.len();
    let sort = raw
        .view
        .sort
        .into_iter()
        .take(MAX_SORT_KEYS)
        .filter_map(|sort| validate_sort(sort, &mut warnings))
        .collect::<Vec<_>>();
    if sort_count > MAX_SORT_KEYS {
        warnings.push(warning(
            "view.sort",
            format!("only the first {MAX_SORT_KEYS} sort keys are used"),
        ));
    }
    let filters = raw
        .view
        .filters
        .into_iter()
        .filter_map(|filter| validate_filter(filter, &mut warnings))
        .collect();

    ValidatedSavedView {
        view: SavedView {
            name: raw.name,
            filenames,
            source: SavedSourceConfig {
                format,
                json_path,
                object_mode,
                schema_scan,
                table,
                query,
                limit,
                filters: source_filters,
                sort: source_sort,
            },
            view: SavedViewConfig {
                locale,
                nulls,
                columns,
                sort,
                filters,
            },
        },
        warnings,
    }
}

fn validate_filename_pattern(
    raw: String,
    warnings: &mut Vec<SavedViewWarning>,
) -> Option<FilenamePattern> {
    if raw.is_empty() {
        warnings.push(warning("filenames", "empty filename pattern ignored"));
        return None;
    }
    let kind = classify_filename_pattern(&raw);
    match kind {
        FilenamePatternKind::Regex => {
            if let Err(err) = Regex::new(&raw) {
                warnings.push(warning(
                    "filenames",
                    format!("invalid regex filename pattern '{raw}': {err}"),
                ));
                return None;
            }
        }
        FilenamePatternKind::Glob => {
            if let Err(message) = validate_glob_pattern(&raw) {
                warnings.push(warning(
                    "filenames",
                    format!("invalid glob filename pattern '{raw}': {message}"),
                ));
                return None;
            }
        }
        FilenamePatternKind::Exact => {}
    }
    Some(FilenamePattern { raw, kind })
}

fn classify_filename_pattern(raw: &str) -> FilenamePatternKind {
    if raw.starts_with('.') && raw.contains("://") {
        // A context's name and table selector are literal identity components,
        // even when they contain characters used by filename pattern syntax.
        FilenamePatternKind::Exact
    } else if raw.starts_with('^') || raw.ends_with('$') {
        FilenamePatternKind::Regex
    } else if raw.contains('*') || raw.contains('?') || raw.contains('[') {
        FilenamePatternKind::Glob
    } else {
        FilenamePatternKind::Exact
    }
}

fn best_match_rank(
    view: &SavedViewFile,
    identity: &str,
    exact_case_sensitive: bool,
) -> Option<MatchRank> {
    view.view
        .filenames
        .iter()
        .filter_map(|pattern| match_filename_pattern(pattern, identity, exact_case_sensitive))
        .min()
}

fn match_filename_pattern(
    pattern: &FilenamePattern,
    identity: &str,
    exact_case_sensitive: bool,
) -> Option<MatchRank> {
    match pattern.kind {
        FilenamePatternKind::Exact => {
            let equal = if exact_case_sensitive {
                pattern.raw == identity
            } else {
                platform_eq(&pattern.raw, identity)
            };
            equal.then_some(MatchRank::Exact)
        }
        FilenamePatternKind::Glob => {
            glob_matches(&pattern.raw, identity).then_some(MatchRank::Glob)
        }
        FilenamePatternKind::Regex => {
            let case_insensitive =
                platform_case_insensitive().then(|| format!("(?i:{})", pattern.raw));
            Regex::new(case_insensitive.as_deref().unwrap_or(&pattern.raw))
                .ok()
                .is_some_and(|regex| regex.is_match(identity))
                .then_some(MatchRank::Regex)
        }
    }
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    let regex_pattern = glob_to_regex(pattern);
    Regex::new(&regex_pattern)
        .ok()
        .is_some_and(|regex| regex.is_match(value))
}

fn column_glob_matches(pattern: &str, value: &str) -> bool {
    let regex_pattern = glob_to_regex_case_insensitive(pattern);
    Regex::new(&regex_pattern)
        .ok()
        .is_some_and(|regex| regex.is_match(value))
}

fn glob_to_regex(pattern: &str) -> String {
    let mut regex = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            '[' | ']' => regex.push(ch),
            _ => regex.push_str(&regex::escape(&ch.to_string())),
        }
    }
    regex.push('$');
    if platform_case_insensitive() {
        format!("(?i:{regex})")
    } else {
        regex
    }
}

fn glob_to_regex_case_insensitive(pattern: &str) -> String {
    format!("(?i:{})", glob_to_regex_base(pattern))
}

fn glob_to_regex_base(pattern: &str) -> String {
    let mut regex = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            '[' | ']' => regex.push(ch),
            _ => regex.push_str(&regex::escape(&ch.to_string())),
        }
    }
    regex.push('$');
    regex
}

fn wildcard_specificity(pattern: &str) -> usize {
    pattern
        .chars()
        .filter(|ch| !matches!(ch, '*' | '?' | '[' | ']'))
        .count()
}

fn view_name_key(name: &str) -> String {
    if platform_case_insensitive() {
        name.to_ascii_lowercase()
    } else {
        name.to_owned()
    }
}

fn platform_eq(left: &str, right: &str) -> bool {
    if platform_case_insensitive() {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

fn platform_case_insensitive() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MatchRank {
    Exact,
    Glob,
    Regex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewExtension {
    Yml,
    Yaml,
}

fn view_extension(path: &Path) -> Option<ViewExtension> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("yml") => Some(ViewExtension::Yml),
        Some("yaml") => Some(ViewExtension::Yaml),
        _ => None,
    }
}

fn view_stem(path: &Path) -> Option<&str> {
    path.file_stem().and_then(|stem| stem.to_str())
}

fn view_extension_priority(path: &Path) -> u8 {
    match view_extension(path) {
        Some(ViewExtension::Yml) => 0,
        Some(ViewExtension::Yaml) => 1,
        None => 2,
    }
}

fn validate_glob_pattern(raw: &str) -> Result<(), &'static str> {
    let mut open_bracket = false;
    for ch in raw.chars() {
        match ch {
            '[' if open_bracket => return Err("nested character classes are not supported"),
            '[' => open_bracket = true,
            ']' if open_bracket => open_bracket = false,
            ']' => return Err("unmatched closing bracket"),
            _ => {}
        }
    }
    if open_bracket {
        Err("unmatched opening bracket")
    } else {
        Ok(())
    }
}

fn validate_column(
    key: String,
    raw: RawColumnView,
    warnings: &mut Vec<SavedViewWarning>,
) -> Option<(String, ColumnView)> {
    if key.is_empty() {
        warnings.push(warning("view.columns", "empty column key ignored"));
        return None;
    }
    if is_wildcard_pattern(&key) {
        if let Err(message) = validate_glob_pattern(&key) {
            warnings.push(warning(
                format!("view.columns.{key}"),
                format!("invalid wildcard column key: {message}"),
            ));
            return None;
        }
    }
    let field = |name: &str| format!("view.columns.{key}.{name}");
    let column_type = raw.column_type.and_then(|value| {
        parse_column_type(&value).or_else(|| {
            warnings.push(warning(
                field("type"),
                format!("unknown column type '{value}'"),
            ));
            None
        })
    });
    let format = raw.format.and_then(|value| {
        parse_display_format(&value).or_else(|| {
            warnings.push(warning(
                field("format"),
                format!("unknown format '{value}'"),
            ));
            None
        })
    });
    let mask = raw.mask.and_then(|value| match parse_number_mask(&value) {
        Ok(mask) => Some(mask),
        Err(message) => {
            warnings.push(warning(
                field("mask"),
                format!("invalid numeric mask '{value}': {message}"),
            ));
            None
        }
    });
    let width = raw.width.and_then(|value| match parse_width(value) {
        Ok(width) => Some(width),
        Err(message) => {
            warnings.push(warning(field("width"), message));
            None
        }
    });
    let align = raw.align.and_then(|value| {
        parse_align(&value).or_else(|| {
            warnings.push(warning(field("align"), format!("unknown align '{value}'")));
            None
        })
    });
    let label = raw.label.and_then(|value| {
        if value.is_empty() {
            warnings.push(warning(field("label"), "label cannot be empty"));
            None
        } else {
            Some(value)
        }
    });
    let nulls = raw.nulls.and_then(|value| {
        parse_null_placement(&value).or_else(|| {
            warnings.push(warning(
                field("nulls"),
                format!("unknown null placement '{value}'"),
            ));
            None
        })
    });
    if format == Some(DisplayFormat::Mask) && mask.is_none() {
        warnings.push(warning(field("mask"), "format: mask requires a valid mask"));
    }
    if matches!(format, Some(DisplayFormat::Locale | DisplayFormat::Mask))
        && !matches!(column_type, Some(ColumnType::Number(_)) | None)
    {
        warnings.push(warning(
            field("format"),
            "number formats are ignored for non-number column types",
        ));
    }
    let colors = raw
        .colors
        .into_iter()
        .enumerate()
        .filter_map(|(idx, rule)| {
            validate_color_rule(rule, &field(&format!("colors.{idx}")), warnings)
        })
        .collect();

    Some((
        key,
        ColumnView {
            label,
            nulls,
            column_type,
            format,
            mask,
            width,
            align,
            visible: raw.visible,
            colors,
        },
    ))
}

fn validate_color_rule(
    raw: RawColorRule,
    field: &str,
    warnings: &mut Vec<SavedViewWarning>,
) -> Option<ConditionalColorRule> {
    let count = usize::from(raw.gradient.is_some())
        + usize::from(raw.match_rule.is_some())
        + usize::from(raw.range.is_some())
        + usize::from(raw.identifiers.is_some());
    if count != 1 {
        warnings.push(warning(
            field,
            "color rule must define exactly one of gradient, match, range, or identifiers",
        ));
        return None;
    }
    if let Some(rule) = raw.match_rule {
        return validate_match_rule(rule, &format!("{field}.match"), warnings);
    }
    if let Some(rule) = raw.range {
        return validate_range_rule(rule, &format!("{field}.range"), warnings);
    }
    if let Some(rule) = raw.identifiers {
        return validate_identifier_rule(rule, &format!("{field}.identifiers"), warnings);
    }
    validate_gradient_rule(raw.gradient.expect("count checked"), field, warnings)
}

fn validate_match_rule(
    raw: Mapping,
    field: &str,
    warnings: &mut Vec<SavedViewWarning>,
) -> Option<ConditionalColorRule> {
    if raw.is_empty() {
        warnings.push(warning(
            field,
            "match requires at least one value/color entry",
        ));
        return None;
    }
    let mut entries = Vec::new();
    for (idx, (key, color_value)) in raw.into_iter().enumerate() {
        let Some(match_value) = match_value_from_yaml_key(key, &format!("{field}.{idx}"), warnings)
        else {
            continue;
        };
        let Some(color) = yaml_string_value(color_value) else {
            warnings.push(warning(
                format!("{field}.{idx}"),
                "match color must be a string",
            ));
            continue;
        };
        if color.trim().is_empty() {
            warnings.push(warning(format!("{field}.{idx}"), "color is required"));
            continue;
        }
        entries.push(MatchEntry {
            value: match_value,
            color,
        });
    }
    if entries.is_empty() {
        warnings.push(warning(field, "match has no valid value/color entries"));
        return None;
    }
    Some(ConditionalColorRule::Match { entries })
}

fn validate_range_rule(
    raw: Mapping,
    field: &str,
    warnings: &mut Vec<SavedViewWarning>,
) -> Option<ConditionalColorRule> {
    if raw.is_empty() {
        warnings.push(warning(
            field,
            "range requires at least one comparison/color entry",
        ));
        return None;
    }
    let mut entries = Vec::new();
    for (idx, (key, color_value)) in raw.into_iter().enumerate() {
        let Some(expression) = yaml_string_value(key) else {
            warnings.push(warning(
                format!("{field}.{idx}"),
                "range comparison must be a string",
            ));
            continue;
        };
        let Some(color) = yaml_string_value(color_value) else {
            warnings.push(warning(
                format!("{field}.{idx}"),
                "range color must be a string",
            ));
            continue;
        };
        if color.trim().is_empty() {
            warnings.push(warning(format!("{field}.{idx}"), "color is required"));
            continue;
        }
        let Some(entry) =
            range_entry_from_expression(&expression, color, &format!("{field}.{idx}"), warnings)
        else {
            continue;
        };
        entries.push(entry);
    }
    if entries.is_empty() {
        warnings.push(warning(
            field,
            "range has no valid comparison/color entries",
        ));
        return None;
    }
    Some(ConditionalColorRule::Range { entries })
}

fn range_entry_from_expression(
    expression: &str,
    color: String,
    field: &str,
    warnings: &mut Vec<SavedViewWarning>,
) -> Option<RangeEntry> {
    let mut entry = RangeEntry {
        lt: None,
        lte: None,
        gt: None,
        gte: None,
        color,
    };
    let mut comparisons = 0;
    for token in expression.split_whitespace() {
        comparisons += 1;
        let Some((operator, value)) = parse_range_comparison(token) else {
            warnings.push(warning(
                field,
                "range comparison must look like <10, <=10, >90, >=90, or >=50 <75",
            ));
            return None;
        };
        let target = match operator {
            "<" => &mut entry.lt,
            "<=" => &mut entry.lte,
            ">" => &mut entry.gt,
            ">=" => &mut entry.gte,
            _ => unreachable!("operator checked"),
        };
        if target.is_some() {
            warnings.push(warning(field, "range comparison has a duplicate bound"));
            return None;
        }
        *target = Some(value);
    }
    if comparisons == 0 {
        warnings.push(warning(field, "range comparison is required"));
        return None;
    }
    Some(entry)
}

fn parse_range_comparison(token: &str) -> Option<(&'static str, f64)> {
    let token = token.trim();
    let (operator, value) = token
        .strip_prefix("<=")
        .map(|value| ("<=", value))
        .or_else(|| token.strip_prefix(">=").map(|value| (">=", value)))
        .or_else(|| token.strip_prefix('<').map(|value| ("<", value)))
        .or_else(|| token.strip_prefix('>').map(|value| (">", value)))?;
    if value.trim() != value || value.is_empty() {
        return None;
    }
    let value = value.parse::<f64>().ok()?;
    value.is_finite().then_some((operator, value))
}

fn match_value_from_yaml_key(
    key: Value,
    field: &str,
    warnings: &mut Vec<SavedViewWarning>,
) -> Option<ConditionalValue> {
    match key {
        Value::Bool(value) => Some(ConditionalValue::Bool(value)),
        Value::Number(value) => {
            let value = value.as_f64()?;
            if !value.is_finite() {
                warnings.push(warning(field, "number must be finite"));
                return None;
            }
            Some(ConditionalValue::Number(value))
        }
        Value::String(value) => {
            if value.is_empty() {
                warnings.push(warning(field, "match string value must not be empty"));
                return None;
            }
            Some(ConditionalValue::String(value))
        }
        _ => {
            warnings.push(warning(
                field,
                "match value must be a string, number, or boolean",
            ));
            None
        }
    }
}

fn yaml_string_value(value: Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value),
        _ => None,
    }
}

fn validate_identifier_rule(
    raw: RawIdentifiersRule,
    field: &str,
    warnings: &mut Vec<SavedViewWarning>,
) -> Option<ConditionalColorRule> {
    let colors = match raw.colors {
        None => IdentifierColors::Auto,
        Some(RawIdentifierColors::Mode(mode)) if mode == "auto" => IdentifierColors::Auto,
        Some(RawIdentifierColors::Mode(mode)) => {
            warnings.push(warning(
                format!("{field}.colors"),
                format!("unknown identifiers colors mode '{mode}'"),
            ));
            return None;
        }
        Some(RawIdentifierColors::Colors(colors)) => {
            if colors.is_empty() || colors.iter().any(|color| color.trim().is_empty()) {
                warnings.push(warning(
                    format!("{field}.colors"),
                    "identifiers colors requires at least one color",
                ));
                return None;
            }
            IdentifierColors::Colors(colors)
        }
    };
    Some(ConditionalColorRule::Identifiers { colors })
}

fn validate_gradient_rule(
    raw: RawGradientRule,
    field: &str,
    warnings: &mut Vec<SavedViewWarning>,
) -> Option<ConditionalColorRule> {
    match raw.mode.as_str() {
        "fixed" => {
            let mut stops = validate_gradient_stops(
                raw.stops.unwrap_or_default(),
                &format!("{field}.gradient.stops"),
                warnings,
            );
            if stops.len() < 2 {
                warnings.push(warning(
                    format!("{field}.gradient.stops"),
                    "fixed gradient requires at least two stops",
                ));
                return None;
            }
            stops.sort_by(|left, right| left.value.total_cmp(&right.value));
            Some(ConditionalColorRule::FixedGradient { stops })
        }
        "auto" => {
            let colors = raw.colors.unwrap_or_default();
            if colors.len() < 2 || colors.iter().any(|color| color.trim().is_empty()) {
                warnings.push(warning(
                    format!("{field}.gradient.colors"),
                    "auto gradient requires at least two colors",
                ));
                return None;
            }
            Some(ConditionalColorRule::AutoGradient {
                colors,
                steps: raw.steps.unwrap_or(8).max(1),
            })
        }
        other => {
            warnings.push(warning(
                format!("{field}.gradient.mode"),
                format!("unknown gradient mode '{other}'"),
            ));
            None
        }
    }
}

fn validate_gradient_stops(
    raw: Mapping,
    field: &str,
    warnings: &mut Vec<SavedViewWarning>,
) -> Vec<GradientStop> {
    raw.into_iter()
        .enumerate()
        .filter_map(|(idx, (key, color_value))| {
            let Some(value) = gradient_stop_value_from_yaml_key(key) else {
                warnings.push(warning(
                    format!("{field}.{idx}"),
                    "fixed gradient stop value must be a finite number",
                ));
                return None;
            };
            let Some(color) = yaml_string_value(color_value) else {
                warnings.push(warning(
                    format!("{field}.{idx}"),
                    "fixed gradient stop color must be a string",
                ));
                return None;
            };
            if color.trim().is_empty() {
                warnings.push(warning(format!("{field}.{idx}"), "color is required"));
                return None;
            }
            Some(GradientStop { value, color })
        })
        .collect()
}

fn gradient_stop_value_from_yaml_key(key: Value) -> Option<f64> {
    let value = match key {
        Value::Number(value) => value.as_f64()?,
        Value::String(value) => value.parse::<f64>().ok()?,
        _ => return None,
    };
    value.is_finite().then_some(value)
}

fn validate_sort(raw: RawSortKey, warnings: &mut Vec<SavedViewWarning>) -> Option<SortKey> {
    let direction = parse_sort_direction(&raw.direction).or_else(|| {
        warnings.push(warning(
            "view.sort.direction",
            format!("unknown sort direction '{}'", raw.direction),
        ));
        None
    })?;
    let kind = parse_sort_kind(&raw.kind).or_else(|| {
        warnings.push(warning(
            "view.sort.kind",
            format!("unknown sort kind '{}'", raw.kind),
        ));
        None
    })?;
    Some(SortKey {
        column: raw.column,
        direction,
        kind,
    })
}

fn validate_filter(
    raw: RawSavedFilter,
    warnings: &mut Vec<SavedViewWarning>,
) -> Option<SavedFilter> {
    let action = parse_filter_action(&raw.action).or_else(|| {
        warnings.push(warning(
            "view.filters.action",
            format!("unknown filter action '{}'", raw.action),
        ));
        None
    })?;
    let kind = parse_filter_kind(&raw.kind).or_else(|| {
        warnings.push(warning(
            "view.filters.kind",
            format!("unknown filter kind '{}'", raw.kind),
        ));
        None
    })?;
    if kind == FilterKind::Regex {
        if let Err(err) = Regex::new(&raw.condition) {
            warnings.push(warning(
                "view.filters.condition",
                format!("invalid regex filter condition '{}': {err}", raw.condition),
            ));
            return None;
        }
    }
    Some(SavedFilter {
        column: raw.column,
        action,
        kind,
        condition: raw.condition,
    })
}

fn validate_source_filter(
    raw: RawSavedSourceFilter,
    warnings: &mut Vec<SavedViewWarning>,
) -> Option<SavedSourceFilter> {
    if raw.column.is_empty() {
        warnings.push(warning(
            "source.filters.column",
            "source filter column cannot be empty",
        ));
        return None;
    }
    let operator = parse_source_filter_operator(&raw.operator).or_else(|| {
        warnings.push(warning(
            "source.filters.operator",
            format!("unknown source filter operator '{}'", raw.operator),
        ));
        None
    })?;
    let expects_value = !matches!(
        operator,
        SourceFilterOperator::IsNull | SourceFilterOperator::IsNotNull
    );
    let value = if expects_value {
        let Some(value) = raw.value else {
            warnings.push(warning(
                "source.filters.value",
                "source filter operator requires a non-null scalar value",
            ));
            return None;
        };
        let Some(value) = source_filter_value(value) else {
            warnings.push(warning(
                "source.filters.value",
                "source filter value must be a non-null scalar",
            ));
            return None;
        };
        Some(value)
    } else {
        if raw.value.is_some() {
            warnings.push(warning(
                "source.filters.value",
                "null-test source filter ignores its value",
            ));
        }
        None
    };
    Some(SavedSourceFilter {
        column: raw.column,
        operator,
        value,
    })
}

fn validate_source_sort(
    raw: RawSavedSourceSort,
    warnings: &mut Vec<SavedViewWarning>,
) -> Option<SavedSourceSort> {
    if raw.column.is_empty() {
        warnings.push(warning(
            "source.sort.column",
            "source sort column cannot be empty",
        ));
        return None;
    }
    let direction = parse_sort_direction(&raw.direction).or_else(|| {
        warnings.push(warning(
            "source.sort.direction",
            format!("unknown sort direction '{}'", raw.direction),
        ));
        None
    })?;
    Some(SavedSourceSort {
        column: raw.column,
        direction,
    })
}

fn source_filter_value(value: Value) -> Option<SourceFilterValue> {
    match value {
        Value::Null => None,
        Value::Bool(value) => Some(SourceFilterValue::Boolean(value)),
        Value::Number(value) => value
            .as_i64()
            .map(SourceFilterValue::Integer)
            .or_else(|| value.as_f64().map(SourceFilterValue::Float)),
        Value::String(value) => Some(SourceFilterValue::Text(value)),
        _ => None,
    }
}

fn parse_column_type(value: &str) -> Option<ColumnType> {
    match value {
        "string" | "text" => Some(ColumnType::String(StringKind::Text)),
        "date" => Some(ColumnType::String(StringKind::Date)),
        "ip" => Some(ColumnType::String(StringKind::Ip)),
        "number" | "float" => Some(ColumnType::Number(NumberKind::Float)),
        "integer" => Some(ColumnType::Number(NumberKind::Int)),
        "semver" => Some(ColumnType::Number(NumberKind::SemVer)),
        "boolean" | "word" => Some(ColumnType::Boolean(BooleanKind::Word)),
        "char" => Some(ColumnType::Boolean(BooleanKind::Char)),
        "bit" => Some(ColumnType::Boolean(BooleanKind::Bit)),
        _ => None,
    }
}

fn parse_display_format(value: &str) -> Option<DisplayFormat> {
    match value {
        "plain" => Some(DisplayFormat::Plain),
        "locale" => Some(DisplayFormat::Locale),
        "mask" => Some(DisplayFormat::Mask),
        "uppercase" => Some(DisplayFormat::Uppercase),
        "lowercase" => Some(DisplayFormat::Lowercase),
        "char" => Some(DisplayFormat::Char),
        "bit" => Some(DisplayFormat::Bit),
        "word" => Some(DisplayFormat::Word),
        _ => None,
    }
}

fn parse_number_mask(value: &str) -> Result<NumberMask, &'static str> {
    let (grouped, rest) = if let Some(rest) = value.strip_prefix("#,##") {
        (true, rest)
    } else {
        (false, value)
    };
    let Some(decimal) = rest.strip_prefix('0') else {
        return Err("mask must contain a required 0 digit");
    };
    let decimal_places = if decimal.is_empty() {
        0
    } else if let Some(places) = decimal.strip_prefix('.') {
        if places.is_empty() || !places.chars().all(|ch| ch == '0') {
            return Err("decimal mask must use one or more 0 placeholders");
        }
        places.len()
    } else {
        return Err("unsupported mask syntax");
    };
    Ok(NumberMask {
        raw: value.to_owned(),
        grouped,
        decimal_places,
    })
}

fn parse_width(value: RawColumnWidth) -> Result<ColumnWidth, String> {
    match value {
        RawColumnWidth::Fixed(width) if width > 0 => Ok(ColumnWidth::Fixed(width)),
        RawColumnWidth::Fixed(_) => Err("fixed width must be greater than zero".to_owned()),
        RawColumnWidth::Mode(value) => match value.as_str() {
            "header" => Ok(ColumnWidth::Header),
            "content" => Ok(ColumnWidth::Content),
            "mode" => Ok(ColumnWidth::Mode),
            "max" => Ok(ColumnWidth::Max),
            _ => Err(format!("unknown width '{value}'")),
        },
    }
}

fn parse_align(value: &str) -> Option<ColumnAlign> {
    match value {
        "left" => Some(ColumnAlign::Left),
        "right" => Some(ColumnAlign::Right),
        _ => None,
    }
}

fn parse_null_placement(value: &str) -> Option<NullPlacement> {
    match value {
        "first" => Some(NullPlacement::First),
        "last" => Some(NullPlacement::Last),
        _ => None,
    }
}

impl SavedView {
    pub fn source_options(&self) -> SourceOptionOverrides {
        SourceOptionOverrides {
            format: self.source.format,
            json_path: self.source.json_path.clone(),
            object_mode: self.source.object_mode,
            schema_scan: self.source.schema_scan,
            table: self.source.table.clone(),
            native_query: self.source.query.clone(),
            limit: self.source.limit.and_then(std::num::NonZeroUsize::new),
            source_filters: Some(
                self.source
                    .filters
                    .iter()
                    .map(|filter| SourceFilterRequest {
                        column: filter.column.clone(),
                        operator: match filter.operator {
                            SourceFilterOperator::Equal => TableSourceFilterOperator::Equal,
                            SourceFilterOperator::NotEqual => TableSourceFilterOperator::NotEqual,
                            SourceFilterOperator::LessThan => TableSourceFilterOperator::LessThan,
                            SourceFilterOperator::LessOrEqual => {
                                TableSourceFilterOperator::LessThanOrEqual
                            }
                            SourceFilterOperator::GreaterThan => {
                                TableSourceFilterOperator::GreaterThan
                            }
                            SourceFilterOperator::GreaterOrEqual => {
                                TableSourceFilterOperator::GreaterThanOrEqual
                            }
                            SourceFilterOperator::Contains => TableSourceFilterOperator::Contains,
                            SourceFilterOperator::Prefix => TableSourceFilterOperator::Prefix,
                            SourceFilterOperator::IsNull => TableSourceFilterOperator::IsNull,
                            SourceFilterOperator::IsNotNull => TableSourceFilterOperator::IsNotNull,
                        },
                        operand: filter.value.as_ref().map(|value| match value {
                            SourceFilterValue::Boolean(value) => SourceOperand::Boolean(*value),
                            SourceFilterValue::Integer(value) => SourceOperand::Integer(*value),
                            SourceFilterValue::Float(value) => SourceOperand::Float(*value),
                            SourceFilterValue::Text(value) => SourceOperand::Text(value.clone()),
                        }),
                    })
                    .collect(),
            ),
            source_sort: Some(
                self.source
                    .sort
                    .iter()
                    .map(|sort| SourceSortRequest {
                        column: sort.column.clone(),
                        direction: match sort.direction {
                            SortDirection::Asc => TableSortDirection::Ascending,
                            SortDirection::Desc => TableSortDirection::Descending,
                        },
                    })
                    .collect(),
            ),
        }
    }

    pub fn merged_open_options(
        &self,
        defaults: OpenOptions,
        cli: &SourceOptionOverrides,
    ) -> OpenOptions {
        OpenOptions::merge(defaults, &self.source_options(), cli)
    }
}

fn parse_sort_direction(value: &str) -> Option<SortDirection> {
    match value {
        "asc" => Some(SortDirection::Asc),
        "desc" => Some(SortDirection::Desc),
        _ => None,
    }
}

fn parse_source_filter_operator(value: &str) -> Option<SourceFilterOperator> {
    match value {
        "equal" => Some(SourceFilterOperator::Equal),
        "not_equal" => Some(SourceFilterOperator::NotEqual),
        "less_than" => Some(SourceFilterOperator::LessThan),
        "less_or_equal" => Some(SourceFilterOperator::LessOrEqual),
        "greater_than" => Some(SourceFilterOperator::GreaterThan),
        "greater_or_equal" => Some(SourceFilterOperator::GreaterOrEqual),
        "contains" => Some(SourceFilterOperator::Contains),
        "prefix" => Some(SourceFilterOperator::Prefix),
        "is_null" => Some(SourceFilterOperator::IsNull),
        "is_not_null" => Some(SourceFilterOperator::IsNotNull),
        _ => None,
    }
}

fn parse_sort_kind(value: &str) -> Option<SortKind> {
    match value {
        "lexical" => Some(SortKind::Lexical),
        "natural" => Some(SortKind::Natural),
        "numeric" => Some(SortKind::Numeric),
        "type" => Some(SortKind::Type),
        _ => None,
    }
}

fn parse_filter_action(value: &str) -> Option<FilterAction> {
    match value {
        "in" => Some(FilterAction::In),
        "out" => Some(FilterAction::Out),
        _ => None,
    }
}

fn parse_filter_kind(value: &str) -> Option<FilterKind> {
    match value {
        "text" => Some(FilterKind::Text),
        "regex" => Some(FilterKind::Regex),
        "numeric" => Some(FilterKind::Numeric),
        _ => None,
    }
}

fn is_posix_locale(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-' | '@'))
}

fn is_wildcard_pattern(value: &str) -> bool {
    value.contains('*') || value.contains('?') || value.contains('[')
}

fn warning(field: impl Into<String>, message: impl Into<String>) -> SavedViewWarning {
    SavedViewWarning {
        field: field.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_saved_view_with_aliases_sort_and_filters() {
        let parsed = parse_saved_view_yaml(
            r##"
name: shards
filenames:
  - cat_shards.txt
  - "*shards*"
source: {}
view:
  locale: en_US
  columns:
    Index:
      type: string
      width: 20
    docs:
      type: integer
      format: mask
      mask: "#,##0"
      visible: false
  sort:
    - column: docs
      direction: desc
      kind: numeric
  filters:
    - column: docs
      action: in
      kind: numeric
      condition: ">0"
"##,
        )
        .expect("parse");

        assert!(parsed.warnings.is_empty());
        assert_eq!(parsed.view.name, "shards");
        assert_eq!(parsed.view.view.locale.as_deref(), Some("en_US"));
        assert_eq!(parsed.view.filenames[0].kind, FilenamePatternKind::Exact);
        assert_eq!(parsed.view.filenames[1].kind, FilenamePatternKind::Glob);
        assert_eq!(
            parsed
                .view
                .view
                .columns
                .get("docs")
                .expect("docs")
                .column_type,
            Some(ColumnType::Number(NumberKind::Int))
        );
        assert_eq!(parsed.view.view.sort.len(), 1);
        assert_eq!(parsed.view.view.filters.len(), 1);
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn parses_nested_source_query_and_view_configuration() {
        let parsed = parse_saved_view_yaml(
            r#"
name: recent
filenames: [transactions.db]
source:
  format: sqlite
  table: transactions
  limit: 2500
  filters:
    - column: settled
      operator: equal
      value: true
    - column: deleted_at
      operator: is_null
  sort:
    - column: timestamp
      direction: desc
view:
  locale: en_US
  nulls: last
  filters:
    - column: description
      action: in
      kind: regex
      condition: '^invoice-'
  sort:
    - column: user
      direction: asc
      kind: natural
"#,
        )
        .expect("parse");

        assert!(parsed.warnings.is_empty());
        assert_eq!(parsed.view.source.format, Some(InputFormat::Sqlite));
        assert_eq!(parsed.view.source.table.as_deref(), Some("transactions"));
        assert_eq!(parsed.view.source.limit, Some(2500));
        assert_eq!(parsed.view.source.filters.len(), 2);
        assert_eq!(
            parsed.view.source.filters[0].value,
            Some(SourceFilterValue::Boolean(true))
        );
        assert_eq!(parsed.view.source.sort.len(), 1);
        assert_eq!(parsed.view.view.filters.len(), 1);
        assert_eq!(parsed.view.view.sort.len(), 1);
    }

    #[cfg(feature = "elasticsearch")]
    #[test]
    fn elasticsearch_query_round_trips_and_cli_query_overrides_saved_query() {
        let parsed = parse_saved_view_yaml(
            r#"
name: errors
filenames: [https_elastic.example_9200]
source:
  format: elasticsearch
  query: 'FROM logs-* | WHERE log.level == "error"'
  limit: 25
view: {}
"#,
        )
        .expect("parse ES|QL saved view");
        assert!(parsed.warnings.is_empty());
        assert_eq!(
            parsed.view.source.query.as_deref(),
            Some(r#"FROM logs-* | WHERE log.level == "error""#)
        );
        let merged = parsed.view.merged_open_options(
            OpenOptions::default(),
            &SourceOptionOverrides {
                native_query: Some("FROM audit-* | LIMIT 5".to_owned()),
                ..SourceOptionOverrides::default()
            },
        );
        assert_eq!(
            merged.native_query.as_deref(),
            Some("FROM audit-* | LIMIT 5")
        );
        assert_eq!(merged.limit.unwrap().get(), 25);
    }

    #[test]
    fn saved_table_and_query_conflict_is_reported_without_choosing_one() {
        let parsed = parse_saved_view_yaml(
            r#"
name: conflict
filenames: [source]
source:
  table: logs
  query: FROM logs
view: {}
"#,
        )
        .expect("parse conflict");
        assert_eq!(parsed.view.source.table.as_deref(), Some("logs"));
        assert_eq!(parsed.view.source.query.as_deref(), Some("FROM logs"));
        assert!(parsed
            .warnings
            .iter()
            .any(|warning| warning.message.contains("mutually exclusive")));
        assert!(parsed
            .view
            .merged_open_options(OpenOptions::default(), &SourceOptionOverrides::default())
            .validate()
            .is_err());
    }

    #[cfg(not(feature = "elasticsearch"))]
    #[test]
    fn feature_disabled_saved_elasticsearch_format_is_compatible_but_unavailable() {
        let parsed = parse_saved_view_yaml(
            r#"
name: errors
filenames: [elastic]
source:
  format: elasticsearch
  query: FROM logs-*
view: {}
"#,
        )
        .expect("parse");
        assert_eq!(parsed.view.source.format, None);
        assert_eq!(parsed.view.source.query.as_deref(), Some("FROM logs-*"));
        assert!(parsed
            .warnings
            .iter()
            .any(|warning| warning.field == "source.format"));
    }

    #[test]
    fn explicit_null_source_operand_is_rejected_by_runtime_and_schema() {
        let parsed = parse_saved_view_yaml(
            r#"
name: null-operand
filenames: [data.csv]
source:
  filters:
    - column: deleted_at
      operator: equal
      value: null
view: {}
"#,
        )
        .expect("parse");

        assert!(parsed.view.source.filters.is_empty());
        assert!(parsed.warnings.iter().any(|warning| {
            warning.field == "source.filters.value" && warning.message.contains("non-null scalar")
        }));

        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../schemas/view.schema.json"))
                .expect("saved view schema");
        assert_eq!(
            schema.pointer("/$defs/sourceFilter/allOf/0/else/properties/value/not/type"),
            Some(&serde_json::Value::String("null".to_owned()))
        );
    }

    #[test]
    fn rejects_legacy_flat_saved_view_fields() {
        let error = parse_saved_view_yaml(
            "name: legacy\nfilenames: [data.csv]\ncolumns: {}\nsource: {}\nview: {}\n",
        )
        .expect_err("legacy root field must fail");

        assert!(error.to_string().contains("unknown field `columns`"));
    }

    #[test]
    fn parses_and_merges_source_label_and_null_options() {
        let parsed = parse_saved_view_yaml(
            r#"
name: elastic
filenames: [response.json]
source:
  format: json
  json_path: /hits/hits
  schema_scan: full
view:
  nulls: first
  columns:
    /_source/user/email:
      label: User email
      nulls: last
"#,
        )
        .expect("parse");

        assert!(parsed.warnings.is_empty());
        assert_eq!(parsed.view.source.format, Some(InputFormat::Json));
        assert_eq!(
            parsed
                .view
                .source
                .json_path
                .as_ref()
                .expect("path")
                .segments(),
            ["hits", "hits"]
        );
        assert_eq!(parsed.view.source.schema_scan, Some(SchemaScan::Full));
        assert_eq!(parsed.view.view.nulls, Some(NullPlacement::First));
        let email = parsed
            .view
            .view
            .columns
            .get("/_source/user/email")
            .expect("email");
        assert_eq!(email.label.as_deref(), Some("User email"));
        assert_eq!(email.nulls, Some(NullPlacement::Last));

        let merged = parsed.view.merged_open_options(
            OpenOptions::default(),
            &SourceOptionOverrides {
                schema_scan: Some(SchemaScan::Default),
                ..SourceOptionOverrides::default()
            },
        );
        assert_eq!(merged.format, InputFormat::Json);
        assert_eq!(merged.schema_scan, SchemaScan::Default);
    }

    #[test]
    fn object_mode_validates_merges_and_warns_for_row_streams() {
        let parsed = parse_saved_view_yaml(
            r#"
name: keyed
filenames: [repositories.json]
source:
  format: json
  object_mode: record
view: {}
"#,
        )
        .expect("parse");
        assert!(parsed.warnings.is_empty());
        assert_eq!(parsed.view.source.object_mode, Some(ObjectMode::Record));
        let merged = parsed.view.merged_open_options(
            OpenOptions::default(),
            &SourceOptionOverrides {
                object_mode: Some(ObjectMode::Entries),
                ..SourceOptionOverrides::default()
            },
        );
        assert_eq!(merged.object_mode, ObjectMode::Entries);
        assert_eq!(
            merged.object_mode_origin,
            crate::ingest::ObjectModeOrigin::Cli
        );

        let invalid = parse_saved_view_yaml(
            "name: bad\nfilenames: [data]\nsource:\n  object_mode: rows\nview: {}\n",
        )
        .expect("parse invalid");
        assert_eq!(invalid.view.source.object_mode, None);
        assert!(invalid
            .warnings
            .iter()
            .any(|warning| warning.field == "source.object_mode"));

        let row_stream = parse_saved_view_yaml(
            "name: stream\nfilenames: [rows.ndjson]\nsource:\n  format: ndjson\n  object_mode: entries\nview: {}\n",
        )
        .expect("parse stream");
        assert_eq!(row_stream.view.source.object_mode, None);
        assert!(row_stream
            .warnings
            .iter()
            .any(|warning| warning.message.contains("row-stream")));
    }

    #[test]
    fn saved_entries_and_record_modes_apply_before_table_construction() {
        use crate::ingest::SourceAdapter;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("objects.json");
        std::fs::write(&path, r#"{"first":{"value":1},"second":{"value":2}}"#).expect("write");

        for (mode, expected_rows) in [
            (ObjectMode::Entries, crate::table::RowCount::Exact(2)),
            (ObjectMode::Record, crate::table::RowCount::Exact(1)),
        ] {
            let parsed = parse_saved_view_yaml(&format!(
                "name: objects\nfilenames: [objects.json]\nsource:\n  format: json\n  object_mode: {mode}\nview: {{}}\n"
            ))
            .expect("saved view");
            let options = parsed
                .view
                .merged_open_options(OpenOptions::default(), &SourceOptionOverrides::default());
            assert_eq!(
                options.object_mode_origin,
                crate::ingest::ObjectModeOrigin::SavedView
            );
            let table = crate::ingest::JsonAdapter::json()
                .open(
                    crate::ingest::source::InputSource::Path(path.clone()),
                    &options,
                )
                .expect("open")
                .into_implicit_table()
                .expect("table");
            assert_eq!(table.store.row_count(), expected_rows);
            assert_eq!(table.object_mode.unwrap().requested, mode);
        }
    }

    #[test]
    fn invalid_source_and_null_values_warn_non_fatally() {
        let parsed = parse_saved_view_yaml(
            r#"
name: bad-source
filenames: [data]
source:
  format: parquet
  json_path: hits/hits
  schema_scan: endless
view:
  nulls: middle
  columns:
    a:
      label: ""
      nulls: middle
"#,
        )
        .expect("parse");
        assert_eq!(parsed.warnings.len(), 6);
        assert_eq!(parsed.view.source.format, None);
        assert_eq!(parsed.view.source.json_path, None);
        assert_eq!(parsed.view.source.schema_scan, None);
        assert_eq!(parsed.view.view.nulls, None);
    }

    #[test]
    fn invalid_semantic_values_are_warnings() {
        let parsed = parse_saved_view_yaml(
            r#"
name: bad
filenames:
  - "[broken"
source: {}
view:
  locale: "?"
  columns:
    "*count":
      type: text
      format: mask
      mask: "bad"
  sort:
    - column: count
      direction: sideways
      kind: numeric
  filters:
    - column: name
      action: in
      kind: regex
      condition: "["
"#,
        )
        .expect("parse");

        assert!(parsed.view.filenames.is_empty());
        assert!(parsed.view.view.sort.is_empty());
        assert!(parsed.view.view.filters.is_empty());
        assert!(parsed.warnings.len() >= 5);
    }

    #[test]
    fn limits_sort_keys_to_three() {
        let parsed = parse_saved_view_yaml(
            r#"
name: sort
filenames: [data.csv]
source: {}
view:
  sort:
    - { column: a, direction: asc, kind: lexical }
    - { column: b, direction: asc, kind: lexical }
    - { column: c, direction: asc, kind: lexical }
    - { column: d, direction: asc, kind: lexical }
"#,
        )
        .expect("parse");

        assert_eq!(parsed.view.view.sort.len(), MAX_SORT_KEYS);
        assert!(parsed
            .warnings
            .iter()
            .any(|warning| warning.field == "view.sort"));
    }

    #[test]
    fn saved_view_dir_uses_tview_views_under_config_root() {
        assert_eq!(
            saved_view_dir(Some(Path::new("/tmp/config"))),
            Some(PathBuf::from("/tmp/config/tview/views"))
        );
    }

    #[test]
    fn discovers_yml_before_yaml_duplicate_stems() {
        let dir = tempfile::tempdir().expect("tempdir");
        let views = dir.path().join("views");
        std::fs::create_dir(&views).expect("views dir");
        std::fs::write(
            views.join("cat-shards.yaml"),
            "name: cat-shards\nfilenames: [ignored.txt]\nsource: {}\nview: {}\n",
        )
        .expect("write yaml");
        std::fs::write(
            views.join("cat-shards.yml"),
            "name: cat-shards\nfilenames: [cat_shards.txt]\nsource: {}\nview: {}\n",
        )
        .expect("write yml");

        let discovered = discover_saved_views_in_dir(&views);

        assert_eq!(discovered.views.len(), 1);
        assert_eq!(discovered.views[0].path, views.join("cat-shards.yml"));
        assert_eq!(discovered.views[0].canonical_name, "cat-shards");
        assert_eq!(discovered.warnings.len(), 1);
    }

    #[test]
    fn discovers_nested_bundles_and_selects_by_stem_and_basename() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle = dir.path().join("tview/views/elasticsearch/nodes");
        std::fs::create_dir_all(&bundle).expect("bundle dir");
        let path = bundle.join("cat_nodes.yml");
        std::fs::write(
            &path,
            "name: nodes\nfilenames: [cat_nodes.txt]\nsource: {}\nview: {}\n",
        )
        .expect("write view");
        std::fs::write(bundle.join("README.md"), "Not a view").expect("write readme");
        let discovered = discover_saved_views(Some(dir.path()));
        assert_eq!(discovered.views.len(), 1);
        assert!(discovered.warnings.is_empty());
        for selection in [
            SavedViewSelection::Force { name: "cat_nodes" },
            SavedViewSelection::Auto {
                input_path: Path::new("/tmp/cat_nodes.txt"),
            },
        ] {
            let selected = select_saved_view(&discovered.views, selection).expect("selected");
            assert_eq!(selected.view.path, path);
        }
    }

    #[test]
    fn nested_duplicate_stems_have_deterministic_precedence() {
        let dir = tempfile::tempdir().expect("tempdir");
        for relative in ["a/shared.yaml", "b/shared.yml", "c/shared.yml"] {
            let path = dir.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).expect("bundle dir");
            std::fs::write(
                path,
                "name: shared\nfilenames: [data.csv]\nsource: {}\nview: {}\n",
            )
            .expect("write view");
        }
        let discovered = discover_saved_views_in_dir(dir.path());
        assert_eq!(discovered.views.len(), 1);
        assert_eq!(discovered.views[0].path, dir.path().join("b/shared.yml"));
        assert_eq!(discovered.warnings.len(), 2);
    }

    #[test]
    fn duplicate_stems_follow_platform_case_semantics() {
        let dir = tempfile::tempdir().expect("tempdir");
        for relative in ["a/Foo.yaml", "b/foo.yml"] {
            let path = dir.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).expect("bundle dir");
            std::fs::write(
                path,
                "name: foo\nfilenames: [data.csv]\nsource: {}\nview: {}\n",
            )
            .expect("write view");
        }
        let discovered = discover_saved_views_in_dir(dir.path());
        if platform_case_insensitive() {
            assert_eq!(discovered.views.len(), 1);
            assert_eq!(discovered.views[0].path, dir.path().join("b/foo.yml"));
            assert_eq!(discovered.warnings.len(), 1);
        } else {
            assert_eq!(discovered.views.len(), 2);
            assert!(discovered.warnings.is_empty());
        }
    }

    #[cfg(unix)]
    #[test]
    fn discovery_does_not_follow_directory_symlink_cycles() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::os::unix::fs::symlink(dir.path(), dir.path().join("cycle"))
            .expect("directory symlink");
        let discovered = discover_saved_views_in_dir(dir.path());
        assert!(discovered.views.is_empty());
        assert!(discovered.warnings.is_empty());
    }

    #[test]
    fn malformed_saved_view_warns_without_blocking_valid_views() {
        let dir = tempfile::tempdir().expect("tempdir");
        let views = dir.path().join("views");
        std::fs::create_dir(&views).expect("views dir");
        std::fs::write(views.join("bad.yml"), "name: [").expect("write bad");
        std::fs::write(
            views.join("good.yml"),
            "name: good\nfilenames: [data.csv]\nsource: {}\nview: {}\n",
        )
        .expect("write good");

        let discovered = discover_saved_views_in_dir(&views);

        assert_eq!(discovered.views.len(), 1);
        assert_eq!(discovered.views[0].canonical_name, "good");
        assert_eq!(discovered.warnings.len(), 1);
    }

    #[test]
    fn selects_exact_before_glob_before_regex() {
        let dir = tempfile::tempdir().expect("tempdir");
        let views = dir.path().join("views");
        std::fs::create_dir(&views).expect("views dir");
        std::fs::write(
            views.join("regex.yml"),
            "name: regex\nfilenames: ['^cat_.*txt$']\nsource: {}\nview: {}\n",
        )
        .expect("write regex");
        std::fs::write(
            views.join("glob.yml"),
            "name: glob\nfilenames: ['*shards*']\nsource: {}\nview: {}\n",
        )
        .expect("write glob");
        std::fs::write(
            views.join("exact.yml"),
            "name: exact\nfilenames: [cat_shards.txt]\nsource: {}\nview: {}\n",
        )
        .expect("write exact");
        let discovered = discover_saved_views_in_dir(&views);

        let selected = select_saved_view(
            &discovered.views,
            SavedViewSelection::Auto {
                input_path: Path::new("/tmp/cat_shards.txt"),
            },
        )
        .expect("selected");

        assert_eq!(selected.view.canonical_name, "exact");
    }

    #[test]
    fn force_selection_normalizes_yaml_extension() {
        let view = SavedViewFile {
            path: PathBuf::from("cat-shards.yml"),
            canonical_name: "cat-shards".to_owned(),
            view: SavedView {
                name: "cat-shards".to_owned(),
                filenames: Vec::new(),
                source: SavedSourceConfig::default(),
                view: SavedViewConfig::default(),
            },
            warnings: Vec::new(),
        };

        let views = [view];
        let selected = select_saved_view(
            &views,
            SavedViewSelection::Force {
                name: "cat-shards.yaml",
            },
        )
        .expect("selected");

        assert_eq!(selected.view.canonical_name, "cat-shards");
    }

    #[test]
    fn invocation_selection_retains_one_validated_file_across_external_edits() {
        let root = tempfile::tempdir().expect("config root");
        let bundle = root.path().join("tview/views/nested");
        std::fs::create_dir_all(&bundle).expect("view bundle");
        let path = bundle.join("chosen.yml");
        std::fs::write(
            &path,
            "name: chosen\nfilenames: [data.csv]\nsource:\n  limit: 2\nview:\n  columns:\n    name:\n      label: Original\n",
        )
        .expect("initial YAML");
        let input = Path::new("data.csv");
        let first = prepare_saved_view(
            Some(SavedViewSelection::Force {
                name: "chosen.yaml",
            }),
            input,
            Some(root.path()),
        )
        .expect("forced selection");
        std::fs::remove_file(&path).expect("remove after selection");
        let SavedViewInvocation::Enabled {
            selected: Some(snapshot),
            target_path,
            warnings,
            ..
        } = first
        else {
            panic!("selected saved view");
        };
        assert_eq!(snapshot.path, path);
        assert_eq!(snapshot.canonical_name, "chosen");
        assert_eq!(target_path, Some(root.path().join("tview/views/data.yml")));
        assert!(warnings.is_empty());
        assert_eq!(snapshot.view.source.limit, Some(2));
        assert_eq!(
            snapshot.view.view.columns["name"].label.as_deref(),
            Some("Original")
        );
        assert!(prepare_saved_view(
            Some(SavedViewSelection::Force { name: "chosen" }),
            input,
            Some(root.path()),
        )
        .is_err());
        std::fs::write(
            &path,
            "name: chosen\nfilenames: [data.csv]\nsource:\n  limit: 4\nview:\n  columns:\n    name:\n      label: Updated\n",
        ).expect("updated YAML");
        let SavedViewInvocation::Enabled {
            selected: Some(fresh),
            ..
        } = prepare_saved_view(
            Some(SavedViewSelection::Auto { input_path: input }),
            input,
            Some(root.path()),
        )
        .expect("fresh invocation")
        else {
            panic!("fresh selection")
        };
        assert_eq!(fresh.view.source.limit, Some(4));
        assert_eq!(
            fresh.view.view.columns["name"].label.as_deref(),
            Some("Updated")
        );
    }

    #[test]
    fn disabled_invocation_neither_discovers_malformed_files_nor_prepares_authoring() {
        let root = tempfile::tempdir().expect("config root");
        let views = root.path().join("tview/views");
        std::fs::create_dir_all(&views).expect("views");
        std::fs::write(views.join("bad.yml"), "name: [").expect("bad YAML");
        assert_eq!(
            prepare_saved_view(None, Path::new("bad.csv"), Some(root.path())),
            Ok(SavedViewInvocation::Disabled)
        );
        let SavedViewInvocation::Enabled {
            selected,
            target_path,
            warnings,
            ..
        } = prepare_saved_view(
            Some(SavedViewSelection::Auto {
                input_path: Path::new("bad.csv"),
            }),
            Path::new("bad.csv"),
            Some(root.path()),
        )
        .expect("automatic selection")
        else {
            panic!("enabled invocation")
        };
        assert!(selected.is_none());
        assert_eq!(target_path, Some(views.join("bad.yml")));
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn elasticsearch_saved_view_path_opens_hits_and_resolves_canonical_columns() {
        use crate::ingest::SourceAdapter;

        let parsed = parse_saved_view_yaml(
            r#"
name: elasticsearch hits
filenames: [elasticsearch-response.json]
source:
  format: json
  json_path: /hits/hits
view:
  columns:
    /_source/user/id:
      label: User ID
"#,
        )
        .expect("saved view");
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples/data/json/elasticsearch-response.json");
        let options = parsed
            .view
            .merged_open_options(OpenOptions::default(), &SourceOptionOverrides::default());
        let table = crate::ingest::JsonAdapter::json()
            .open(crate::ingest::source::InputSource::Path(fixture), &options)
            .expect("open fixture")
            .into_implicit_table()
            .expect("table");

        let user_id = table
            .definition
            .columns
            .iter()
            .position(|column| {
                matches!(
                    &column.source_identity,
                    ColumnSourceIdentity::StructuredPath(pointer)
                        if pointer.as_str() == "/_source/user/id"
                )
            })
            .expect("user id column");
        assert!(!table.definition.columns.iter().any(|column| {
            matches!(
                &column.source_identity,
                ColumnSourceIdentity::StructuredPath(pointer)
                    if pointer.as_str().contains("took") || pointer.as_str().contains("total")
            )
        }));
        let mut view =
            crate::view::TableView::from_opened_table(table, crate::view::Viewport::new(10, 20))
                .expect("view");
        view.install_saved_binding(parsed.view.view, true);
        assert_eq!(
            view.header()
                .and_then(|header| header.get(user_id))
                .map(String::as_str),
            Some("User ID")
        );
    }

    #[test]
    fn parses_conditional_color_rules_and_warns_for_invalid_rules() {
        let parsed = parse_saved_view_yaml(
            r##"
name: colors
filenames: [data.csv]
source: {}
view:
  columns:
    health:
      colors:
        - match:
            true: green
            false: muted
            "": red
        - range:
            "<10": red
            ">=90": red
            ">=50 <75": yellow
        - gradient:
            mode: fixed
            stops:
              10: green
              "90": yellow
        - gradient:
            mode: auto
            steps: 5
            colors: [green, yellow, red]
        - identifiers:
            colors: auto
        - identifiers:
            colors: [green, "#ff00ffff"]
        - range:
            nope: red
"##,
        )
        .expect("parse");

        let colors = &parsed
            .view
            .view
            .columns
            .get("health")
            .expect("health")
            .colors;
        assert_eq!(colors.len(), 6);
        assert_eq!(
            colors[0],
            ConditionalColorRule::Match {
                entries: vec![
                    MatchEntry {
                        value: ConditionalValue::Bool(true),
                        color: "green".to_owned(),
                    },
                    MatchEntry {
                        value: ConditionalValue::Bool(false),
                        color: "muted".to_owned(),
                    },
                ]
            }
        );
        assert_eq!(
            colors[1],
            ConditionalColorRule::Range {
                entries: vec![
                    RangeEntry {
                        lt: Some(10.0),
                        lte: None,
                        gt: None,
                        gte: None,
                        color: "red".to_owned(),
                    },
                    RangeEntry {
                        lt: None,
                        lte: None,
                        gt: None,
                        gte: Some(90.0),
                        color: "red".to_owned(),
                    },
                    RangeEntry {
                        lt: Some(75.0),
                        lte: None,
                        gt: None,
                        gte: Some(50.0),
                        color: "yellow".to_owned(),
                    },
                ]
            }
        );
        assert_eq!(
            colors[2],
            ConditionalColorRule::FixedGradient {
                stops: vec![
                    GradientStop {
                        value: 10.0,
                        color: "green".to_owned(),
                    },
                    GradientStop {
                        value: 90.0,
                        color: "yellow".to_owned(),
                    },
                ]
            }
        );
        assert!(parsed
            .warnings
            .iter()
            .any(|warning| warning.field.contains("colors.0.match")));
        assert!(parsed
            .warnings
            .iter()
            .any(|warning| warning.field.contains("colors.6.range")));
        assert_eq!(
            colors[5],
            ConditionalColorRule::Identifiers {
                colors: IdentifierColors::Colors(vec!["green".to_owned(), "#ff00ffff".to_owned()])
            }
        );
    }

    #[test]
    fn segments_example_uses_ip_metadata_and_numeric_size_sorting() {
        let parsed = parse_saved_view_yaml(include_str!(
            "../../examples/views/elasticsearch/cat_segments.yml"
        ))
        .expect("segments saved view");
        assert!(parsed.warnings.is_empty());
        assert_eq!(
            parsed.view.view.columns.get("ip").expect("ip").column_type,
            Some(ColumnType::String(StringKind::Ip))
        );
        assert_eq!(parsed.view.view.sort[0].column, "size.memory");
        assert_eq!(parsed.view.view.sort[0].kind, SortKind::Numeric);
        assert_eq!(parsed.view.view.sort[0].direction, SortDirection::Desc);
    }

    #[test]
    fn ohlcv_examples_sort_volume_numerically() {
        for input in [
            include_str!("../../examples/views/data_ohlcv.yml"),
            include_str!("../../examples/data/data_ohlcv.view.yml"),
        ] {
            let parsed = parse_saved_view_yaml(input).expect("ohlcv saved view");
            assert!(parsed.warnings.is_empty());
            let sort = parsed
                .view
                .view
                .sort
                .iter()
                .find(|sort| sort.column == "Volume")
                .expect("volume sort");
            assert_eq!(sort.kind, SortKind::Numeric);
            assert_eq!(sort.direction, SortDirection::Desc);
        }
    }

    #[test]
    fn sample_conditional_colors_fixture_parses() {
        let parsed = parse_saved_view_yaml(include_str!(
            "../../examples/data/config/views/conditional-colors.yml"
        ))
        .expect("sample saved view");

        assert_eq!(parsed.view.name, "conditional-colors");
        assert!(parsed.warnings.is_empty());
        assert_eq!(
            parsed
                .view
                .view
                .columns
                .get("used_percent")
                .expect("used_percent")
                .colors
                .len(),
            2
        );
    }
}
