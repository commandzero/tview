use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;

use crate::table::{
    validate_source_query, CapabilityStatus, CellValue, ColumnDefinition, ColumnId,
    ColumnSourceIdentity, InMemoryTable, IndexProgress, LogicalType, NativeQueryArtifact,
    NativeQueryLanguage, NativeQueryParameter, ResultExtent, Row, RowCount, RowId, RowIndex,
    RowVisitor, ScanDirection, ScanProgress, ScanRequest, SchemaDelta, SchemaState, SortDirection,
    SourceFilter, SourceFilterOperator, SourceFilterScope, SourceGeneration, SourceOperand,
    SourceOperationCapabilities, SourceQuery, SourceQueryExecution, SourceSort, StableRowIdentity,
    TableDefinition, TableStore, TypeOrigin, TypeWidening, DEFAULT_SQLITE_SOURCE_LIMIT,
};

use super::adapter::{
    OpenedSource, OpenedTable, ProbeResult, RelationAvailability, RelationCatalogEntry,
    RelationKind, RelationOpener, SourceAdapter,
};
use super::source::InputSource;
use super::{InputFormat, OpenOptions, SourceFilterRequest, SourceSortRequest};

const SQLITE_SIGNATURE: &[u8; 16] = b"SQLite format 3\0";
const DISCOVER_SCHEMA_SQL: &str =
    "SELECT type, name, tbl_name, sql FROM sqlite_schema ORDER BY name";
const TABLE_LIST_SQL: &str = "PRAGMA table_list";
const QUERY_ONLY_ENABLE_SQL: &str = "PRAGMA query_only=1";
const QUERY_ONLY_VERIFY_SQL: &str = "PRAGMA query_only";

#[derive(Debug, Clone)]
pub struct SqliteAdapter;

impl SourceAdapter for SqliteAdapter {
    fn format(&self) -> InputFormat {
        InputFormat::Sqlite
    }

    fn probe(&self, source: &InputSource, sample: &[u8]) -> ProbeResult {
        if matches!(source, InputSource::Path(_)) && sample.starts_with(SQLITE_SIGNATURE) {
            ProbeResult::Strong
        } else {
            ProbeResult::NoMatch
        }
    }

    fn open(&self, source: InputSource, options: &OpenOptions) -> anyhow::Result<OpenedSource> {
        let path = match source {
            InputSource::Path(path) => path,
            InputSource::Stdin | InputSource::StreamingStdin(_) => {
                anyhow::bail!("SQLite input from stdin is not supported; provide a local path")
            }
            InputSource::Url(url) => {
                anyhow::bail!(
                    "remote SQLite database '{}' is not supported yet",
                    InputSource::Url(url).safe_identity()
                )
            }
        };
        let session = Arc::new(SqliteSession::open(&path)?);
        if let Some(native_query) = options.native_query.as_deref() {
            return Ok(OpenedSource::implicit(open_sqlite_native_query(
                session,
                native_query,
                options,
            )?));
        }
        let discovered = session.discover_relations()?;
        let catalog = discovered
            .iter()
            .filter_map(DiscoveredRelation::catalog_entry)
            .collect::<Vec<_>>();
        let selectable = catalog
            .iter()
            .filter(|entry| entry.is_selectable())
            .map(|entry| entry.metadata.name.clone())
            .collect::<Vec<_>>();

        let requested = options.table.as_deref();
        let selected_name = if let Some(requested) = requested {
            let relation = discovered
                .iter()
                .find(|relation| relation.name.eq_ignore_ascii_case(requested))
                .ok_or_else(|| anyhow::anyhow!("relation '{requested}' was not found"))?;
            match &relation.classification {
                RelationClassification::Selectable(_) => Some(relation.name.clone()),
                RelationClassification::Unavailable(reason) => {
                    anyhow::bail!("relation '{requested}' is unavailable: {reason}")
                }
                RelationClassification::Virtual => {
                    anyhow::bail!("relation '{requested}' is a virtual table and is unsupported")
                }
                RelationClassification::Hidden => {
                    anyhow::bail!("relation '{requested}' is not a user-selectable SQLite object")
                }
            }
        } else if selectable.len() == 1 {
            selectable.first().cloned()
        } else {
            None
        };

        if selectable.is_empty() {
            if discovered
                .iter()
                .all(|relation| matches!(relation.classification, RelationClassification::Hidden))
            {
                anyhow::bail!("SQLite database contains no user tables or views");
            }
            anyhow::bail!("SQLite database contains no selectable tables or compatible views");
        }

        let mut opener = SqliteRelationOpener {
            session,
            discovered,
            options: options.clone(),
        };
        let selected = selected_name
            .as_deref()
            .map(|name| opener.open_relation(name))
            .transpose()?;
        Ok(OpenedSource::relational(
            catalog,
            selected,
            Box::new(opener),
        ))
    }
}

#[derive(Clone)]
struct SqliteSession {
    _database: Arc<turso::core::Database>,
    connection: Arc<turso::core::Connection>,
}

impl SqliteSession {
    fn open(path: &Path) -> anyhow::Result<Self> {
        let path = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("SQLite path is not valid UTF-8"))?;
        let io: Arc<dyn turso::core::IO> = Arc::new(turso::core::PlatformIO::new()?);
        let options = turso::core::DatabaseOpts::new()
            .with_generated_columns(true)
            .with_without_rowid(true);
        let database = turso::core::Database::open_file_with_flags(
            io,
            path,
            turso::core::OpenFlags::ReadOnly,
            options,
            None,
            Arc::new(turso::core::SqliteDialect),
        )?;
        let connection = database.connect()?;
        let session = Self {
            _database: database,
            connection,
        };
        session.enable_and_verify_query_only()?;
        Ok(session)
    }

    fn enable_and_verify_query_only(&self) -> anyhow::Result<()> {
        self.connection.execute(QUERY_ONLY_ENABLE_SQL)?;
        let values = self.query_values(QUERY_ONLY_VERIFY_SQL, Vec::new())?;
        let enabled = values
            .first()
            .and_then(|row| row.first())
            .is_some_and(|value| matches!(value, SqliteValue::Integer(1)));
        if !enabled {
            anyhow::bail!("failed to verify SQLite query-only confinement");
        }
        Ok(())
    }

    fn query_values(
        &self,
        sql: &str,
        params: Vec<turso::core::Value>,
    ) -> anyhow::Result<Vec<Vec<SqliteValue>>> {
        let mut rows = self.query(sql, params)?;
        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            result.push(row);
        }
        Ok(result)
    }

    fn prepare_columns(&self, sql: &str) -> anyhow::Result<Vec<PreparedColumn>> {
        let statement = self.connection.prepare(sql)?;
        Ok((0..statement.num_columns())
            .map(|index| PreparedColumn {
                name: statement.get_column_name(index).into_owned(),
                declared_type: statement.get_column_decltype(index),
            })
            .collect())
    }

    fn prepare_native_row_query(&self, sql: &str) -> anyhow::Result<PreparedNativeQuery> {
        let statement = self.connection.prepare(sql).map_err(|error| {
            if error.to_string().contains("query_only") {
                anyhow::anyhow!("native SQLite query must be read-only: {error}")
            } else {
                anyhow::Error::from(error)
            }
        })?;
        if !sql_tail_is_empty(&sql[statement.tail_offset()..]) {
            anyhow::bail!("native SQLite query must contain exactly one statement");
        }
        if !statement.get_program().is_readonly() {
            anyhow::bail!("native SQLite query must be read-only");
        }
        if statement.num_columns() == 0 {
            anyhow::bail!("native SQLite query must produce a tabular result");
        }
        if statement.parameters_count() != 0 {
            anyhow::bail!("native SQLite query cannot contain unbound parameters");
        }
        let embeddable_sql = remove_trailing_statement_terminator(sql);
        let columns = (0..statement.num_columns())
            .map(|index| PreparedColumn {
                name: statement.get_column_name(index).into_owned(),
                declared_type: statement.get_column_decltype(index),
            })
            .collect();
        Ok(PreparedNativeQuery {
            columns,
            embeddable_sql,
        })
    }

    fn discover_relations(&self) -> anyhow::Result<Vec<DiscoveredRelation>> {
        let table_kinds = self
            .query_values(TABLE_LIST_SQL, Vec::new())?
            .into_iter()
            .filter_map(|row| {
                Some((
                    value_text(row.get(1)?)?.to_owned(),
                    value_text(row.get(2)?)?.to_owned(),
                ))
            })
            .collect::<BTreeMap<_, _>>();
        let schema_rows = self.query_values(DISCOVER_SCHEMA_SQL, Vec::new())?;
        let virtual_roots = schema_rows
            .iter()
            .filter_map(|row| {
                let name = row.get(1).and_then(value_text)?;
                let sql = row.get(3).and_then(value_text)?;
                sql.trim_start()
                    .to_ascii_uppercase()
                    .starts_with("CREATE VIRTUAL TABLE")
                    .then(|| name.to_owned())
            })
            .collect::<Vec<_>>();
        let mut relations = Vec::new();
        for row in schema_rows {
            let Some(object_type) = row.first().and_then(value_text) else {
                continue;
            };
            let Some(name) = row.get(1).and_then(value_text) else {
                continue;
            };
            let sql = row.get(3).and_then(value_text).unwrap_or_default();
            let table_kind = table_kinds.get(name).map(String::as_str);
            let is_virtual_shadow = virtual_roots.iter().any(|root| {
                name.strip_prefix(root)
                    .is_some_and(|rest| rest.starts_with('_'))
            });
            let classification = if name.starts_with("sqlite_")
                || matches!(table_kind, Some("shadow"))
                || is_virtual_shadow
            {
                RelationClassification::Hidden
            } else if object_type == "view" {
                let probe = format!("SELECT * FROM {} LIMIT 0", quote_identifier(name));
                match self.prepare_columns(&probe) {
                    Ok(columns) => {
                        RelationClassification::Selectable(DiscoveredKind::View { columns })
                    }
                    Err(error) => RelationClassification::Unavailable(error.to_string()),
                }
            } else if object_type == "table"
                && (matches!(table_kind, Some("virtual"))
                    || sql
                        .trim_start()
                        .to_ascii_uppercase()
                        .starts_with("CREATE VIRTUAL TABLE"))
            {
                RelationClassification::Virtual
            } else if object_type == "table" {
                RelationClassification::Selectable(DiscoveredKind::Table)
            } else {
                RelationClassification::Hidden
            };
            relations.push(DiscoveredRelation {
                name: name.to_owned(),
                classification,
            });
        }
        Ok(relations)
    }

    fn table_schema(&self, relation: &str) -> anyhow::Result<TableSchema> {
        let pragma = format!("PRAGMA table_xinfo({})", quote_identifier(relation));
        let rows = self.query_values(&pragma, Vec::new())?;
        let mut columns = Vec::new();
        for row in rows {
            let ordinal = value_i64(row.first()).unwrap_or(columns.len() as i64) as usize;
            let name = row
                .get(1)
                .and_then(value_text)
                .unwrap_or_default()
                .to_owned();
            let declared_type = row
                .get(2)
                .and_then(value_text)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
            let primary_key_order = value_i64(row.get(5)).unwrap_or_default() as usize;
            columns.push(SqliteColumn {
                ordinal,
                name,
                declared_type,
                primary_key_order,
            });
        }
        let table_list = self.query_values(TABLE_LIST_SQL, Vec::new())?;
        let without_rowid = table_list.into_iter().any(|row| {
            row.get(1).and_then(value_text) == Some(relation)
                && value_i64(row.get(4)).unwrap_or_default() != 0
        });
        Ok(TableSchema {
            columns,
            without_rowid,
        })
    }

    fn query_rows(&self, sql: &str, params: &[SourceOperand]) -> anyhow::Result<SqliteRows> {
        let values = params.iter().map(turso_value).collect::<Vec<_>>();
        self.query(sql, values)
    }

    fn query(&self, sql: &str, params: Vec<turso::core::Value>) -> anyhow::Result<SqliteRows> {
        let mut statement = self.connection.prepare(sql)?;
        for (index, value) in params.into_iter().enumerate() {
            statement.bind_at(
                NonZeroUsize::new(index + 1).expect("parameter indices are one-based"),
                value,
            )?;
        }
        Ok(SqliteRows { statement })
    }

    #[cfg(test)]
    fn mutation_is_rejected(&self, sql: &str) -> bool {
        self.connection.execute(sql).is_err()
    }
}

