use std::io::Read;
use std::path::Path;

use crate::table::{RelationMetadata, SourceGeneration, TableDefinition, TableStore};

use super::source::InputSource;
#[cfg(feature = "elasticsearch")]
use super::ElasticsearchAdapter;
#[cfg(feature = "sqlite")]
use super::SqliteAdapter;
use super::{DelimitedAdapter, JsonAdapter};
use super::{InputFormat, ObjectMode, ObjectModeOrigin, ObjectModeResolution, OpenOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProbeResult {
    NoMatch,
    Possible,
    Strong,
}

pub struct OpenedTable {
    pub generation: SourceGeneration,
    pub definition: TableDefinition,
    pub store: Box<dyn TableStore>,
    pub object_mode: Option<ObjectModeResolution>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationKind {
    Table,
    View,
    VirtualTable,
    Index,
    DataStream,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelationAvailability {
    Selectable,
    Unavailable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationCatalogEntry {
    pub metadata: RelationMetadata,
    pub kind: RelationKind,
    pub availability: RelationAvailability,
}

impl RelationCatalogEntry {
    pub fn is_selectable(&self) -> bool {
        matches!(self.availability, RelationAvailability::Selectable)
    }
}

pub trait RelationOpener {
    fn open_relation(&mut self, name: &str) -> anyhow::Result<OpenedTable>;
}

pub struct OpenedSource {
    relations: Vec<RelationCatalogEntry>,
    tables: Vec<OpenedTable>,
    relation_opener: Option<Box<dyn RelationOpener>>,
}

impl OpenedSource {
    pub fn implicit(table: OpenedTable) -> Self {
        Self {
            relations: vec![RelationCatalogEntry {
                metadata: table.definition.relation.clone(),
                kind: RelationKind::Table,
                availability: RelationAvailability::Selectable,
            }],
            tables: vec![table],
            relation_opener: None,
        }
    }

    pub fn relational(
        relations: Vec<RelationCatalogEntry>,
        selected: Option<OpenedTable>,
        opener: Box<dyn RelationOpener>,
    ) -> Self {
        Self {
            relations,
            tables: selected.into_iter().collect(),
            relation_opener: Some(opener),
        }
    }

    pub fn list_relations(&self) -> &[RelationCatalogEntry] {
        &self.relations
    }

    pub fn selectable_relations(&self) -> impl Iterator<Item = &RelationCatalogEntry> {
        self.relations
            .iter()
            .filter(|relation| relation.is_selectable())
    }

    pub fn has_selected_table(&self) -> bool {
        self.tables.len() == 1
    }

    pub fn open_relation(&mut self, name: &str) -> anyhow::Result<()> {
        let Some(entry) = self
            .relations
            .iter()
            .find(|entry| entry.metadata.name == name)
        else {
            anyhow::bail!("relation '{name}' was not found");
        };
        if let RelationAvailability::Unavailable { reason } = &entry.availability {
            anyhow::bail!("relation '{name}' is unavailable: {reason}");
        }
        let opener = self
            .relation_opener
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("source does not support relation selection"))?;
        let table = opener.open_relation(name)?;
        self.tables.clear();
        self.tables.push(table);
        Ok(())
    }

    pub fn into_implicit_table(mut self) -> anyhow::Result<OpenedTable> {
        if self.tables.is_empty() {
            let selectable = self
                .selectable_relations()
                .map(|entry| entry.metadata.name.as_str())
                .collect::<Vec<_>>();
            if selectable.len() > 1 {
                anyhow::bail!(
                    "SQLite database has multiple selectable relations ({}); select one with --table or saved source.table",
                    selectable.join(", ")
                );
            }
            anyhow::bail!("source has no selected relation");
        }
        if self.tables.len() != 1 {
            anyhow::bail!("source contains more than one opened relation");
        }
        Ok(self.tables.remove(0))
    }
}

pub trait SourceAdapter {
    fn format(&self) -> InputFormat;
    fn probe(&self, source: &InputSource, sample: &[u8]) -> ProbeResult;
    fn open(&self, source: InputSource, options: &OpenOptions) -> anyhow::Result<OpenedSource>;
}

pub struct FormatResolver;

impl FormatResolver {
    pub fn resolve(requested: InputFormat, source: &InputSource, sample: &[u8]) -> InputFormat {
        if requested != InputFormat::Auto {
            return requested;
        }

        if let InputSource::Path(path) = source {
            if let Some(format) = format_from_extension(path) {
                return format;
            }
        }

        #[cfg(feature = "sqlite")]
        if let InputSource::Url(url) = source {
            if url.scheme().eq_ignore_ascii_case("libsql") {
                return InputFormat::Sqlite;
            }
        }

        probe_content(sample)
    }
}

pub fn open_source(source: InputSource, options: &OpenOptions) -> anyhow::Result<OpenedSource> {
    let detected = match &source {
        InputSource::Path(path) => {
            let mut sample = Vec::new();
            std::fs::File::open(path)?
                .take(64 * 1024)
                .read_to_end(&mut sample)?;
            resolve_format(options, &source, &sample)
        }
        InputSource::Stdin => {
            // Stdin is consumed exactly once by the selected adapter. With no
            // explicit or saved-view format, preserve the historical
            // delimited default rather than attempting a destructive probe.
            if options.format == InputFormat::Auto {
                InputFormat::Delimited
            } else {
                options.format
            }
        }
        InputSource::StreamingStdin(input) => {
            if options.format != InputFormat::Auto {
                options.format
            } else {
                input.wait_for_probe_sample()?;
                let snapshot = input.snapshot(false)?;
                let sample_len = snapshot.bytes.len().min(64 * 1024);
                resolve_format(options, &source, &snapshot.bytes[..sample_len])
            }
        }
        InputSource::Url(url) => {
            if options.format != InputFormat::Auto {
                options.format
            } else if url.scheme().eq_ignore_ascii_case("libsql") {
                #[cfg(feature = "sqlite")]
                {
                    InputFormat::Sqlite
                }
                #[cfg(not(feature = "sqlite"))]
                {
                    anyhow::bail!("URL scheme 'libsql' requires a build with SQLite support")
                }
            } else if matches!(url.scheme(), "http" | "https") {
                anyhow::bail!(
                    "remote HTTP(S) target '{}' requires an explicit --format",
                    source.safe_identity()
                )
            } else {
                anyhow::bail!(
                    "URL scheme '{}' has no registered input format",
                    url.scheme()
                )
            }
        }
    };
    // A JSON Pointer is itself an explicit request for structured parsing. If
    // auto-detection cannot identify JSON/NDJSON, prefer JSON so the option is
    // either honored or produces a parse error instead of being silently
    // ignored by the delimited adapter.
    let resolved = resolve_structured_options(detected, options);
    let sqlite = is_sqlite_format(resolved);
    let elasticsearch = is_elasticsearch_format(resolved);
    if options.table.is_some() && options.native_query.is_some() {
        anyhow::bail!("source table and native query are mutually exclusive");
    }
    if !sqlite && !elasticsearch && options.table.is_some() {
        anyhow::bail!("the resolved {resolved} source does not support relation selection");
    }
    if !sqlite && !elasticsearch && options.native_query.is_some() {
        anyhow::bail!("the resolved {resolved} source does not support native queries");
    }
    if sqlite {
        if has_delimited_options(options) {
            anyhow::bail!(
                "encoding, delimiter, quoting, and quote-character options cannot be used with SQLite input"
            );
        }
        if options.json_path.is_some() {
            anyhow::bail!("JSON starting paths cannot be used with SQLite input");
        }
    }
    if elasticsearch {
        if !matches!(&source, InputSource::Url(url) if matches!(url.scheme(), "http" | "https")) {
            anyhow::bail!("Elasticsearch input requires an HTTP(S) endpoint target");
        }
        if has_delimited_options(options)
            || options.json_path.is_some()
            || options.object_mode != ObjectMode::Auto
        {
            anyhow::bail!(
                "delimited and structured-file parsing options cannot be used with Elasticsearch input"
            );
        }
    }
    let incompatible_object_mode = options.object_mode != ObjectMode::Auto
        && (matches!(resolved, InputFormat::Delimited | InputFormat::Ndjson)
            || is_sqlite_format(resolved));
    let mut effective_options = options.clone();
    let warning =
        if incompatible_object_mode && options.object_mode_origin == ObjectModeOrigin::SavedView {
            effective_options.object_mode = ObjectMode::Auto;
            Some(format!(
                "saved object_mode '{}' is incompatible with {resolved} input and was ignored",
                options.object_mode
            ))
        } else if incompatible_object_mode {
            anyhow::bail!(
                "object mode '{}' is incompatible with {resolved} input",
                options.object_mode
            );
        } else {
            None
        };
    let mut opened = match resolved {
        InputFormat::Delimited => DelimitedAdapter.open(source, &effective_options),
        InputFormat::Json => JsonAdapter::json().open(source, &effective_options),
        InputFormat::Ndjson => JsonAdapter::ndjson().open(source, &effective_options),
        #[cfg(feature = "sqlite")]
        InputFormat::Sqlite => SqliteAdapter.open(source, &effective_options),
        #[cfg(feature = "elasticsearch")]
        InputFormat::Elasticsearch => ElasticsearchAdapter.open(source, &effective_options),
        InputFormat::Auto => unreachable!("auto format must be resolved"),
    }?;
    if !sqlite && !elasticsearch {
        opened.tables = opened
            .tables
            .into_iter()
            .map(|table| apply_file_source_query(table, &effective_options))
            .collect::<anyhow::Result<Vec<_>>>()?;
    }
    if let Some(warning) = warning {
        for table in &mut opened.tables {
            table.warnings.push(warning.clone());
        }
    }
    Ok(opened)
}

fn apply_file_source_query(
    mut table: OpenedTable,
    options: &OpenOptions,
) -> anyhow::Result<OpenedTable> {
    let definition = table.definition.clone();
    let mut query = crate::table::SourceQuery::new(
        definition.generation,
        options.limit.unwrap_or(std::num::NonZeroUsize::MAX),
    );
    query.filters = options
        .source_filters
        .iter()
        .map(|request| {
            let scope = if request.column == "*" {
                crate::table::SourceFilterScope::WholeRecord
            } else {
                crate::table::SourceFilterScope::Column(if options.preview {
                    resolve_source_column(&definition, &request.column).unwrap_or(
                        crate::table::ColumnId {
                            generation: definition.generation,
                            ordinal: u32::MAX,
                        },
                    )
                } else {
                    resolve_source_column(&definition, &request.column)?
                })
            };
            Ok(crate::table::SourceFilter {
                scope,
                operator: request.operator,
                operand: request.operand.clone(),
            })
        })
        .collect::<anyhow::Result<_>>()?;
    query.order_by = options
        .source_sort
        .iter()
        .map(|request| {
            Ok(crate::table::SourceSort {
                column: resolve_source_column(&definition, &request.column)?,
                direction: request.direction,
            })
        })
        .collect::<anyhow::Result<_>>()?;
    let placeholder = Box::new(crate::table::InMemoryTable::from_text_rows(
        definition.generation,
        Vec::new(),
    ));
    let base = std::mem::replace(&mut table.store, placeholder);
    table.store = if options.preview {
        Box::new(crate::table::PreviewSourceStore::new(
            base,
            table.definition.clone(),
            query,
            options.source_filters.clone(),
        )?)
    } else if options.limit.is_none()
        && options.source_filters.is_empty()
        && options.source_sort.is_empty()
    {
        Box::new(crate::table::FileSourceQueryStore::passthrough(
            base,
            table.definition.clone(),
            query,
        ))
    } else {
        Box::new(crate::table::FileSourceQueryStore::execute_initial(
            base,
            &mut table.definition,
            query,
        )?)
    };
    Ok(table)
}

fn resolve_source_column(
    definition: &TableDefinition,
    key: &str,
) -> anyhow::Result<crate::table::ColumnId> {
    if let Some((index, _)) = definition
        .columns
        .iter()
        .enumerate()
        .find(|(index, _)| definition.canonical_column_key(*index).as_deref() == Some(key))
    {
        return Ok(definition.columns[index].id);
    }
    let matches = definition
        .columns
        .iter()
        .filter(|column| column.display_name == key)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [column] => Ok(column.id),
        [] => anyhow::bail!("source operation references unknown column '{key}'"),
        _ => anyhow::bail!("source operation column '{key}' is ambiguous"),
    }
}

fn resolve_structured_options(detected: InputFormat, options: &OpenOptions) -> InputFormat {
    if options.format == InputFormat::Auto
        && options.json_path.is_some()
        && detected == InputFormat::Delimited
    {
        InputFormat::Json
    } else {
        detected
    }
}

fn resolve_format(options: &OpenOptions, source: &InputSource, sample: &[u8]) -> InputFormat {
    if options.format == InputFormat::Auto && has_delimited_options(options) {
        if has_sqlite_signature(sample) {
            #[cfg(feature = "sqlite")]
            return InputFormat::Sqlite;
        }
        return InputFormat::Delimited;
    }
    FormatResolver::resolve(options.format, source, sample)
}

fn has_delimited_options(options: &OpenOptions) -> bool {
    options.delimited.encoding.is_some()
        || options.delimited.delimiter.is_some()
        || options.delimited.quoting.is_some()
        || options.delimited.quote_char != b'"'
}

fn has_sqlite_signature(sample: &[u8]) -> bool {
    #[cfg(feature = "sqlite")]
    {
        sample.starts_with(b"SQLite format 3\0")
    }
    #[cfg(not(feature = "sqlite"))]
    {
        let _ = sample;
        false
    }
}

fn is_sqlite_format(format: InputFormat) -> bool {
    #[cfg(feature = "sqlite")]
    {
        format == InputFormat::Sqlite
    }
    #[cfg(not(feature = "sqlite"))]
    {
        let _ = format;
        false
    }
}

fn is_elasticsearch_format(format: InputFormat) -> bool {
    #[cfg(feature = "elasticsearch")]
    {
        format == InputFormat::Elasticsearch
    }
    #[cfg(not(feature = "elasticsearch"))]
    {
        let _ = format;
        false
    }
}

fn format_from_extension(path: &Path) -> Option<InputFormat> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "json" => Some(InputFormat::Json),
        "ndjson" | "jsonl" => Some(InputFormat::Ndjson),
        _ => None,
    }
}