fn sql_tail_is_empty(mut tail: &str) -> bool {
    loop {
        tail = tail.trim_start();
        if tail.is_empty() {
            return true;
        }
        if let Some(rest) = tail.strip_prefix("--") {
            tail = rest.split_once('\n').map(|(_, rest)| rest).unwrap_or("");
            continue;
        }
        if let Some(rest) = tail.strip_prefix("/*") {
            let Some((_, rest)) = rest.split_once("*/") else {
                return false;
            };
            tail = rest;
            continue;
        }
        return false;
    }
}

fn remove_trailing_statement_terminator(sql: &str) -> String {
    #[derive(Clone, Copy)]
    enum State {
        Sql,
        SingleQuote,
        DoubleQuote,
        Backtick,
        Bracket,
        LineComment,
        BlockComment,
    }

    let bytes = sql.as_bytes();
    let mut state = State::Sql;
    let mut index = 0;
    let mut last_significant = None;
    while index < bytes.len() {
        match state {
            State::Sql => match bytes[index] {
                byte if byte.is_ascii_whitespace() => index += 1,
                b'\'' => {
                    last_significant = Some(index);
                    state = State::SingleQuote;
                    index += 1;
                }
                b'"' => {
                    last_significant = Some(index);
                    state = State::DoubleQuote;
                    index += 1;
                }
                b'`' => {
                    last_significant = Some(index);
                    state = State::Backtick;
                    index += 1;
                }
                b'[' => {
                    last_significant = Some(index);
                    state = State::Bracket;
                    index += 1;
                }
                b'-' if bytes.get(index + 1) == Some(&b'-') => {
                    state = State::LineComment;
                    index += 2;
                }
                b'/' if bytes.get(index + 1) == Some(&b'*') => {
                    state = State::BlockComment;
                    index += 2;
                }
                _ => {
                    last_significant = Some(index);
                    index += 1;
                }
            },
            State::SingleQuote => {
                if bytes[index] == b'\'' {
                    if bytes.get(index + 1) == Some(&b'\'') {
                        index += 2;
                    } else {
                        last_significant = Some(index);
                        state = State::Sql;
                        index += 1;
                    }
                } else {
                    index += 1;
                }
            }
            State::DoubleQuote => {
                if bytes[index] == b'"' {
                    if bytes.get(index + 1) == Some(&b'"') {
                        index += 2;
                    } else {
                        last_significant = Some(index);
                        state = State::Sql;
                        index += 1;
                    }
                } else {
                    index += 1;
                }
            }
            State::Backtick => {
                if bytes[index] == b'`' {
                    if bytes.get(index + 1) == Some(&b'`') {
                        index += 2;
                    } else {
                        last_significant = Some(index);
                        state = State::Sql;
                        index += 1;
                    }
                } else {
                    index += 1;
                }
            }
            State::Bracket => {
                if bytes[index] == b']' {
                    last_significant = Some(index);
                    state = State::Sql;
                }
                index += 1;
            }
            State::LineComment => {
                if bytes[index] == b'\n' {
                    state = State::Sql;
                }
                index += 1;
            }
            State::BlockComment => {
                if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    state = State::Sql;
                    index += 2;
                } else {
                    index += 1;
                }
            }
        }
    }

    let Some(terminator) = last_significant.filter(|index| bytes[*index] == b';') else {
        return sql.trim().to_owned();
    };
    format!("{}{}", &sql[..terminator], &sql[terminator + 1..])
        .trim()
        .to_owned()
}

#[derive(Debug, Clone, PartialEq)]
enum SqliteValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

struct SqliteRows {
    statement: turso::core::Statement,
}

impl SqliteRows {
    fn next(&mut self) -> anyhow::Result<Option<Vec<SqliteValue>>> {
        loop {
            match self.statement.step()? {
                turso::core::StepResult::Row => {
                    let row = self
                        .statement
                        .row()
                        .ok_or_else(|| anyhow::anyhow!("SQLite returned an empty row state"))?;
                    return Ok(Some(
                        row.get_values()
                            .cloned()
                            .map(sqlite_value)
                            .collect::<Vec<_>>(),
                    ));
                }
                turso::core::StepResult::Done => return Ok(None),
                turso::core::StepResult::IO | turso::core::StepResult::Yield => {
                    self.statement._io().step()?;
                }
                turso::core::StepResult::Sleep { duration } => std::thread::sleep(duration),
                turso::core::StepResult::Busy => anyhow::bail!("SQLite database is busy"),
                turso::core::StepResult::Interrupt => {
                    anyhow::bail!("SQLite query was interrupted")
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct PreparedColumn {
    name: String,
    declared_type: Option<String>,
}

struct PreparedNativeQuery {
    columns: Vec<PreparedColumn>,
    embeddable_sql: String,
}

#[derive(Debug, Clone)]
struct DiscoveredRelation {
    name: String,
    classification: RelationClassification,
}

impl DiscoveredRelation {
    fn catalog_entry(&self) -> Option<RelationCatalogEntry> {
        let (kind, availability) = match &self.classification {
            RelationClassification::Selectable(DiscoveredKind::Table) => {
                (RelationKind::Table, RelationAvailability::Selectable)
            }
            RelationClassification::Selectable(DiscoveredKind::View { .. }) => {
                (RelationKind::View, RelationAvailability::Selectable)
            }
            RelationClassification::Unavailable(reason) => (
                RelationKind::View,
                RelationAvailability::Unavailable {
                    reason: reason.clone(),
                },
            ),
            RelationClassification::Virtual => (
                RelationKind::VirtualTable,
                RelationAvailability::Unavailable {
                    reason: "virtual tables are unsupported in this release".to_owned(),
                },
            ),
            RelationClassification::Hidden => return None,
        };
        Some(RelationCatalogEntry {
            metadata: crate::table::RelationMetadata {
                name: self.name.clone(),
                display_name: self.name.clone(),
                header_visible: true,
            },
            kind,
            availability,
        })
    }
}

#[derive(Debug, Clone)]
enum RelationClassification {
    Selectable(DiscoveredKind),
    Unavailable(String),
    Virtual,
    Hidden,
}

#[derive(Debug, Clone)]
enum DiscoveredKind {
    Table,
    View { columns: Vec<PreparedColumn> },
}

struct SqliteRelationOpener {
    session: Arc<SqliteSession>,
    discovered: Vec<DiscoveredRelation>,
    options: OpenOptions,
}

impl RelationOpener for SqliteRelationOpener {
    fn open_relation(&mut self, name: &str) -> anyhow::Result<OpenedTable> {
        let relation = self
            .discovered
            .iter()
            .find(|relation| relation.name == name)
            .ok_or_else(|| anyhow::anyhow!("relation '{name}' was not found"))?
            .clone();
        let kind = match relation.classification {
            RelationClassification::Selectable(kind) => kind,
            RelationClassification::Unavailable(reason) => {
                anyhow::bail!("relation '{name}' is unavailable: {reason}")
            }
            RelationClassification::Virtual => {
                anyhow::bail!("relation '{name}' is a virtual table and is unsupported")
            }
            RelationClassification::Hidden => {
                anyhow::bail!("relation '{name}' is not selectable")
            }
        };
        open_sqlite_relation(self.session.clone(), relation.name, kind, &self.options)
    }
}

#[derive(Debug, Clone)]
struct SqliteColumn {
    ordinal: usize,
    name: String,
    declared_type: Option<String>,
    primary_key_order: usize,
}

#[derive(Debug, Clone)]
struct TableSchema {
    columns: Vec<SqliteColumn>,
    without_rowid: bool,
}

#[derive(Debug, Clone)]
enum SqliteIdentityPlan {
    RowId { expression: String },
    PrimaryKey { visible_indices: Vec<usize> },
    Unavailable,
}

fn open_sqlite_relation(
    session: Arc<SqliteSession>,
    relation: String,
    kind: DiscoveredKind,
    options: &OpenOptions,
) -> anyhow::Result<OpenedTable> {
    let generation = SourceGeneration::new();
    let (source_columns, identity, is_view) = match kind {
        DiscoveredKind::Table => {
            let schema = session.table_schema(&relation)?;
            let identity = identity_plan(&schema);
            (schema.columns, identity, false)
        }
        DiscoveredKind::View { columns } => (
            columns
                .into_iter()
                .enumerate()
                .map(|(ordinal, column)| SqliteColumn {
                    ordinal,
                    name: column.name,
                    declared_type: column.declared_type,
                    primary_key_order: 0,
                })
                .collect(),
            SqliteIdentityPlan::Unavailable,
            true,
        ),
    };
    let columns = source_columns
        .iter()
        .map(|column| ColumnDefinition {
            id: ColumnId {
                generation,
                ordinal: column.ordinal as u32,
            },
            source_identity: ColumnSourceIdentity::RelationColumn {
                relation: relation.clone(),
                ordinal: column.ordinal,
                name: column.name.clone(),
            },
            display_name: column.name.clone(),
            source_declared_type: column.declared_type.clone(),
            source_type: declared_type_hint(column.declared_type.as_deref()),
            type_origin: if is_view && column.declared_type.is_none() {
                TypeOrigin::Inferred
            } else {
                TypeOrigin::Declared
            },
        })
        .collect::<Vec<_>>();
    let definition = TableDefinition {
        generation,
        columns,
        schema_state: SchemaState::Complete,
        relation: crate::table::RelationMetadata {
            name: relation.clone(),
            display_name: relation.clone(),
            header_visible: true,
        },
    };
    let mut query = SourceQuery::new(
        generation,
        options.limit.unwrap_or_else(|| {
            NonZeroUsize::new(DEFAULT_SQLITE_SOURCE_LIMIT).expect("non-zero default")
        }),
    );
    query.filters = resolve_source_filters(&definition, &options.source_filters)?;
    query.order_by = resolve_source_sort(&definition, &options.source_sort)?;
    validate_source_query(&definition, &query)?;
    let store = TursoTableStore::start(session, definition.clone(), identity, query)?;
    Ok(OpenedTable {
        generation,
        definition,
        store: Box::new(store),
        object_mode: None,
        warnings: Vec::new(),
    })
}

fn open_sqlite_native_query(
    session: Arc<SqliteSession>,
    native_query: &str,
    options: &OpenOptions,
) -> anyhow::Result<OpenedTable> {
    let definition = native_query_definition(&session, native_query)?;
    let generation = definition.generation;
    let mut query = SourceQuery::new(
        definition.generation,
        options.limit.unwrap_or_else(|| {
            NonZeroUsize::new(DEFAULT_SQLITE_SOURCE_LIMIT).expect("non-zero default")
        }),
    );
    query.native_query = Some(native_query.to_owned());
    query.filters = resolve_source_filters(&definition, &options.source_filters)?;
    query.order_by = resolve_source_sort(&definition, &options.source_sort)?;
    validate_source_query(&definition, &query)?;
    let store = TursoTableStore::start(
        session,
        definition.clone(),
        SqliteIdentityPlan::Unavailable,
        query,
    )?;
    Ok(OpenedTable {
        generation,
        definition,
        store: Box::new(store),
        object_mode: None,
        warnings: Vec::new(),
    })
}

fn native_query_definition(
    session: &SqliteSession,
    native_query: &str,
) -> anyhow::Result<TableDefinition> {
    let prepared = session.prepare_native_row_query(native_query)?;
    let generation = SourceGeneration::new();
    let relation = "__tview_native_query".to_owned();
    let columns = prepared
        .columns
        .into_iter()
        .enumerate()
        .map(|(ordinal, column)| ColumnDefinition {
            id: ColumnId {
                generation,
                ordinal: ordinal as u32,
            },
            source_identity: ColumnSourceIdentity::RelationColumn {
                relation: relation.clone(),
                ordinal,
                name: column.name.clone(),
            },
            display_name: column.name,
            source_declared_type: column.declared_type.clone(),
            source_type: declared_type_hint(column.declared_type.as_deref()),
            type_origin: if column.declared_type.is_some() {
                TypeOrigin::Declared
            } else {
                TypeOrigin::Inferred
            },
        })
        .collect();
    Ok(TableDefinition {
        generation,
        columns,
        schema_state: SchemaState::Complete,
        relation: crate::table::RelationMetadata {
            name: relation,
            display_name: "SQLite query".to_owned(),
            header_visible: true,
        },
    })
}

fn remap_native_query(
    query: &SourceQuery,
    old: &TableDefinition,
    new: &TableDefinition,
) -> anyhow::Result<SourceQuery> {
    let mut remapped = SourceQuery::new(new.generation, query.limit);
    remapped.native_query = query.native_query.clone();
    remapped.filters = query
        .filters
        .iter()
        .map(|filter| {
            let SourceFilterScope::Column(old_id) = filter.scope else {
                anyhow::bail!("SQLite source filters require a result column");
            };
            let name = old
                .columns
                .get(old_id.ordinal as usize)
                .ok_or_else(|| anyhow::anyhow!("native query filter column is stale"))?
                .display_name
                .clone();
            Ok(SourceFilter {
                scope: SourceFilterScope::Column(resolve_column(new, &name)?),
                operator: filter.operator,
                operand: filter.operand.clone(),
            })
        })
        .collect::<anyhow::Result<_>>()?;
    remapped.order_by = query
        .order_by
        .iter()
        .map(|sort| {
            let name = old
                .columns
                .get(sort.column.ordinal as usize)
                .ok_or_else(|| anyhow::anyhow!("native query sort column is stale"))?
                .display_name
                .clone();
            Ok(SourceSort {
                column: resolve_column(new, &name)?,
                direction: sort.direction,
            })
        })
        .collect::<anyhow::Result<_>>()?;
    Ok(remapped)
}

fn resolve_source_filters(
    definition: &TableDefinition,
    requests: &[SourceFilterRequest],
) -> anyhow::Result<Vec<SourceFilter>> {
    requests
        .iter()
        .map(|request| {
            let column = resolve_column(definition, &request.column)?;
            Ok(SourceFilter {
                scope: SourceFilterScope::Column(column),
                operator: request.operator,
                operand: request.operand.clone(),
            })
        })
        .collect()
}

fn resolve_source_sort(
    definition: &TableDefinition,
    requests: &[SourceSortRequest],
) -> anyhow::Result<Vec<SourceSort>> {
    requests
        .iter()
        .map(|request| {
            Ok(SourceSort {
                column: resolve_column(definition, &request.column)?,
                direction: request.direction,
            })
        })
        .collect()
}

fn resolve_column(definition: &TableDefinition, name: &str) -> anyhow::Result<ColumnId> {
    if let Some((index, _)) = definition
        .columns
        .iter()
        .enumerate()
        .find(|(index, _)| definition.canonical_column_key(*index).as_deref() == Some(name))
    {
        return Ok(definition.columns[index].id);
    }
    let matches = definition
        .columns
        .iter()
        .filter(|column| match &column.source_identity {
            ColumnSourceIdentity::RelationColumn {
                name: source_name, ..
            } => source_name == name,
            _ => false,
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [column] => Ok(column.id),
        [] => anyhow::bail!("source operation references unknown column '{name}'"),
        _ => {
            anyhow::bail!("source column '{name}' is ambiguous; use a deterministic occurrence key")
        }
    }
}

fn identity_plan(schema: &TableSchema) -> SqliteIdentityPlan {
    if schema.without_rowid {
        let mut primary = schema
            .columns
            .iter()
            .filter(|column| column.primary_key_order > 0)
            .map(|column| (column.primary_key_order, column.ordinal))
            .collect::<Vec<_>>();
        primary.sort_by_key(|(order, _)| *order);
        return if primary.is_empty() {
            SqliteIdentityPlan::Unavailable
        } else {
            SqliteIdentityPlan::PrimaryKey {
                visible_indices: primary.into_iter().map(|(_, index)| index).collect(),
            }
        };
    }
    let names = schema
        .columns
        .iter()
        .map(|column| column.name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    ["rowid", "_rowid_", "oid"]
        .into_iter()
        .find(|candidate| !names.contains(*candidate))
        .map(|expression| SqliteIdentityPlan::RowId {
            expression: expression.to_owned(),
        })
        .unwrap_or(SqliteIdentityPlan::Unavailable)
}

fn declared_type_hint(declaration: Option<&str>) -> LogicalType {
    let Some(declaration) = declaration else {
        return LogicalType::Unknown;
    };
    let normalized = declaration.trim().to_ascii_uppercase();
    if normalized.is_empty() || normalized == "ANY" {
        LogicalType::Unknown
    } else if normalized.contains("INT") {
        LogicalType::Integer
    } else if normalized.contains("CHAR")
        || normalized.contains("CLOB")
        || normalized.contains("TEXT")
    {
        LogicalType::Text
    } else if normalized == "BLOB" {
        LogicalType::Binary
    } else if normalized.contains("REAL")
        || normalized.contains("FLOA")
        || normalized.contains("DOUB")
    {
        LogicalType::Float
    } else {
        LogicalType::Unknown
    }
}

#[derive(Debug, Clone)]
struct CompiledSqliteQuery {
    execution_sql: String,
    execution_parameters: Vec<SourceOperand>,
    provenance: NativeQueryArtifact,
}

fn compile_sqlite_query(
    definition: &TableDefinition,
    query: &SourceQuery,
    identity: &SqliteIdentityPlan,
    embeddable_native_query: Option<&str>,
) -> anyhow::Result<CompiledSqliteQuery> {
    validate_source_query(definition, query)?;
    let relation = quote_identifier(&definition.relation.name);
    let hidden_identity = match identity {
        SqliteIdentityPlan::RowId { expression } => {
            format!(", {} AS \"__tview_rowid\"", quote_identifier(expression))
        }
        SqliteIdentityPlan::PrimaryKey { .. } | SqliteIdentityPlan::Unavailable => String::new(),
    };
    let mut logical = if let Some(base) = embeddable_native_query {
        format!("SELECT * FROM (\n{base}\n) AS \"__tview_source\"")
    } else {
        format!("SELECT *{hidden_identity} FROM {relation}")
    };
    let mut parameters = Vec::new();
    if !query.filters.is_empty() {
        logical.push_str(" WHERE ");
        for (index, filter) in query.filters.iter().enumerate() {
            if index > 0 {
                logical.push_str(" AND ");
            }
            let SourceFilterScope::Column(column_id) = filter.scope else {
                anyhow::bail!("SQLite source filters require a resolved column");
            };
            let column = definition
                .columns
                .get(column_id.ordinal as usize)
                .ok_or_else(|| anyhow::anyhow!("source filter column is unavailable"))?;
            let identifier = quote_identifier(source_column_name(column)?);
            match filter.operator {
                SourceFilterOperator::IsNull => {
                    logical.push_str(&format!("{identifier} IS NULL"));
                }
                SourceFilterOperator::IsNotNull => {
                    logical.push_str(&format!("{identifier} IS NOT NULL"));
                }
                operator => {
                    let operand = filter
                        .operand
                        .clone()
                        .ok_or_else(|| anyhow::anyhow!("source filter operand is missing"))?;
                    parameters.push(operand);
                    let parameter = format!("?{}", parameters.len());
                    match operator {
                        SourceFilterOperator::Equal => {
                            logical.push_str(&format!("{identifier} = {parameter}"));
                        }
                        SourceFilterOperator::NotEqual => {
                            logical.push_str(&format!("{identifier} <> {parameter}"));
                        }
                        SourceFilterOperator::LessThan => {
                            logical.push_str(&format!("{identifier} < {parameter}"));
                        }
                        SourceFilterOperator::LessThanOrEqual => {
                            logical.push_str(&format!("{identifier} <= {parameter}"));
                        }
                        SourceFilterOperator::GreaterThan => {
                            logical.push_str(&format!("{identifier} > {parameter}"));
                        }
                        SourceFilterOperator::GreaterThanOrEqual => {
                            logical.push_str(&format!("{identifier} >= {parameter}"));
                        }
                        SourceFilterOperator::Contains => logical.push_str(&format!(
                            "instr(CAST({identifier} AS TEXT), {parameter}) > 0"
                        )),
                        SourceFilterOperator::Prefix => logical.push_str(&format!(
                            "instr(CAST({identifier} AS TEXT), {parameter}) = 1"
                        )),
                        SourceFilterOperator::IsNull | SourceFilterOperator::IsNotNull => {
                            unreachable!("handled above")
                        }
                    }
                }
            }
        }
    }
    let mut order = query
        .order_by
        .iter()
        .map(|sort| {
            let column = definition
                .columns
                .get(sort.column.ordinal as usize)
                .ok_or_else(|| anyhow::anyhow!("source sort column is unavailable"))?;
            Ok(format!(
                "{} {}",
                quote_identifier(source_column_name(column)?),
                match sort.direction {
                    SortDirection::Ascending => "ASC",
                    SortDirection::Descending => "DESC",
                }
            ))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    if !order.is_empty() {
        if let SqliteIdentityPlan::RowId { expression } = identity {
            order.push(format!("{} ASC", quote_identifier(expression)));
        }
        logical.push_str(" ORDER BY ");
        logical.push_str(&order.join(", "));
    }
    logical.push_str(&format!(" LIMIT {}", query.limit));
    let copyable_sql = render_copyable_sql(&logical, &parameters);
    let logical_without_limit = logical
        .strip_suffix(&format!(" LIMIT {}", query.limit))
        .expect("compiler adds limit");
    let probe_limit = query.limit.get().saturating_add(1);
    let execution_sql = format!("{logical_without_limit} LIMIT {probe_limit}");
    Ok(CompiledSqliteQuery {
        execution_sql,
        execution_parameters: parameters.clone(),
        provenance: NativeQueryArtifact {
            language: NativeQueryLanguage::Sql,
            base: query.native_query.clone(),
            logical,
            parameters: parameters
                .into_iter()
                .map(NativeQueryParameter::Value)
                .collect(),
            copyable: copyable_sql,
        },
    })
}

fn source_column_name(column: &ColumnDefinition) -> anyhow::Result<&str> {
    match &column.source_identity {
        ColumnSourceIdentity::RelationColumn { name, .. } => Ok(name),
        _ => anyhow::bail!("SQLite query references a non-relational column"),
    }
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn render_copyable_sql(sql: &str, parameters: &[SourceOperand]) -> String {
    let mut rendered = sql.to_owned();
    for (index, value) in parameters.iter().enumerate().rev() {
        rendered = rendered.replace(&format!("?{}", index + 1), &render_sql_literal(value));
    }
    rendered
}

fn render_sql_literal(value: &SourceOperand) -> String {
    match value {
        SourceOperand::Null => "NULL".to_owned(),
        SourceOperand::Boolean(value) => i32::from(*value).to_string(),
        SourceOperand::Integer(value) => value.to_string(),
        SourceOperand::Float(value) => value.to_string(),
        SourceOperand::Text(value) => format!("'{}'", value.replace('\'', "''")),
        SourceOperand::Binary(value) => {
            let hex = value
                .iter()
                .map(|byte| format!("{byte:02X}"))
                .collect::<String>();
            format!("X'{hex}'")
        }
    }
}

fn turso_value(value: &SourceOperand) -> turso::core::Value {
    match value {
        SourceOperand::Null => turso::core::Value::Null,
        SourceOperand::Boolean(value) => turso::core::Value::from_i64(i64::from(*value)),
        SourceOperand::Integer(value) => turso::core::Value::from_i64(*value),
        SourceOperand::Float(value) => turso::core::Value::from_f64(*value),
        SourceOperand::Text(value) => turso::core::Value::from_text(value.clone()),
        SourceOperand::Binary(value) => turso::core::Value::from_blob(value.clone()),
    }
}

fn sqlite_value(value: turso::core::Value) -> SqliteValue {
    match value {
        turso::core::Value::Null => SqliteValue::Null,
        turso::core::Value::Numeric(turso::core::Numeric::Integer(value)) => {
            SqliteValue::Integer(value)
        }
        turso::core::Value::Numeric(turso::core::Numeric::Float(value)) => {
            SqliteValue::Real(value.into())
        }
        turso::core::Value::Text(value) => SqliteValue::Text(value.as_str().to_owned()),
        turso::core::Value::Blob(value) => SqliteValue::Blob(value),
    }
}

fn cell_value(value: SqliteValue) -> CellValue {
    match value {
        SqliteValue::Null => CellValue::Null,
        SqliteValue::Integer(value) => CellValue::Integer(value),
        SqliteValue::Real(value) => CellValue::Float(value),
        SqliteValue::Text(value) => CellValue::Text(value),
        SqliteValue::Blob(value) => CellValue::Binary(value),
    }
}

fn value_text(value: &SqliteValue) -> Option<&str> {
    match value {
        SqliteValue::Text(value) => Some(value),
        _ => None,
    }
}

fn value_i64(value: Option<&SqliteValue>) -> Option<i64> {
    match value {
        Some(SqliteValue::Integer(value)) => Some(*value),
        _ => None,
    }
}

struct TursoTableStore {
    session: Arc<SqliteSession>,
    definition: TableDefinition,
    identity_plan: SqliteIdentityPlan,
    active_query: SourceQuery,
    rows: Option<SqliteRows>,
    cached: Vec<Row>,
    identities: Vec<Option<StableRowIdentity>>,
    extent: Option<ResultExtent>,
    provenance: NativeQueryArtifact,
    pending_widenings: Vec<TypeWidening>,
}

impl TursoTableStore {
    fn start(
        session: Arc<SqliteSession>,
        definition: TableDefinition,
        identity_plan: SqliteIdentityPlan,
        query: SourceQuery,
    ) -> anyhow::Result<Self> {
        let embeddable_native_query = query
            .native_query
            .as_deref()
            .map(|sql| {
                session
                    .prepare_native_row_query(sql)
                    .map(|prepared| prepared.embeddable_sql)
            })
            .transpose()?;
        let compiled = compile_sqlite_query(
            &definition,
            &query,
            &identity_plan,
            embeddable_native_query.as_deref(),
        )?;
        let rows = session.query_rows(&compiled.execution_sql, &compiled.execution_parameters)?;
        Ok(Self {
            session,
            definition,
            identity_plan,
            active_query: query,
            rows: Some(rows),
            cached: Vec::new(),
            identities: Vec::new(),
            extent: None,
            provenance: compiled.provenance,
            pending_widenings: Vec::new(),
        })
    }

    fn fetch_next(&mut self) -> anyhow::Result<bool> {
        let Some(rows) = self.rows.as_mut() else {
            return Ok(false);
        };
        let next = rows.next()?;
        let Some(row) = next else {
            self.rows = None;
            self.extent = Some(ResultExtent::Complete {
                source_rows: self.cached.len(),
            });
            return Ok(false);
        };
        let limit = self.active_query.limit.get();
        if self.cached.len() >= limit {
            self.rows = None;
            self.extent = Some(ResultExtent::Truncated {
                source_rows: limit,
                limit,
            });
            return Ok(false);
        }
        let mut cells = row.into_iter().map(cell_value).collect::<Vec<_>>();
        let identity = match &self.identity_plan {
            SqliteIdentityPlan::RowId { .. } => match cells.pop() {
                Some(CellValue::Integer(value)) => Some(StableRowIdentity::SqliteRowId(value)),
                _ => None,
            },
            SqliteIdentityPlan::PrimaryKey { visible_indices } => {
                Some(StableRowIdentity::PrimaryKey(
                    visible_indices
                        .iter()
                        .map(|index| cells.get(*index).cloned().unwrap_or(CellValue::Null))
                        .collect(),
                ))
            }
            SqliteIdentityPlan::Unavailable => None,
        };
        for (index, value) in cells.iter().enumerate() {
            if value.logical_type() == LogicalType::Null {
                continue;
            }
            let Some(column) = self.definition.columns.get_mut(index) else {
                continue;
            };
            let widened = column.source_type.widen(value.logical_type());
            if widened != column.source_type {
                column.source_type = widened;
                self.pending_widenings
                    .retain(|item| item.column != column.id);
                self.pending_widenings.push(TypeWidening {
                    column: column.id,
                    source_type: widened,
                });
            }
        }
        let ordinal = self.cached.len();
        self.cached.push(Row::new(
            RowId {
                generation: self.definition.generation,
                ordinal: ordinal as u64,
            },
            cells,
        ));
        self.identities.push(identity);
        Ok(true)
    }

    fn fetch_through(&mut self, index: usize) -> anyhow::Result<()> {
        let target = index.min(self.active_query.limit.get().saturating_sub(1));
        while self.cached.len() <= target && self.fetch_next()? {}
        if self.cached.len() == self.active_query.limit.get() && self.extent.is_none() {
            let _ = self.fetch_next()?;
        }
        Ok(())
    }

    fn fetch_all(&mut self) -> anyhow::Result<()> {
        let limit = self.active_query.limit.get();
        self.fetch_through(limit.saturating_sub(1))
    }
}

impl TableStore for TursoTableStore {
    fn generation(&self) -> SourceGeneration {
        self.definition.generation
    }

    fn row_count(&self) -> RowCount {
        if let Some(extent) = self.extent {
            match extent {
                ResultExtent::Complete { source_rows }
                | ResultExtent::Truncated { source_rows, .. } => RowCount::Exact(source_rows),
            }
        } else if self.cached.is_empty() {
            RowCount::Unknown
        } else {
            RowCount::AtLeast(self.cached.len())
        }
    }

    fn column_count(&self) -> usize {
        self.definition.columns.len()
    }

    fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
        self.fetch_through(index.0)?;
        Ok(self.cached.get(index.0).cloned())
    }

    fn ensure_indexed_through(&mut self, index: RowIndex) -> anyhow::Result<IndexProgress> {
        self.fetch_through(index.0)?;
        Ok(IndexProgress {
            row_count: self.row_count(),
            schema_delta: SchemaDelta {
                widened_types: std::mem::take(&mut self.pending_widenings),
                ..SchemaDelta::default()
            },
            bytes_scanned: 0,
        })
    }

    fn scan_rows(
        &mut self,
        request: ScanRequest,
        visitor: &mut dyn RowVisitor,
    ) -> anyhow::Result<ScanProgress> {
        if request.max_rows == 0 {
            return Ok(ScanProgress {
                visited: 0,
                next: Some(request.start),
                reached_end: false,
            });
        }
        match request.direction {
            ScanDirection::Forward => {
                let end = request.start.0.saturating_add(request.max_rows);
                self.fetch_through(end.saturating_sub(1))?;
                let mut visited = 0;
                for index in request.start.0..end.min(self.cached.len()) {
                    visited += 1;
                    if visitor
                        .visit(RowIndex(index), &self.cached[index])
                        .is_break()
                    {
                        return Ok(ScanProgress {
                            visited,
                            next: Some(RowIndex(index.saturating_add(1))),
                            reached_end: false,
                        });
                    }
                }
                let next_index = request.start.0.saturating_add(visited);
                let reached_end = self.extent.is_some_and(|extent| match extent {
                    ResultExtent::Complete { source_rows }
                    | ResultExtent::Truncated { source_rows, .. } => next_index >= source_rows,
                });
                Ok(ScanProgress {
                    visited,
                    next: (!reached_end).then_some(RowIndex(next_index)),
                    reached_end,
                })
            }
            ScanDirection::Reverse => {
                self.fetch_through(request.start.0)?;
                let mut visited = 0;
                let mut current = request.start.0.min(self.cached.len().saturating_sub(1));
                loop {
                    visited += 1;
                    if visitor
                        .visit(RowIndex(current), &self.cached[current])
                        .is_break()
                        || visited >= request.max_rows
                        || current == 0
                    {
                        break;
                    }
                    current -= 1;
                }
                let reached_end = current == 0;
                Ok(ScanProgress {
                    visited,
                    next: (!reached_end).then_some(RowIndex(current.saturating_sub(1))),
                    reached_end,
                })
            }
        }
    }

    fn materialize(&mut self) -> anyhow::Result<InMemoryTable> {
        self.fetch_all()?;
        InMemoryTable::from_rows(self.definition.generation, self.cached.clone())
    }

    fn source_capabilities(&self) -> SourceOperationCapabilities {
        SourceOperationCapabilities {
            filters: vec![
                SourceFilterOperator::Equal,
                SourceFilterOperator::NotEqual,
                SourceFilterOperator::LessThan,
                SourceFilterOperator::LessThanOrEqual,
                SourceFilterOperator::GreaterThan,
                SourceFilterOperator::GreaterThanOrEqual,
                SourceFilterOperator::Contains,
                SourceFilterOperator::Prefix,
                SourceFilterOperator::IsNull,
                SourceFilterOperator::IsNotNull,
            ],
            sorting: CapabilityStatus::Supported,
            configurable_limit: true,
        }
    }

    fn active_source_query(&self) -> Option<&SourceQuery> {
        Some(&self.active_query)
    }

    fn execute_source_query(
        &mut self,
        query: &SourceQuery,
    ) -> anyhow::Result<SourceQueryExecution> {
        validate_source_query(&self.definition, query)?;
        let definition = self.definition.clone();
        let store = Box::new(Self::start(
            self.session.clone(),
            definition.clone(),
            self.identity_plan.clone(),
            query.clone(),
        )?) as Box<dyn TableStore>;
        Ok(SourceQueryExecution::SourceExecuted(
            crate::table::SourceResult::from_store(definition, store),
        ))
    }

    fn source_query_task(
        &mut self,
        query: SourceQuery,
    ) -> anyhow::Result<crate::table::SourceQueryTask> {
        validate_source_query(&self.definition, &query)?;
        let session = self.session.clone();
        let definition = self.definition.clone();
        let identity_plan = self.identity_plan.clone();
        Ok(crate::table::SourceQueryTask::Blocking(Box::new(
            move || {
                let (definition, identity_plan, query) = if let Some(native_query) =
                    query.native_query.as_deref()
                {
                    let replacement_definition = native_query_definition(&session, native_query)?;
                    let replacement_query =
                        remap_native_query(&query, &definition, &replacement_definition)?;
                    (
                        replacement_definition,
                        SqliteIdentityPlan::Unavailable,
                        replacement_query,
                    )
                } else {
                    (definition, identity_plan, query)
                };
                let store = Box::new(TursoTableStore::start(
                    session,
                    definition.clone(),
                    identity_plan,
                    query,
                )?) as Box<dyn TableStore>;
                Ok(crate::table::SourceResult::from_store(definition, store))
            },
        )))
    }

    fn result_extent(&self) -> Option<ResultExtent> {
        self.extent
    }

    fn query_provenance(&self) -> Option<&NativeQueryArtifact> {
        Some(&self.provenance)
    }

    fn stable_row_identity(
        &mut self,
        index: RowIndex,
    ) -> anyhow::Result<Option<StableRowIdentity>> {
        self.fetch_through(index.0)?;
        Ok(self.identities.get(index.0).cloned().flatten())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_database(path: &Path, statements: &[&str]) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let database = turso::Builder::new_local(path.to_str().unwrap())
                .experimental_generated_columns(true)
                .experimental_without_rowid(true)
                .build()
                .await
                .unwrap();
            let connection = database.connect().unwrap();
            for statement in statements {
                connection.execute(statement, ()).await.unwrap();
            }
        });
    }

    fn sidecar_path(path: &Path, suffix: &str) -> std::path::PathBuf {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        sidecar.into()
    }

    #[test]
    fn identifier_and_literal_rendering_are_safe() {
        assert_eq!(quote_identifier("a\"b"), "\"a\"\"b\"");
        assert_eq!(
            render_sql_literal(&SourceOperand::Text("O'Brien".to_owned())),
            "'O''Brien'"
        );
        assert_eq!(
            render_sql_literal(&SourceOperand::Binary(vec![0, 255])),
            "X'00FF'"
        );
    }

    #[test]
    fn affinity_rules_are_conservative_and_ordered() {
        assert_eq!(
            declared_type_hint(Some("FLOATING POINT")),
            LogicalType::Integer
        );
        assert_eq!(declared_type_hint(Some("VARCHAR(20)")), LogicalType::Text);
        assert_eq!(declared_type_hint(Some("DOUBLE")), LogicalType::Float);
        assert_eq!(declared_type_hint(Some("BOOLEAN")), LogicalType::Unknown);
        assert_eq!(declared_type_hint(Some("BLOB")), LogicalType::Binary);
        assert_eq!(declared_type_hint(None), LogicalType::Unknown);
        assert_eq!(declared_type_hint(Some("ANY")), LogicalType::Unknown);
    }

    #[test]
    fn query_only_rejects_mutation_and_reads_remain_available() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("readonly.db");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let database = turso::Builder::new_local(path.to_str().unwrap())
                .experimental_generated_columns(true)
                .experimental_without_rowid(true)
                .build()
                .await
                .unwrap();
            let connection = database.connect().unwrap();
            connection
                .execute("CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT)", ())
                .await
                .unwrap();
            connection
                .execute("INSERT INTO users(name) VALUES ('Ada')", ())
                .await
                .unwrap();
        });
        drop(runtime);

        let session = SqliteSession::open(&path).unwrap();
        assert!(session.mutation_is_rejected("DELETE FROM users"));
        let rows = session
            .query_values("SELECT name FROM users", Vec::new())
            .unwrap();
        assert_eq!(rows, vec![vec![SqliteValue::Text("Ada".to_owned())]]);
    }

    #[test]
    fn readonly_open_preserves_database_bytes_and_creates_no_sidecars() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("physical-readonly.db");
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT);
                     INSERT INTO users(name) VALUES ('Ada');",
                )
                .unwrap();
        }
        let sidecars =
            ["-journal", "-wal", "-shm", "-tshm", "-log"].map(|suffix| sidecar_path(&path, suffix));
        assert!(sidecars.iter().all(|sidecar| !sidecar.exists()));
        let before = std::fs::read(&path).unwrap();

        {
            let session = SqliteSession::open(&path).unwrap();
            let rows = session
                .query_values("SELECT name FROM users", Vec::new())
                .unwrap();
            assert_eq!(rows, vec![vec![SqliteValue::Text("Ada".to_owned())]]);
            assert!(session.mutation_is_rejected("DELETE FROM users"));
        }

        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(sidecars.iter().all(|sidecar| !sidecar.exists()));
    }