fn probe_content(sample: &[u8]) -> InputFormat {
    #[cfg(feature = "sqlite")]
    if has_sqlite_signature(sample) {
        return InputFormat::Sqlite;
    }
    let Ok(text) = std::str::from_utf8(sample) else {
        return InputFormat::Delimited;
    };
    let trimmed = text.trim_start_matches('\u{feff}').trim();
    let structured_lines = trimmed
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter(|line| {
            let line = line.trim();
            line.starts_with('{') || line.starts_with('[')
        })
        .count();
    let nonempty_lines = trimmed
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    if nonempty_lines > 1 && structured_lines == nonempty_lines {
        InputFormat::Ndjson
    } else if trimmed.starts_with('{') || trimmed.starts_with('[') {
        InputFormat::Json
    } else {
        InputFormat::Delimited
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::table::{RowCount, SortDirection, SourceFilterOperator, SourceOperand};

    use super::*;

    #[test]
    fn explicit_format_has_precedence() {
        let source = InputSource::Path(PathBuf::from("data.json"));
        assert_eq!(
            FormatResolver::resolve(InputFormat::Delimited, &source, br#"[{"a":1}]"#),
            InputFormat::Delimited
        );
    }

    #[test]
    fn resolves_extensions_content_and_stdin() {
        assert_eq!(
            FormatResolver::resolve(
                InputFormat::Auto,
                &InputSource::Path(PathBuf::from("data.jsonl")),
                b""
            ),
            InputFormat::Ndjson
        );
        assert_eq!(
            FormatResolver::resolve(InputFormat::Auto, &InputSource::Stdin, b"[1]\n[2]\n"),
            InputFormat::Ndjson
        );
        assert_eq!(
            FormatResolver::resolve(InputFormat::Auto, &InputSource::Stdin, b"{\"a\":1}\n"),
            InputFormat::Json
        );
        assert_eq!(
            FormatResolver::resolve(
                InputFormat::Auto,
                &InputSource::Stdin,
                b"{\"a\":1}\n{\"a\":2}\n"
            ),
            InputFormat::Ndjson
        );
        assert_eq!(
            FormatResolver::resolve(InputFormat::Auto, &InputSource::Stdin, b"a,b\n1,2\n"),
            InputFormat::Delimited
        );
    }

    #[test]
    fn explicit_selection_resolves_ambiguous_content() {
        let source = InputSource::Stdin;
        let sample = b"{a,b}\n";
        assert_eq!(
            FormatResolver::resolve(InputFormat::Json, &source, sample),
            InputFormat::Json
        );
        assert_eq!(
            FormatResolver::resolve(InputFormat::Delimited, &source, sample),
            InputFormat::Delimited
        );
    }

    #[test]
    fn ambiguous_http_urls_require_an_explicit_format() {
        for url in [
            "https://example.com/data.json",
            "http://example.com/data.csv",
            "HTTPS://example.com/data.json",
        ] {
            let source = InputSource::from_cli_value(url);
            let error = open_source(source, &OpenOptions::default())
                .err()
                .expect("ambiguous remote URL");
            assert!(error.to_string().contains("requires an explicit --format"));
        }
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn libsql_scheme_infers_sqlite_without_local_probing() {
        let source = InputSource::from_cli_value("libsql://example.turso.io");
        assert_eq!(
            FormatResolver::resolve(InputFormat::Auto, &source, b"not sqlite bytes"),
            InputFormat::Sqlite
        );
        let error = open_source(source, &OpenOptions::default())
            .err()
            .expect("remote libSQL is reserved but unsupported");
        assert!(error.to_string().contains("not supported yet"));
    }

    #[test]
    fn json_path_prevents_auto_format_from_falling_back_to_delimited() {
        let options = OpenOptions {
            json_path: Some("/rows".parse().unwrap()),
            ..OpenOptions::default()
        };
        let detected = FormatResolver::resolve(
            options.format,
            &InputSource::Stdin,
            b"not a structured sample",
        );
        assert_eq!(detected, InputFormat::Delimited);

        let resolved = resolve_structured_options(detected, &options);
        assert_eq!(resolved, InputFormat::Json);
    }

    #[test]
    fn delimited_options_override_auto_extension_and_content_detection() {
        let options = OpenOptions {
            delimited: super::super::ParseOptions {
                delimiter: Some(b'|'),
                ..super::super::ParseOptions::default()
            },
            ..OpenOptions::default()
        };

        assert_eq!(
            resolve_format(
                &options,
                &InputSource::Path(PathBuf::from("data.json")),
                br#"[{"a":1}]"#
            ),
            InputFormat::Delimited
        );
        assert_eq!(
            resolve_format(
                &options,
                &InputSource::StreamingStdin(
                    crate::ingest::source::StreamingInput::pending_for_test()
                ),
                b"{\"a\":1}\n{\"a\":2}\n"
            ),
            InputFormat::Delimited
        );
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn sqlite_signature_precedes_delimited_options_under_auto() {
        let options = OpenOptions {
            delimited: super::super::ParseOptions {
                delimiter: Some(b'|'),
                ..super::super::ParseOptions::default()
            },
            ..OpenOptions::default()
        };

        assert_eq!(
            resolve_format(
                &options,
                &InputSource::Path(PathBuf::from("database.data")),
                b"SQLite format 3\0"
            ),
            InputFormat::Sqlite
        );

        let file = tempfile::NamedTempFile::new().expect("SQLite signature fixture");
        std::fs::write(file.path(), b"SQLite format 3\0").expect("write signature");
        let error = open_source(InputSource::Path(file.path().to_path_buf()), &options)
            .err()
            .expect("delimited options must be rejected");
        assert!(error
            .to_string()
            .contains("options cannot be used with SQLite input"));
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn saved_object_mode_is_ignored_for_sqlite_but_cli_mode_is_rejected() {
        let file = tempfile::NamedTempFile::new().expect("SQLite fixture");
        let connection = rusqlite::Connection::open(file.path()).expect("SQLite connection");
        connection
            .execute_batch(
                "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT);
                 INSERT INTO users VALUES (1, 'Ada');",
            )
            .expect("SQLite fixture data");
        drop(connection);
        let saved_options = OpenOptions {
            object_mode: ObjectMode::Entries,
            object_mode_origin: ObjectModeOrigin::SavedView,
            table: Some("users".to_owned()),
            ..OpenOptions::default()
        };

        let table = open_source(InputSource::Path(file.path().to_path_buf()), &saved_options)
            .expect("saved mode ignored")
            .into_implicit_table()
            .expect("SQLite table");
        assert!(table
            .warnings
            .iter()
            .any(|warning| warning.contains("object_mode") && warning.contains("ignored")));

        let error = open_source(
            InputSource::Path(file.path().to_path_buf()),
            &OpenOptions {
                object_mode_origin: ObjectModeOrigin::Cli,
                ..saved_options
            },
        )
        .err()
        .expect("CLI mode rejected");
        assert!(error.to_string().contains("object mode"));
        assert!(error.to_string().contains("incompatible with sqlite"));
    }

    #[test]
    fn automatic_streaming_probe_selects_ndjson_without_waiting_for_eof() {
        let input = crate::ingest::source::StreamingInput::pending_for_test();
        input.append_for_test(b"{\"a\":1}\n{\"a\":2}\n");
        let source = open_source(
            InputSource::StreamingStdin(input.clone()),
            &OpenOptions::default(),
        )
        .expect("open streaming NDJSON");
        let table = source.into_implicit_table().expect("table");
        assert_eq!(table.store.row_count(), RowCount::AtLeast(2));
        input.finish_for_test();
    }

    #[test]
    fn file_source_filter_treats_quoted_multiline_csv_as_one_record() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "id,note\n1,\"multi\nline\"\n2,single\n").unwrap();
        let source = open_source(
            InputSource::Path(file.path().to_path_buf()),
            &OpenOptions {
                source_filters: vec![super::super::SourceFilterRequest {
                    column: "*".to_owned(),
                    operator: SourceFilterOperator::Contains,
                    operand: Some(SourceOperand::Text("multi\nline".to_owned())),
                }],
                limit: std::num::NonZeroUsize::new(10),
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let mut table = source.into_implicit_table().unwrap();
        let materialized = table.store.materialize().unwrap();
        assert_eq!(materialized.rows().len(), 1);
        assert_eq!(materialized.rows()[0].cells[0].display(), "1");
        assert!(matches!(
            table.store.result_extent(),
            Some(crate::table::ResultExtent::Complete { source_rows: 1 })
        ));
    }

    #[test]
    fn file_source_sort_fails_without_unbounded_fallback() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "id\n2\n1\n").unwrap();
        let result = open_source(
            InputSource::Path(file.path().to_path_buf()),
            &OpenOptions {
                source_sort: vec![super::super::SourceSortRequest {
                    column: "id".to_owned(),
                    direction: SortDirection::Ascending,
                }],
                ..OpenOptions::default()
            },
        );
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("source sorting is unavailable"));
    }
}