    #[test]
    fn readonly_open_reads_existing_wal_without_changing_database_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("existing-wal.db");
        let writer = rusqlite::Connection::open(&path).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA wal_autocheckpoint=0;
                 CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT);
                 INSERT INTO users(name) VALUES ('Ada');",
            )
            .unwrap();
        let database_path = path.clone();
        let wal_path = sidecar_path(&path, "-wal");
        let shm_path = sidecar_path(&path, "-shm");
        assert!(wal_path.exists());
        assert!(shm_path.exists());
        let before =
            [&database_path, &wal_path, &shm_path].map(|file| std::fs::read(file).unwrap());

        {
            let session = SqliteSession::open(&path).unwrap();
            let rows = session
                .query_values("SELECT name FROM users", Vec::new())
                .unwrap();
            assert_eq!(rows, vec![vec![SqliteValue::Text("Ada".to_owned())]]);
        }

        let after = [&database_path, &wal_path, &shm_path].map(|file| std::fs::read(file).unwrap());
        assert_eq!(after, before);
        drop(writer);
    }

    #[cfg(unix)]
    #[test]
    fn readonly_open_accepts_a_non_writable_database_and_directory() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("permissions.db");
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT);
                     INSERT INTO users(name) VALUES ('Ada');",
                )
                .unwrap();
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

        let result = SqliteSession::open(&path)
            .and_then(|session| session.query_values("SELECT name FROM users", Vec::new()));

        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            result.unwrap(),
            vec![vec![SqliteValue::Text("Ada".to_owned())]]
        );
    }

    #[test]
    fn adapter_discovers_selects_and_bounds_sqlite_relations() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.db");
        create_database(
            &path,
            &[
                "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT, payload BLOB)",
                "CREATE TABLE orders(id INTEGER PRIMARY KEY, user_id INTEGER)",
                "INSERT INTO users VALUES (1, 'Ada', x'00FF')",
                "INSERT INTO users VALUES (2, 'Grace', NULL)",
                "CREATE VIEW user_names AS SELECT name FROM users",
                "CREATE VIEW incompatible AS SELECT unavailable_function(name) AS value FROM users",
            ],
        );
        let source = SqliteAdapter
            .open(InputSource::Path(path.clone()), &OpenOptions::default())
            .unwrap();
        assert!(!source.has_selected_table());
        assert!(source
            .list_relations()
            .iter()
            .any(|entry| entry.metadata.name == "users" && entry.is_selectable()));
        assert!(source.list_relations().iter().any(|entry| {
            entry.metadata.name == "incompatible"
                && matches!(entry.availability, RelationAvailability::Unavailable { .. })
        }));
        let error = source
            .into_implicit_table()
            .err()
            .expect("ambiguous relation selection must fail");
        assert!(error.to_string().contains("multiple selectable relations"));

        let mut options = OpenOptions {
            table: Some("users".to_owned()),
            limit: NonZeroUsize::new(1),
            ..OpenOptions::default()
        };
        let mut table = SqliteAdapter
            .open(InputSource::Path(path), &options)
            .unwrap()
            .into_implicit_table()
            .unwrap();
        assert_eq!(
            table.definition.columns[0].source_declared_type.as_deref(),
            Some("INTEGER")
        );
        assert_eq!(table.definition.columns[2].source_type, LogicalType::Binary);
        let result = table.store.materialize().unwrap();
        assert_eq!(result.rows().len(), 1);
        assert!(matches!(
            table.store.result_extent(),
            Some(ResultExtent::Truncated {
                source_rows: 1,
                limit: 1
            })
        ));
        let provenance = table.store.query_provenance().unwrap();
        assert!(provenance.logical.ends_with("LIMIT 1"));
        assert!(!provenance.copyable.contains("LIMIT 2"));

        options.table = Some("missing".to_owned());
        let error = match SqliteAdapter.open(
            InputSource::Path(directory.path().join("catalog.db")),
            &options,
        ) {
            Ok(_) => panic!("missing relation unexpectedly opened"),
            Err(error) => error,
        };
        assert_eq!(error.to_string(), "relation 'missing' was not found");

        options.table = Some("incompatible".to_owned());
        let error = SqliteAdapter
            .open(
                InputSource::Path(directory.path().join("catalog.db")),
                &options,
            )
            .err()
            .expect("unavailable view unexpectedly opened");
        assert!(error
            .to_string()
            .starts_with("relation 'incompatible' is unavailable:"));

        options.table = Some("user_names".to_owned());
        let mut view = SqliteAdapter
            .open(
                InputSource::Path(directory.path().join("catalog.db")),
                &options,
            )
            .expect("selectable view")
            .into_implicit_table()
            .expect("selected view");
        assert_eq!(view.store.materialize().unwrap().rows().len(), 1);
    }

    #[test]
    fn compiler_handles_predicates_sort_parameters_and_stable_ties() {
        let generation = SourceGeneration::new();
        let definition = TableDefinition {
            generation,
            columns: vec![
                ColumnDefinition {
                    id: ColumnId {
                        generation,
                        ordinal: 0,
                    },
                    source_identity: ColumnSourceIdentity::RelationColumn {
                        relation: "odd \"table".to_owned(),
                        ordinal: 0,
                        name: "first name".to_owned(),
                    },
                    display_name: "first name".to_owned(),
                    source_declared_type: Some("TEXT".to_owned()),
                    source_type: LogicalType::Text,
                    type_origin: TypeOrigin::Declared,
                },
                ColumnDefinition {
                    id: ColumnId {
                        generation,
                        ordinal: 1,
                    },
                    source_identity: ColumnSourceIdentity::RelationColumn {
                        relation: "odd \"table".to_owned(),
                        ordinal: 1,
                        name: "age".to_owned(),
                    },
                    display_name: "age".to_owned(),
                    source_declared_type: Some("INTEGER".to_owned()),
                    source_type: LogicalType::Integer,
                    type_origin: TypeOrigin::Declared,
                },
            ],
            schema_state: SchemaState::Complete,
            relation: crate::table::RelationMetadata {
                name: "odd \"table".to_owned(),
                display_name: "odd \"table".to_owned(),
                header_visible: true,
            },
        };
        let mut query = SourceQuery::new(generation, NonZeroUsize::new(25).unwrap());
        query.filters = vec![
            SourceFilter {
                scope: SourceFilterScope::Column(definition.columns[0].id),
                operator: SourceFilterOperator::Contains,
                operand: Some(SourceOperand::Text("O'Brien".to_owned())),
            },
            SourceFilter {
                scope: SourceFilterScope::Column(definition.columns[1].id),
                operator: SourceFilterOperator::GreaterThanOrEqual,
                operand: Some(SourceOperand::Integer(21)),
            },
            SourceFilter {
                scope: SourceFilterScope::Column(definition.columns[1].id),
                operator: SourceFilterOperator::IsNotNull,
                operand: None,
            },
        ];
        query.order_by.push(SourceSort {
            column: definition.columns[0].id,
            direction: SortDirection::Descending,
        });
        let compiled = compile_sqlite_query(
            &definition,
            &query,
            &SqliteIdentityPlan::RowId {
                expression: "rowid".to_owned(),
            },
            None,
        )
        .unwrap();
        assert!(compiled
            .provenance
            .logical
            .contains("FROM \"odd \"\"table\""));
        assert!(compiled
            .provenance
            .logical
            .contains("instr(CAST(\"first name\" AS TEXT), ?1) > 0"));
        assert!(compiled
            .provenance
            .logical
            .contains("\"first name\" DESC, \"rowid\" ASC LIMIT 25"));
        assert!(compiled.provenance.copyable.contains("'O''Brien'"));
        assert!(compiled.execution_sql.ends_with("LIMIT 26"));
    }

    #[test]
    fn compiler_covers_the_complete_source_predicate_vocabulary() {
        let generation = SourceGeneration::new();
        let column = ColumnDefinition {
            id: ColumnId {
                generation,
                ordinal: 0,
            },
            source_identity: ColumnSourceIdentity::RelationColumn {
                relation: "items".to_owned(),
                ordinal: 0,
                name: "value".to_owned(),
            },
            display_name: "value".to_owned(),
            source_declared_type: None,
            source_type: LogicalType::Unknown,
            type_origin: TypeOrigin::Inferred,
        };
        let definition = TableDefinition {
            generation,
            columns: vec![column.clone()],
            schema_state: SchemaState::Complete,
            relation: crate::table::RelationMetadata {
                name: "items".to_owned(),
                display_name: "items".to_owned(),
                header_visible: true,
            },
        };
        let cases = [
            (SourceFilterOperator::Equal, " = ?1"),
            (SourceFilterOperator::NotEqual, " <> ?1"),
            (SourceFilterOperator::LessThan, " < ?1"),
            (SourceFilterOperator::LessThanOrEqual, " <= ?1"),
            (SourceFilterOperator::GreaterThan, " > ?1"),
            (SourceFilterOperator::GreaterThanOrEqual, " >= ?1"),
            (
                SourceFilterOperator::Contains,
                "instr(CAST(\"value\" AS TEXT), ?1) > 0",
            ),
            (
                SourceFilterOperator::Prefix,
                "instr(CAST(\"value\" AS TEXT), ?1) = 1",
            ),
        ];
        for (operator, expected) in cases {
            let mut query = SourceQuery::new(generation, NonZeroUsize::new(1).unwrap());
            query.filters.push(SourceFilter {
                scope: SourceFilterScope::Column(column.id),
                operator,
                operand: Some(SourceOperand::Text("x".to_owned())),
            });
            let compiled =
                compile_sqlite_query(&definition, &query, &SqliteIdentityPlan::Unavailable, None)
                    .unwrap();
            assert!(
                compiled.provenance.logical.contains(expected),
                "{}",
                compiled.provenance.logical
            );
            assert_eq!(
                compiled.provenance.parameters,
                vec![NativeQueryParameter::Value(SourceOperand::Text(
                    "x".to_owned()
                ))]
            );
        }
        for (operator, expected) in [
            (SourceFilterOperator::IsNull, "\"value\" IS NULL"),
            (SourceFilterOperator::IsNotNull, "\"value\" IS NOT NULL"),
        ] {
            let mut query = SourceQuery::new(generation, NonZeroUsize::new(1).unwrap());
            query.filters.push(SourceFilter {
                scope: SourceFilterScope::Column(column.id),
                operator,
                operand: None,
            });
            let compiled =
                compile_sqlite_query(&definition, &query, &SqliteIdentityPlan::Unavailable, None)
                    .unwrap();
            assert!(compiled.provenance.logical.contains(expected));
            assert!(compiled.provenance.parameters.is_empty());
        }
    }

    #[test]
    fn discovery_fixture_covers_sqlite_relation_and_metadata_edges() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture-matrix.db");
        create_database(
            &path,
            &[
                "CREATE TABLE \"odd table\"(id INTEGER PRIMARY KEY AUTOINCREMENT, i INT, t VARCHAR(20), r DOUBLE, b BLOB, n NUMERIC, anything ANY, contradictory INTEGER, generated TEXT GENERATED ALWAYS AS (t || '-g') VIRTUAL)",
                "INSERT INTO \"odd table\"(i,t,r,b,n,anything,contradictory) VALUES (1,'text',2.5,x'00FF',3.25,NULL,'not-an-integer')",
                "CREATE TABLE strict_values(id INTEGER PRIMARY KEY, value TEXT) STRICT",
                "CREATE TABLE composite(a TEXT, b INTEGER, PRIMARY KEY(a,b)) WITHOUT ROWID",
                "CREATE VIEW compatible AS SELECT t AS duplicate, generated AS duplicate FROM \"odd table\"",
                "CREATE VIEW data_dependent AS SELECT unavailable_function(t) AS value FROM \"odd table\"",
            ],
        );
        let fixture_connection = rusqlite::Connection::open(&path).unwrap();
        fixture_connection
            .execute_batch(
                "CREATE VIRTUAL TABLE search USING fts5(body);
                 CREATE VIRTUAL TABLE boxes USING rtree(id,min_x,max_x,min_y,max_y);
                 PRAGMA writable_schema=ON;
                 INSERT INTO sqlite_schema(type,name,tbl_name,rootpage,sql)
                   VALUES(
                     'table',
                     'missing_module',
                     'missing_module',
                     0,
                     'CREATE VIRTUAL TABLE missing_module USING no_such_module(value)'
                   );
                 PRAGMA writable_schema=OFF;
                 PRAGMA schema_version=100;",
            )
            .unwrap();
        drop(fixture_connection);

        let source = SqliteAdapter
            .open(InputSource::Path(path.clone()), &OpenOptions::default())
            .unwrap();
        let names = source
            .list_relations()
            .iter()
            .map(|entry| entry.metadata.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"odd table"));
        assert!(names.contains(&"strict_values"));
        assert!(names.contains(&"composite"));
        assert!(!names.contains(&"sqlite_sequence"));
        assert!(source.list_relations().iter().any(|entry| {
            entry.metadata.name == "data_dependent"
                && matches!(entry.availability, RelationAvailability::Unavailable { .. })
        }));
        for virtual_name in ["search", "boxes", "missing_module"] {
            assert!(source.list_relations().iter().any(|entry| {
                entry.metadata.name == virtual_name
                    && entry.kind == RelationKind::VirtualTable
                    && !entry.is_selectable()
            }));
        }
        assert!(!names.iter().any(|name| name.contains("_data")));

        let mut table = SqliteAdapter
            .open(
                InputSource::Path(path),
                &OpenOptions {
                    table: Some("odd table".to_owned()),
                    ..OpenOptions::default()
                },
            )
            .unwrap()
            .into_implicit_table()
            .unwrap();
        assert_eq!(table.definition.columns.len(), 9);
        assert_eq!(
            table.definition.columns[1].source_type,
            LogicalType::Integer
        );
        assert_eq!(table.definition.columns[2].source_type, LogicalType::Text);
        assert_eq!(table.definition.columns[3].source_type, LogicalType::Float);
        assert_eq!(table.definition.columns[4].source_type, LogicalType::Binary);
        assert_eq!(
            table.definition.columns[5].source_type,
            LogicalType::Unknown
        );
        assert_eq!(
            table.definition.columns[6].source_type,
            LogicalType::Unknown
        );
        let progress = table.store.ensure_indexed_through(RowIndex(0)).unwrap();
        assert!(progress
            .schema_delta
            .widened_types
            .iter()
            .any(|widening| widening.column == table.definition.columns[7].id));

        let empty_path = directory.path().join("empty.db");
        create_database(&empty_path, &[]);
        let error = SqliteAdapter
            .open(InputSource::Path(empty_path), &OpenOptions::default())
            .err()
            .expect("empty database should fail clearly");
        assert!(error.to_string().contains("no user tables or views"));
    }

    fn logical_snapshot(path: &Path) -> Vec<Vec<SqliteValue>> {
        let session = SqliteSession::open(path).unwrap();
        let mut snapshot = session
            .query_values(
                "SELECT type, name, tbl_name, sql FROM sqlite_schema ORDER BY type, name",
                Vec::new(),
            )
            .unwrap();
        snapshot.extend(
            session
                .query_values("SELECT id, name FROM users ORDER BY id", Vec::new())
                .unwrap(),
        );
        snapshot
    }

    #[test]
    fn supported_actions_preserve_logical_snapshots_in_rollback_and_wal_modes() {
        for (name, journal_mode) in [("rollback", "DELETE"), ("wal", "WAL")] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join(format!("{name}.db"));
            create_database(
                &path,
                &[
                    "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT)",
                    "INSERT INTO users VALUES (1, 'Ada')",
                    "INSERT INTO users VALUES (2, 'Grace')",
                ],
            );
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async {
                let database = turso::Builder::new_local(path.to_str().unwrap())
                    .build()
                    .await
                    .unwrap();
                let connection = database.connect().unwrap();
                let mut rows = connection
                    .query(&format!("PRAGMA journal_mode={journal_mode}"), ())
                    .await
                    .unwrap();
                assert!(rows.next().await.unwrap().is_some());
            });
            let before = logical_snapshot(&path);
            {
                let mut table = SqliteAdapter
                    .open(
                        InputSource::Path(path.clone()),
                        &OpenOptions {
                            table: Some("users".to_owned()),
                            ..OpenOptions::default()
                        },
                    )
                    .unwrap()
                    .into_implicit_table()
                    .unwrap();
                assert_eq!(table.store.materialize().unwrap().rows().len(), 2);
                let mut query = table.store.active_source_query().unwrap().clone();
                query.order_by.push(SourceSort {
                    column: table.definition.columns[1].id,
                    direction: SortDirection::Descending,
                });
                let mut replacement = match table.store.execute_source_query(&query).unwrap() {
                    SourceQueryExecution::SourceExecuted(store) => store,
                    _ => panic!("SQLite query was not source executed"),
                };
                assert_eq!(replacement.store.materialize().unwrap().rows().len(), 2);
            }
            assert_eq!(logical_snapshot(&path), before, "{name}");
        }
    }

    #[test]
    fn relational_duplicate_keys_and_without_rowid_identity_are_stable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("identity.db");
        create_database(
            &path,
            &[
                "CREATE TABLE keyed(part_a TEXT, part_b INTEGER, value ANY, PRIMARY KEY(part_a, part_b)) WITHOUT ROWID",
                "INSERT INTO keyed VALUES ('a', 1, 'text')",
                "INSERT INTO keyed VALUES ('b', 2, 3.5)",
            ],
        );
        let mut table = SqliteAdapter
            .open(
                InputSource::Path(path),
                &OpenOptions {
                    table: Some("keyed".to_owned()),
                    ..OpenOptions::default()
                },
            )
            .unwrap()
            .into_implicit_table()
            .unwrap();
        assert_eq!(
            table.definition.columns[2].source_type,
            LogicalType::Unknown
        );
        let first_identity = table
            .store
            .stable_row_identity(RowIndex(0))
            .unwrap()
            .unwrap();
        assert!(matches!(
            first_identity,
            StableRowIdentity::PrimaryKey(values)
                if values == vec![CellValue::Text("a".to_owned()), CellValue::Integer(1)]
        ));
    }

    #[test]
    fn native_select_and_cte_queries_open_as_implicit_bounded_relations() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native.db");
        create_database(
            &path,
            &[
                "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT, active INTEGER)",
                "INSERT INTO users VALUES (1, 'Ada', 1)",
                "INSERT INTO users VALUES (2, 'Grace', 0)",
            ],
        );
        for sql in [
            "SELECT id, name FROM users WHERE active = 1",
            "SELECT id, name FROM users WHERE active = 1;",
            "SELECT id, name FROM users WHERE active = 1; -- trailing comment",
            "SELECT id, name FROM users WHERE active = 1; /* trailing comment */",
            "WITH active AS (SELECT * FROM users WHERE active = 1) SELECT name FROM active",
        ] {
            let opened = SqliteAdapter
                .open(
                    InputSource::Path(path.clone()),
                    &OpenOptions {
                        native_query: Some(sql.to_owned()),
                        ..OpenOptions::default()
                    },
                )
                .unwrap_or_else(|error| panic!("{sql}: {error}"))
                .into_implicit_table()
                .unwrap_or_else(|error| panic!("{sql}: {error}"));
            assert_eq!(opened.generation, opened.definition.generation);
            let mut table = opened;
            assert_eq!(table.store.materialize().unwrap().rows().len(), 1);
            assert_eq!(
                table
                    .store
                    .active_source_query()
                    .unwrap()
                    .native_query
                    .as_deref(),
                Some(sql)
            );
            assert!(table
                .store
                .stable_row_identity(RowIndex(0))
                .unwrap()
                .is_none());
        }
    }

    #[test]
    fn native_sql_rejects_writes_multiple_statements_non_rows_and_parameters() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native_reject.db");
        create_database(
            &path,
            &[
                "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT)",
                "INSERT INTO users VALUES (1, 'Ada')",
            ],
        );
        let before = std::fs::read(&path).unwrap();
        for (sql, expected) in [
            ("DELETE FROM users", "read-only"),
            (
                "SELECT * FROM users; DELETE FROM users",
                "exactly one statement",
            ),
            ("BEGIN", "tabular result"),
            ("SELECT * FROM users WHERE id = ?1", "unbound parameters"),
        ] {
            let error = SqliteAdapter
                .open(
                    InputSource::Path(path.clone()),
                    &OpenOptions {
                        native_query: Some(sql.to_owned()),
                        ..OpenOptions::default()
                    },
                )
                .err()
                .expect("native SQL must be rejected");
            assert!(
                error.to_string().contains(expected),
                "{sql}: expected {expected}, got {error}"
            );
            assert_eq!(std::fs::read(&path).unwrap(), before, "{sql}");
        }
        let connection = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM users", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn native_sql_composes_filters_sort_and_limit_around_the_base_query() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native_compose.db");
        create_database(
            &path,
            &[
                "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT)",
                "INSERT INTO users VALUES (1, 'Ada')",
                "INSERT INTO users VALUES (2, 'Grace')",
                "INSERT INTO users VALUES (3, 'Alan')",
            ],
        );
        let mut table = SqliteAdapter
            .open(
                InputSource::Path(path),
                &OpenOptions {
                    native_query: Some(
                        "SELECT id, upper(name) AS display_name FROM users LIMIT 3".to_owned(),
                    ),
                    limit: NonZeroUsize::new(1),
                    source_filters: vec![SourceFilterRequest {
                        column: "display_name".to_owned(),
                        operator: SourceFilterOperator::Prefix,
                        operand: Some(SourceOperand::Text("A".to_owned())),
                    }],
                    source_sort: vec![SourceSortRequest {
                        column: "display_name".to_owned(),
                        direction: SortDirection::Descending,
                    }],
                    ..OpenOptions::default()
                },
            )
            .unwrap()
            .into_implicit_table()
            .unwrap();
        let materialized = table.store.materialize().unwrap();
        assert_eq!(materialized.rows().len(), 1);
        assert_eq!(materialized.rows()[0].cells[1].display(), "ALAN");
        let provenance = table.store.query_provenance().unwrap();
        assert!(provenance.logical.contains("FROM (\nSELECT"));
        assert!(provenance.logical.contains("ORDER BY"));
        assert!(provenance.logical.ends_with("LIMIT 1"));
        assert!(!provenance.copyable.contains("LIMIT 2"));
    }

    #[test]
    fn duplicate_native_result_names_are_allowed_until_addressed_by_source_operations() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native_duplicates.db");
        create_database(
            &path,
            &[
                "CREATE TABLE users(id INTEGER)",
                "INSERT INTO users VALUES (1)",
            ],
        );
        let base = "SELECT id AS duplicate, id AS duplicate FROM users";
        SqliteAdapter
            .open(
                InputSource::Path(path.clone()),
                &OpenOptions {
                    native_query: Some(base.to_owned()),
                    ..OpenOptions::default()
                },
            )
            .expect("duplicate output is renderable");
        let error = SqliteAdapter
            .open(
                InputSource::Path(path),
                &OpenOptions {
                    native_query: Some(base.to_owned()),
                    source_sort: vec![SourceSortRequest {
                        column: "duplicate".to_owned(),
                        direction: SortDirection::Ascending,
                    }],
                    ..OpenOptions::default()
                },
            )
            .err()
            .expect("ambiguous result column");
        assert!(error.to_string().contains("ambiguous"));
    }

    #[test]
    fn layered_operations_apply_source_sort_before_limit_and_never_refill_view() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("layers.db");
        create_database(
            &path,
            &[
                "CREATE TABLE events(id INTEGER PRIMARY KEY, name TEXT)",
                "INSERT INTO events VALUES (1, 'a')",
                "INSERT INTO events VALUES (2, 'b')",
                "INSERT INTO events VALUES (3, 'c')",
                "INSERT INTO events VALUES (4, 'd')",
            ],
        );
        let opened = SqliteAdapter
            .open(
                InputSource::Path(path),
                &OpenOptions {
                    table: Some("events".to_owned()),
                    limit: NonZeroUsize::new(2),
                    source_sort: vec![SourceSortRequest {
                        column: "id".to_owned(),
                        direction: SortDirection::Descending,
                    }],
                    ..OpenOptions::default()
                },
            )
            .unwrap()
            .into_implicit_table()
            .unwrap();
        let mut view =
            crate::view::TableView::from_opened_table(opened, crate::view::Viewport::new(10, 80))
                .unwrap();
        assert_eq!(view.rows()[0][1], "d");
        assert_eq!(view.rows()[1][1], "c");
        view.apply_filter(
            1,
            crate::ops::filter::FilterMode::In,
            crate::ops::filter::FilterKind::Text,
            "c".to_owned(),
        )
        .unwrap();
        assert_eq!(view.visible_row_count(), 1);
        assert_eq!(view.fetched_source_row_count(), 2);
        assert!(view
            .progressive_search("b", crate::ops::search::SearchDirection::Forward)
            .is_none());
        assert_eq!(view.fetched_source_row_count(), 2);
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn sqlite_source_and_view_state_round_trip_through_nested_saved_yaml() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("saved.db");
        create_database(
            &path,
            &[
                "CREATE TABLE events(id INTEGER PRIMARY KEY, name TEXT, active INTEGER)",
                "INSERT INTO events VALUES (1, 'alpha', 1)",
                "INSERT INTO events VALUES (2, 'beta', 0)",
            ],
        );
        let options = OpenOptions {
            format: InputFormat::Sqlite,
            table: Some("events".to_owned()),
            limit: NonZeroUsize::new(25),
            source_filters: vec![SourceFilterRequest {
                column: "active".to_owned(),
                operator: SourceFilterOperator::Equal,
                operand: Some(SourceOperand::Boolean(true)),
            }],
            source_sort: vec![SourceSortRequest {
                column: "id".to_owned(),
                direction: SortDirection::Descending,
            }],
            ..OpenOptions::default()
        };
        let opened = SqliteAdapter
            .open(InputSource::Path(path), &options)
            .unwrap()
            .into_implicit_table()
            .unwrap();
        let mut view =
            crate::view::TableView::from_opened_table(opened, crate::view::Viewport::new(10, 80))
                .unwrap();
        view.set_view_null_placement(Some(crate::table::NullPlacement::First));
        view.apply_filter(
            1,
            crate::ops::filter::FilterMode::In,
            crate::ops::filter::FilterKind::Regex,
            "^a".to_owned(),
        )
        .unwrap();
        view.goto(0, 1);
        view.sort_current_column(
            crate::ops::sort::SortMode::Natural,
            crate::ops::sort::SortDirection::Ascending,
        );

        let yaml = view.to_saved_view_yaml_with_source_options(
            "events",
            "saved.db",
            Some("en_US"),
            &options,
        );
        let parsed = crate::saved_views::parse_saved_view_yaml(&yaml).unwrap();
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(parsed.view.source.format, Some(InputFormat::Sqlite));
        assert_eq!(parsed.view.source.table.as_deref(), Some("events"));
        assert_eq!(parsed.view.source.limit, Some(25));
        assert_eq!(parsed.view.source.filters.len(), 1);
        assert_eq!(parsed.view.source.sort.len(), 1);
        assert_eq!(parsed.view.view.locale.as_deref(), Some("en_US"));
        assert_eq!(
            parsed.view.view.nulls,
            Some(crate::table::NullPlacement::First)
        );
        assert_eq!(parsed.view.view.filters.len(), 1);
        assert_eq!(parsed.view.view.sort.len(), 1);
    }

    #[test]
    fn asynchronous_replacement_preserves_keyed_cursor_and_failure_keeps_result() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("replace.db");
        create_database(
            &path,
            &[
                "CREATE TABLE events(id INTEGER PRIMARY KEY, name TEXT)",
                "INSERT INTO events VALUES (1, 'a')",
                "INSERT INTO events VALUES (2, 'b')",
                "INSERT INTO events VALUES (3, 'c')",
            ],
        );
        let opened = SqliteAdapter
            .open(
                InputSource::Path(path),
                &OpenOptions {
                    table: Some("events".to_owned()),
                    ..OpenOptions::default()
                },
            )
            .unwrap()
            .into_implicit_table()
            .unwrap();
        let mut view =
            crate::view::TableView::from_opened_table(opened, crate::view::Viewport::new(10, 80))
                .unwrap();
        view.set_mark();
        let mut query = view.active_source_query().unwrap().clone();
        query.order_by = vec![SourceSort {
            column: view.table_definition().unwrap().columns[0].id,
            direction: SortDirection::Descending,
        }];
        assert!(view.request_source_query(query));
        view.await_latest_source_query().unwrap();
        assert_eq!(view.current_raw_cell(), Some("1"));
        assert_eq!(view.cursor().row, 2);
        assert_eq!(view.mark().unwrap().row, 2);

        let prior_rows = view.rows().to_vec();
        let mut invalid = view.active_source_query().unwrap().clone();
        invalid.filters.push(SourceFilter {
            scope: SourceFilterScope::WholeRecord,
            operator: SourceFilterOperator::Contains,
            operand: Some(SourceOperand::Text("a".to_owned())),
        });
        assert!(view.request_source_query(invalid));
        assert!(view.await_latest_source_query().is_err());
        assert_eq!(view.rows(), prior_rows);
    }
}
