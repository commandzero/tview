use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use elasticsearch::auth::Credentials;
use elasticsearch::cert::{Certificate, CertificateValidation};
use elasticsearch::http::transport::{SingleNodeConnectionPool, TransportBuilder};
use elasticsearch::indices::{IndicesGetMappingParts, IndicesResolveIndexParts};
use elasticsearch::params::ExpandWildcards;
use elasticsearch::{Elasticsearch, FieldCapsParts};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::table::{
    validate_source_query, CapabilityStatus, CellValue, ColumnDefinition, ColumnId,
    ColumnSourceIdentity, InMemoryTable, IndexProgress, LogicalType, NativeQueryArtifact,
    NativeQueryLanguage, NativeQueryParameter, ResultExtent, Row, RowCount, RowIndex, RowVisitor,
    ScanProgress, ScanRequest, SchemaState, SortDirection, SourceFieldMetadata,
    SourceFilterOperator, SourceFilterScope, SourceGeneration, SourceOperand,
    SourceOperationCapabilities, SourceQuery, SourceQueryExecution, SourceSort, StableRowIdentity,
    TableDefinition, TableStore, TypeOrigin,
};

use super::adapter::{
    OpenedSource, OpenedTable, ProbeResult, RelationAvailability, RelationCatalogEntry,
    RelationKind, RelationOpener, SourceAdapter,
};
use super::source::InputSource;
use super::{InputFormat, OpenOptions, SourceFilterRequest, SourceSortRequest};

pub const DEFAULT_ELASTICSEARCH_SOURCE_LIMIT: usize = 1_000;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct ElasticsearchAdapter;

impl SourceAdapter for ElasticsearchAdapter {
    fn format(&self) -> InputFormat {
        InputFormat::Elasticsearch
    }

    fn probe(&self, _source: &InputSource, _sample: &[u8]) -> ProbeResult {
        ProbeResult::NoMatch
    }

    fn open(&self, source: InputSource, options: &OpenOptions) -> anyhow::Result<OpenedSource> {
        if options.native_query.is_none() {
            if let Some(target) = options.table.as_deref() {
                validate_from_target(target)?;
            }
        }
        let (client, identity) = match source {
            InputSource::ElasticContext(target) => {
                let connection = super::elastic_context::resolve_connection(&target)?;
                (
                    connection.client.clone(),
                    ElasticsearchSourceIdentity {
                        value: connection.safe_identity.clone(),
                        context: true,
                    },
                )
            }
            InputSource::Url(url) if matches!(url.scheme(), "http" | "https") => {
                let client = Arc::new(build_client(&url)?);
                let identity = ElasticsearchSourceIdentity {
                    value: InputSource::Url(url).safe_identity().into(),
                    context: false,
                };
                (client, identity)
            }
            InputSource::Url(_)
            | InputSource::Path(_)
            | InputSource::Stdin
            | InputSource::StreamingStdin(_) => {
                anyhow::bail!(
                    "Elasticsearch input requires an HTTP(S) endpoint or dot-context target"
                )
            }
        };

        if let Some(base) = options.native_query.as_deref() {
            let opened =
                execute_opened_table(client, identity, None, Some(base.to_owned()), None, options)?;
            return Ok(OpenedSource::implicit(opened));
        }

        if let Some(target) = options.table.as_deref() {
            let catalog = fetch_field_catalog(client.clone(), identity.clone(), target)?;
            let opened = execute_opened_table(
                client,
                identity,
                Some(target.to_owned()),
                None,
                Some(catalog),
                options,
            )?;
            return Ok(OpenedSource::implicit(opened));
        }

        let targets = discover_targets(client.clone(), identity.clone())?;
        let relations = targets
            .iter()
            .map(|target| RelationCatalogEntry {
                metadata: crate::table::RelationMetadata {
                    name: target.name.clone(),
                    display_name: target.name.clone(),
                    header_visible: true,
                },
                kind: match target.kind {
                    ElasticsearchTargetKind::Index => RelationKind::Index,
                    ElasticsearchTargetKind::DataStream => RelationKind::DataStream,
                },
                availability: RelationAvailability::Selectable,
            })
            .collect::<Vec<_>>();
        Ok(OpenedSource::relational(
            relations,
            None,
            Box::new(ElasticsearchRelationOpener {
                client,
                identity,
                options: options.clone(),
            }),
        ))
    }
}

struct ElasticsearchRelationOpener {
    client: Arc<Elasticsearch>,
    identity: ElasticsearchSourceIdentity,
    options: OpenOptions,
}

impl RelationOpener for ElasticsearchRelationOpener {
    fn open_relation(&mut self, name: &str) -> anyhow::Result<OpenedTable> {
        let catalog = fetch_field_catalog(self.client.clone(), self.identity.clone(), name)?;
        execute_opened_table(
            self.client.clone(),
            self.identity.clone(),
            Some(name.to_owned()),
            None,
            Some(catalog),
            &self.options,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElasticsearchTargetKind {
    Index,
    DataStream,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElasticsearchTarget {
    pub name: String,
    pub kind: ElasticsearchTargetKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ElasticsearchField {
    pub name: String,
    pub mapping_types: BTreeSet<String>,
    pub capability_types: BTreeSet<String>,
    pub searchable: bool,
    pub aggregatable: bool,
    pub conflict: bool,
    pub runtime: bool,
    pub object: bool,
    pub nested: bool,
    pub multifield: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ElasticsearchFieldCatalog {
    pub fields: BTreeMap<String, ElasticsearchField>,
}

#[derive(Clone)]
struct ElasticsearchSourceIdentity {
    value: Arc<str>,
    context: bool,
}

impl ElasticsearchSourceIdentity {
    fn failure(&self, operation: &str, error: impl Into<anyhow::Error>) -> anyhow::Error {
        if self.context {
            anyhow::anyhow!("{operation} failed for {}", self.value)
        } else {
            error.into()
        }
    }
}

fn build_client(endpoint: &url::Url) -> anyhow::Result<Elasticsearch> {
    let settings = transport_settings_from_env()?;
    build_client_with_settings(endpoint, settings, REQUEST_TIMEOUT)
}

pub(super) fn build_context_client(
    endpoint: &url::Url,
    auth: elasticrc::Auth,
) -> anyhow::Result<Elasticsearch> {
    let credentials = match auth {
        elasticrc::Auth::ApiKey(api_key) => {
            Some(Credentials::EncodedApiKey(api_key.expose_secret().clone()))
        }
        elasticrc::Auth::Basic { username, password } => Some(Credentials::Basic(
            username,
            password.expose_secret().clone(),
        )),
        elasticrc::Auth::None => None,
    };
    build_client_with_credentials(
        endpoint,
        credentials,
        std::env::var_os("ELASTIC_CA_CERT").map(std::path::PathBuf::from),
        REQUEST_TIMEOUT,
    )
}

#[derive(Clone, PartialEq, Eq)]
enum ElasticsearchAuth {
    ApiKey(String),
    Basic { username: String, password: String },
}

impl std::fmt::Debug for ElasticsearchAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApiKey(_) => formatter.write_str("ApiKey([REDACTED])"),
            Self::Basic { .. } => formatter.write_str("Basic([REDACTED])"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ElasticsearchTransportSettings {
    auth: Option<ElasticsearchAuth>,
    ca_cert: Option<std::path::PathBuf>,
}

fn transport_settings_from_env() -> anyhow::Result<ElasticsearchTransportSettings> {
    transport_settings(
        std::env::var("ELASTIC_API_KEY").ok(),
        std::env::var("ELASTIC_USERNAME").ok(),
        std::env::var("ELASTIC_PASSWORD").ok(),
        std::env::var_os("ELASTIC_CA_CERT").map(std::path::PathBuf::from),
    )
}

fn transport_settings(
    api_key: Option<String>,
    username: Option<String>,
    password: Option<String>,
    ca_cert: Option<std::path::PathBuf>,
) -> anyhow::Result<ElasticsearchTransportSettings> {
    let api_key = api_key.filter(|value| !value.is_empty());
    let username = username.filter(|value| !value.is_empty());
    let password = password.filter(|value| !value.is_empty());
    if api_key.is_some() && (username.is_some() || password.is_some()) {
        anyhow::bail!("ELASTIC_API_KEY conflicts with ELASTIC_USERNAME/ELASTIC_PASSWORD");
    }
    if username.is_some() != password.is_some() {
        anyhow::bail!("ELASTIC_USERNAME and ELASTIC_PASSWORD must be set together");
    }
    let auth = if let Some(api_key) = api_key {
        Some(ElasticsearchAuth::ApiKey(api_key))
    } else if let (Some(username), Some(password)) = (username, password) {
        Some(ElasticsearchAuth::Basic { username, password })
    } else {
        None
    };
    Ok(ElasticsearchTransportSettings { auth, ca_cert })
}

fn build_client_with_settings(
    endpoint: &url::Url,
    settings: ElasticsearchTransportSettings,
    timeout: Duration,
) -> anyhow::Result<Elasticsearch> {
    let credentials = settings.auth.map(|auth| match auth {
        ElasticsearchAuth::ApiKey(api_key) => Credentials::EncodedApiKey(api_key),
        ElasticsearchAuth::Basic { username, password } => Credentials::Basic(username, password),
    });
    build_client_with_credentials(endpoint, credentials, settings.ca_cert, timeout)
}

fn build_client_with_credentials(
    endpoint: &url::Url,
    credentials: Option<Credentials>,
    ca_cert: Option<std::path::PathBuf>,
    timeout: Duration,
) -> anyhow::Result<Elasticsearch> {
    if !matches!(endpoint.scheme(), "http" | "https") {
        anyhow::bail!("Elasticsearch endpoint must use HTTP(S)");
    }
    if !endpoint.username().is_empty() || endpoint.password().is_some() {
        anyhow::bail!("Elasticsearch endpoint credentials are unsupported; configure authentication separately");
    }
    let pool = SingleNodeConnectionPool::new(endpoint.clone());
    let mut builder = TransportBuilder::new(pool).timeout(timeout);
    if let Some(credentials) = credentials {
        builder = builder.auth(credentials);
    }
    if let Some(ca_path) = ca_cert {
        let bytes = std::fs::read(ca_path).map_err(|_| {
            anyhow::anyhow!("failed to read ELASTIC_CA_CERT; check file permissions")
        })?;
        let certificate = Certificate::from_pem(&bytes)
            .map_err(|_| anyhow::anyhow!("invalid ELASTIC_CA_CERT; expected a PEM certificate"))?;
        builder = builder.cert_validation(CertificateValidation::Full(certificate));
    }
    let transport = builder
        .build()
        .map_err(|_| anyhow::anyhow!("failed to construct Elasticsearch transport"))?;
    Ok(Elasticsearch::new(transport))
}

fn run_async<T, F>(future: F) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: Future<Output = anyhow::Result<T>> + Send + 'static,
{
    std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .map_err(anyhow::Error::from)?
            .block_on(future)
    })
    .join()
    .map_err(|_| anyhow::anyhow!("Elasticsearch worker panicked"))?
}

fn discover_targets(
    client: Arc<Elasticsearch>,
    identity: ElasticsearchSourceIdentity,
) -> anyhow::Result<Vec<ElasticsearchTarget>> {
    run_async(async move {
        let names = ["*"];
        let expand = [ExpandWildcards::Open];
        let response = client
            .indices()
            .resolve_index(IndicesResolveIndexParts::Name(&names))
            .expand_wildcards(&expand)
            .ignore_unavailable(true)
            .request_timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|error| identity.failure("resolve Elasticsearch targets", error))?;
        let value = response_json(response, "resolve Elasticsearch targets", &identity).await?;
        parse_resolved_targets(&value)
    })
}

fn parse_resolved_targets(value: &Value) -> anyhow::Result<Vec<ElasticsearchTarget>> {
    let mut targets = Vec::new();
    for entry in value
        .get("indices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(name) = entry.get("name").and_then(Value::as_str) else {
            continue;
        };
        let attributes = entry
            .get("attributes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>();
        if name.starts_with('.')
            || attributes.contains("hidden")
            || attributes.contains("closed")
            || (!attributes.is_empty() && !attributes.contains("open"))
        {
            continue;
        }
        targets.push(ElasticsearchTarget {
            name: name.to_owned(),
            kind: ElasticsearchTargetKind::Index,
        });
    }
    for entry in value
        .get("data_streams")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(name) = entry.get("name").and_then(Value::as_str) else {
            continue;
        };
        let hidden = entry
            .get("hidden")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if name.starts_with('.') || hidden {
            continue;
        }
        targets.push(ElasticsearchTarget {
            name: name.to_owned(),
            kind: ElasticsearchTargetKind::DataStream,
        });
    }
    targets.sort_by(|left, right| {
        let left_kind = matches!(left.kind, ElasticsearchTargetKind::DataStream);
        let right_kind = matches!(right.kind, ElasticsearchTargetKind::DataStream);
        left_kind
            .cmp(&right_kind)
            .then_with(|| left.name.cmp(&right.name))
    });
    targets.dedup();
    Ok(targets)
}

fn fetch_field_catalog(
    client: Arc<Elasticsearch>,
    identity: ElasticsearchSourceIdentity,
    target: &str,
) -> anyhow::Result<ElasticsearchFieldCatalog> {
    let target = target.to_owned();
    run_async(async move {
        let indices = [target.as_str()];
        let expand = [ExpandWildcards::Open];
        let mappings_response = client
            .indices()
            .get_mapping(IndicesGetMappingParts::Index(&indices))
            .expand_wildcards(&expand)
            .request_timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|error| identity.failure("read Elasticsearch mappings", error))?;
        let mappings =
            response_json(mappings_response, "read Elasticsearch mappings", &identity).await?;
        let field_caps_response = client
            .field_caps(FieldCapsParts::Index(&indices))
            .fields(&["*"])
            .expand_wildcards(&expand)
            .request_timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|error| identity.failure("read Elasticsearch field capabilities", error))?;
        let field_caps = response_json(
            field_caps_response,
            "read Elasticsearch field capabilities",
            &identity,
        )
        .await?;
        Ok(parse_field_catalog(&mappings, &field_caps))
    })
}

fn parse_field_catalog(mappings: &Value, field_caps: &Value) -> ElasticsearchFieldCatalog {
    let mut catalog = ElasticsearchFieldCatalog::default();
    if let Some(indices) = mappings.as_object() {
        for index in indices.values() {
            let mapping = index.get("mappings").unwrap_or(index);
            if let Some(properties) = mapping.get("properties").and_then(Value::as_object) {
                flatten_mapping_properties("", properties, false, &mut catalog);
            }
            if let Some(runtime) = mapping.get("runtime").and_then(Value::as_object) {
                for (name, definition) in runtime {
                    let field =
                        catalog
                            .fields
                            .entry(name.clone())
                            .or_insert_with(|| ElasticsearchField {
                                name: name.clone(),
                                ..ElasticsearchField::default()
                            });
                    field.runtime = true;
                    if let Some(field_type) = definition.get("type").and_then(Value::as_str) {
                        field.mapping_types.insert(field_type.to_owned());
                    }
                }
            }
        }
    }
    if let Some(fields) = field_caps.get("fields").and_then(Value::as_object) {
        for (name, types) in fields {
            let field = catalog
                .fields
                .entry(name.clone())
                .or_insert_with(|| ElasticsearchField {
                    name: name.clone(),
                    ..ElasticsearchField::default()
                });
            if let Some(types) = types.as_object() {
                for (field_type, capabilities) in types {
                    field.capability_types.insert(field_type.clone());
                    field.searchable |= capabilities
                        .get("searchable")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    field.aggregatable |= capabilities
                        .get("aggregatable")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                }
            }
            field.conflict = field.capability_types.len() > 1
                || field.mapping_types.len() > 1
                || (!field.mapping_types.is_empty()
                    && !field.capability_types.is_empty()
                    && field.mapping_types != field.capability_types);
        }
    }
    catalog
}

fn source_field_metadata(catalog: &ElasticsearchFieldCatalog) -> Vec<SourceFieldMetadata> {
    catalog
        .fields
        .values()
        .map(|field| SourceFieldMetadata {
            name: field.name.clone(),
            source_types: field
                .mapping_types
                .union(&field.capability_types)
                .cloned()
                .collect(),
            searchable: field.searchable,
            aggregatable: field.aggregatable,
            conflict: field.conflict,
            runtime: field.runtime,
            nested: field.nested,
            multifield: field.multifield,
        })
        .collect()
}

fn flatten_mapping_properties(
    prefix: &str,
    properties: &serde_json::Map<String, Value>,
    multifield: bool,
    catalog: &mut ElasticsearchFieldCatalog,
) {
    for (name, definition) in properties {
        let full_name = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        let field_type = definition
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                if definition.get("properties").is_some() {
                    "object"
                } else {
                    "unknown"
                }
            });
        let field = catalog
            .fields
            .entry(full_name.clone())
            .or_insert_with(|| ElasticsearchField {
                name: full_name.clone(),
                ..ElasticsearchField::default()
            });
        field.mapping_types.insert(field_type.to_owned());
        field.object |= field_type == "object";
        field.nested |= field_type == "nested";
        field.multifield |= multifield;
        if let Some(children) = definition.get("properties").and_then(Value::as_object) {
            flatten_mapping_properties(&full_name, children, false, catalog);
        }
        if let Some(fields) = definition.get("fields").and_then(Value::as_object) {
            flatten_mapping_properties(&full_name, fields, true, catalog);
        }
    }
}

async fn response_json(
    response: elasticsearch::http::response::Response,
    operation: &str,
    identity: &ElasticsearchSourceIdentity,
) -> anyhow::Result<Value> {
    let status = response.status_code();
    if !status.is_success() && identity.context {
        anyhow::bail!("{operation} failed for {} ({status})", identity.value);
    }
    let value = response
        .json::<Value>()
        .await
        .map_err(|error| identity.failure(operation, error))?;
    if !status.is_success() {
        let reason = value
            .pointer("/error/reason")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/error/type").and_then(Value::as_str))
            .unwrap_or("Elasticsearch request failed");
        anyhow::bail!("{operation} failed ({status}): {reason}");
    }
    Ok(value)
}

fn execute_opened_table(
    client: Arc<Elasticsearch>,
    identity: ElasticsearchSourceIdentity,
    target: Option<String>,
    native_query: Option<String>,
    field_catalog: Option<ElasticsearchFieldCatalog>,
    options: &OpenOptions,
) -> anyhow::Result<OpenedTable> {
    let limit = options.limit.unwrap_or_else(|| {
        NonZeroUsize::new(DEFAULT_ELASTICSEARCH_SOURCE_LIMIT).expect("non-zero default")
    });
    let base = match native_query.as_deref() {
        Some(query) => query.to_owned(),
        None => {
            let target = target
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("Elasticsearch target or query is required"))?;
            format!("FROM {} METADATA _index, _id", render_from_target(target)?)
        }
    };
    let generation = SourceGeneration::new();
    let mut request = SourceQuery::new(generation, limit);
    request.native_query = Some(base);
    let source_fields: Arc<[SourceFieldMetadata]> = field_catalog
        .as_ref()
        .map(source_field_metadata)
        .unwrap_or_default()
        .into();
    let result = execute_esql(
        client.clone(),
        identity.clone(),
        target.clone(),
        request,
        &options.source_filters,
        &options.source_sort,
    )?;
    Ok(OpenedTable {
        generation: result.definition.generation,
        definition: result.definition.clone(),
        store: Box::new(ElasticsearchTableStore {
            inner: result.table,
            definition: result.definition,
            client,
            identity,
            target,
            field_catalog: source_fields,
            active_query: result.query,
            extent: result.extent,
            is_partial: result.is_partial,
            warnings: result.warnings,
            provenance: result.provenance,
            identities: result.identities,
        }),
        object_mode: None,
        warnings: Vec::new(),
    })
}

struct ExecutedEsql {
    definition: TableDefinition,
    table: InMemoryTable,
    query: SourceQuery,
    extent: ResultExtent,
    is_partial: bool,
    warnings: Vec<String>,
    provenance: NativeQueryArtifact,
    identities: Vec<Option<StableRowIdentity>>,
}

fn execute_esql(
    client: Arc<Elasticsearch>,
    identity: ElasticsearchSourceIdentity,
    target: Option<String>,
    request: SourceQuery,
    filters: &[SourceFilterRequest],
    sorts: &[SourceSortRequest],
) -> anyhow::Result<ExecutedEsql> {
    run_async(execute_esql_async(
        client,
        identity,
        target,
        request,
        filters.to_vec(),
        sorts.to_vec(),
    ))
}

async fn execute_esql_async(
    client: Arc<Elasticsearch>,
    identity: ElasticsearchSourceIdentity,
    target: Option<String>,
    mut request: SourceQuery,
    filters: Vec<SourceFilterRequest>,
    sorts: Vec<SourceSortRequest>,
) -> anyhow::Result<ExecutedEsql> {
    let base = request
        .native_query
        .clone()
        .ok_or_else(|| anyhow::anyhow!("ES|QL base query is missing"))?;
    let compiled = compile_esql(&base, &filters, &sorts, request.limit)?;
    let execution_query = compiled.execution.clone();
    let params = compiled.params.clone();
    let response = client
        .esql()
        .query()
        .allow_partial_results(true)
        .request_timeout(REQUEST_TIMEOUT)
        .body(esql_request_body(execution_query, params))
        .send()
        .await
        .map_err(|error| identity.failure("execute ES|QL", error))?;
    let response = response_json(response, "execute ES|QL", &identity).await?;
    let parsed: EsqlResponse = serde_json::from_value(response).map_err(|error| {
        if identity.context {
            identity.failure("decode ES|QL response", error)
        } else {
            anyhow::anyhow!("invalid ES|QL response: {error}")
        }
    })?;
    let warnings = if identity.context {
        let count = parsed.warnings.len();
        drop(parsed.warnings);
        if count == 0 {
            Vec::new()
        } else {
            vec![format!(
                "Elasticsearch reported {count} warning(s); context warning details are redacted"
            )]
        }
    } else {
        parsed.warnings
    };
    let generation = SourceGeneration::new();
    request.generation = generation;
    let relation_name = target.unwrap_or_else(|| "__tview_esql_query".to_owned());
    let definition = definition_from_esql_columns(generation, &relation_name, &parsed.columns);
    request.filters = resolve_esql_filters(&definition, &filters)?;
    request.order_by = resolve_esql_sort(&definition, &sorts)?;
    validate_source_query(&definition, &request)?;

    let limit = request.limit.get();
    let had_extra = parsed.values.len() > limit;
    let mut rows = Vec::with_capacity(parsed.values.len().min(limit));
    for (ordinal, values) in parsed.values.into_iter().take(limit).enumerate() {
        if values.len() != definition.columns.len() {
            anyhow::bail!(
                "ES|QL row {ordinal} has {} values for {} columns",
                values.len(),
                definition.columns.len()
            );
        }
        rows.push(Row::new(
            crate::table::RowId {
                generation,
                ordinal: ordinal as u64,
            },
            values.into_iter().map(esql_cell).collect(),
        ));
    }
    let identities = document_identities(&parsed.columns, &rows);
    let extent = if had_extra {
        ResultExtent::Truncated {
            source_rows: rows.len(),
            limit,
        }
    } else {
        ResultExtent::Complete {
            source_rows: rows.len(),
        }
    };
    let table = InMemoryTable::from_rows(generation, rows)?;
    Ok(ExecutedEsql {
        definition,
        table,
        query: request,
        extent,
        is_partial: parsed.is_partial,
        warnings,
        provenance: compiled.provenance,
        identities,
    })
}

fn esql_request_body(query: String, params: BTreeMap<String, Value>) -> Value {
    let mut body = json!({ "query": query });
    if !params.is_empty() {
        body["params"] = Value::Array(
            params
                .into_iter()
                .map(|(name, value)| json!({ name: value }))
                .collect(),
        );
    }
    body
}

#[derive(Debug, Deserialize)]
struct EsqlResponse {
    columns: Vec<EsqlColumn>,
    #[serde(default)]
    values: Vec<Vec<Value>>,
    #[serde(default)]
    is_partial: bool,
    #[serde(default)]
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct EsqlColumn {
    name: String,
    #[serde(rename = "type")]
    field_type: String,
}

fn definition_from_esql_columns(
    generation: SourceGeneration,
    relation: &str,
    columns: &[EsqlColumn],
) -> TableDefinition {
    TableDefinition {
        generation,
        columns: columns
            .iter()
            .enumerate()
            .map(|(ordinal, column)| ColumnDefinition {
                id: ColumnId {
                    generation,
                    ordinal: ordinal as u32,
                },
                source_identity: ColumnSourceIdentity::RelationColumn {
                    relation: relation.to_owned(),
                    ordinal,
                    name: column.name.clone(),
                },
                display_name: column.name.clone(),
                source_declared_type: Some(column.field_type.clone()),
                source_type: esql_logical_type(&column.field_type),
                type_origin: TypeOrigin::Declared,
            })
            .collect(),
        schema_state: SchemaState::Complete,
        relation: crate::table::RelationMetadata {
            name: relation.to_owned(),
            display_name: relation.to_owned(),
            header_visible: true,
        },
    }
}

fn esql_logical_type(field_type: &str) -> LogicalType {
    match field_type {
        "boolean" => LogicalType::Boolean,
        "byte" | "short" | "integer" | "long" | "counter_integer" | "counter_long" => {
            LogicalType::Integer
        }
        "double" | "float" | "half_float" | "scaled_float" => LogicalType::Float,
        "binary" => LogicalType::Binary,
        "null" => LogicalType::Null,
        "object" | "nested" => LogicalType::Structured,
        "keyword" | "text" | "date" | "date_nanos" | "ip" | "version" | "geo_point"
        | "cartesian_point" | "unsigned_long" | "unsupported" => LogicalType::Text,
        _ => LogicalType::Unknown,
    }
}

fn esql_cell(value: Value) -> CellValue {
    match value {
        Value::Null => CellValue::Null,
        Value::Bool(value) => CellValue::Boolean(value),
        Value::Number(value) => {
            if let Some(integer) = value.as_i64() {
                CellValue::Integer(integer)
            } else if let Some(unsigned) = value.as_u64() {
                i64::try_from(unsigned)
                    .map(CellValue::Integer)
                    .unwrap_or_else(|_| CellValue::Json(value.to_string()))
            } else {
                value
                    .as_f64()
                    .filter(|value| value.is_finite())
                    .map(CellValue::Float)
                    .unwrap_or_else(|| CellValue::Json(value.to_string()))
            }
        }
        Value::String(value) => CellValue::Text(value),
        Value::Array(_) | Value::Object(_) => CellValue::Json(value.to_string()),
    }
}

fn document_identities(columns: &[EsqlColumn], rows: &[Row]) -> Vec<Option<StableRowIdentity>> {
    let index_column = columns.iter().position(|column| column.name == "_index");
    let id_column = columns.iter().position(|column| column.name == "_id");
    let (Some(index_column), Some(id_column)) = (index_column, id_column) else {
        return vec![None; rows.len()];
    };
    let mut seen = BTreeSet::new();
    let mut identities = Vec::with_capacity(rows.len());
    for row in rows {
        let index = row.cells.get(index_column).and_then(identity_text);
        let id = row.cells.get(id_column).and_then(identity_text);
        let Some((index, id)) = index.zip(id) else {
            identities.push(None);
            continue;
        };
        if !seen.insert((index.clone(), id.clone())) {
            return vec![None; rows.len()];
        }
        identities.push(Some(StableRowIdentity::ElasticsearchDocument { index, id }));
    }
    identities
}

fn identity_text(value: &CellValue) -> Option<String> {
    match value {
        CellValue::Text(value) if !value.is_empty() => Some(value.clone()),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct CompiledEsql {
    execution: String,
    params: BTreeMap<String, Value>,
    provenance: NativeQueryArtifact,
}

fn compile_esql(
    base: &str,
    filters: &[SourceFilterRequest],
    sorts: &[SourceSortRequest],
    limit: NonZeroUsize,
) -> anyhow::Result<CompiledEsql> {
    if base.trim().is_empty() {
        anyhow::bail!("ES|QL query cannot be empty");
    }
    let mut logical = base.trim().to_owned();
    let mut params = BTreeMap::new();
    let mut artifact_params = Vec::new();
    if !filters.is_empty() {
        logical.push_str("\n| WHERE ");
        for (index, filter) in filters.iter().enumerate() {
            if index > 0 {
                logical.push_str(" AND ");
            }
            let identifier = render_esql_identifier(&filter.column)?;
            artifact_params.push(NativeQueryParameter::Identifier(filter.column.clone()));
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
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("source filter operand is missing"))?;
                    let parameter = format!("v{}", params.len() + 1);
                    params.insert(parameter.clone(), esql_parameter_value(operand));
                    artifact_params.push(NativeQueryParameter::Value(operand.clone()));
                    let expression = match operator {
                        SourceFilterOperator::Equal => format!("{identifier} == ?{parameter}"),
                        SourceFilterOperator::NotEqual => format!("{identifier} != ?{parameter}"),
                        SourceFilterOperator::LessThan => format!("{identifier} < ?{parameter}"),
                        SourceFilterOperator::LessThanOrEqual => {
                            format!("{identifier} <= ?{parameter}")
                        }
                        SourceFilterOperator::GreaterThan => {
                            format!("{identifier} > ?{parameter}")
                        }
                        SourceFilterOperator::GreaterThanOrEqual => {
                            format!("{identifier} >= ?{parameter}")
                        }
                        SourceFilterOperator::Contains => {
                            format!("{identifier} LIKE CONCAT(\"*\", ?{parameter}, \"*\")")
                        }
                        SourceFilterOperator::Prefix => {
                            format!("{identifier} LIKE CONCAT(?{parameter}, \"*\")")
                        }
                        SourceFilterOperator::IsNull | SourceFilterOperator::IsNotNull => {
                            unreachable!("handled above")
                        }
                    };
                    logical.push_str(&expression);
                }
            }
        }
    }
    if !sorts.is_empty() {
        logical.push_str("\n| SORT ");
        for (index, sort) in sorts.iter().enumerate() {
            if index > 0 {
                logical.push_str(", ");
            }
            let identifier = render_esql_identifier(&sort.column)?;
            artifact_params.push(NativeQueryParameter::Identifier(sort.column.clone()));
            logical.push_str(&identifier);
            logical.push(' ');
            logical.push_str(match sort.direction {
                SortDirection::Ascending => "ASC",
                SortDirection::Descending => "DESC",
            });
        }
    }
    logical.push_str(&format!("\n| LIMIT {}", limit.get()));
    let execution = logical
        .strip_suffix(&format!("LIMIT {}", limit.get()))
        .expect("logical query ends in limit")
        .to_owned()
        + &format!("LIMIT {}", limit.get().saturating_add(1));
    Ok(CompiledEsql {
        execution,
        params,
        provenance: NativeQueryArtifact {
            language: NativeQueryLanguage::Esql,
            base: Some(base.to_owned()),
            logical: logical.clone(),
            parameters: artifact_params,
            copyable: logical,
        },
    })
}

fn esql_parameter_value(value: &SourceOperand) -> Value {
    match value {
        SourceOperand::Null => Value::Null,
        SourceOperand::Boolean(value) => Value::Bool(*value),
        SourceOperand::Integer(value) => json!(value),
        SourceOperand::Float(value) => json!(value),
        SourceOperand::Text(value) => Value::String(value.clone()),
        SourceOperand::Binary(value) => Value::String(hex_bytes(value)),
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn validate_from_target(target: &str) -> anyhow::Result<()> {
    if target.is_empty()
        || target.chars().any(|ch| {
            !(ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.' | '*' | '?' | ',' | ':' | '@'))
        })
    {
        anyhow::bail!("Elasticsearch target contains unsupported ES|QL FROM characters");
    }
    Ok(())
}

fn render_from_target(target: &str) -> anyhow::Result<String> {
    validate_from_target(target)?;
    Ok(target.to_owned())
}

fn render_esql_identifier(identifier: &str) -> anyhow::Result<String> {
    if identifier.is_empty() || identifier.contains(['\n', '\r', '\0']) {
        anyhow::bail!("invalid ES|QL identifier");
    }
    Ok(format!("`{}`", identifier.replace('`', "``")))
}

fn resolve_esql_filters(
    definition: &TableDefinition,
    requests: &[SourceFilterRequest],
) -> anyhow::Result<Vec<crate::table::SourceFilter>> {
    requests
        .iter()
        .map(|request| {
            Ok(crate::table::SourceFilter {
                scope: SourceFilterScope::Column(resolve_column(definition, &request.column)?),
                operator: request.operator,
                operand: request.operand.clone(),
            })
        })
        .collect()
}

fn resolve_esql_sort(
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
    let matches = definition
        .columns
        .iter()
        .filter(|column| column.display_name == name)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [column] => Ok(column.id),
        [] => anyhow::bail!("source operation references unknown ES|QL column '{name}'"),
        _ => anyhow::bail!("source operation ES|QL column '{name}' is ambiguous"),
    }
}

struct ElasticsearchTableStore {
    inner: InMemoryTable,
    definition: TableDefinition,
    client: Arc<Elasticsearch>,
    identity: ElasticsearchSourceIdentity,
    target: Option<String>,
    field_catalog: Arc<[SourceFieldMetadata]>,
    active_query: SourceQuery,
    extent: ResultExtent,
    is_partial: bool,
    warnings: Vec<String>,
    provenance: NativeQueryArtifact,
    identities: Vec<Option<StableRowIdentity>>,
}

impl TableStore for ElasticsearchTableStore {
    fn generation(&self) -> SourceGeneration {
        self.inner.generation()
    }

    fn row_count(&self) -> RowCount {
        self.inner.row_count()
    }

    fn column_count(&self) -> usize {
        self.inner.column_count()
    }

    fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
        self.inner.row(index)
    }

    fn ensure_indexed_through(&mut self, index: RowIndex) -> anyhow::Result<IndexProgress> {
        self.inner.ensure_indexed_through(index)
    }

    fn scan_rows(
        &mut self,
        request: ScanRequest,
        visitor: &mut dyn RowVisitor,
    ) -> anyhow::Result<ScanProgress> {
        self.inner.scan_rows(request, visitor)
    }

    fn materialize(&mut self) -> anyhow::Result<InMemoryTable> {
        self.inner.materialize()
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
        let task = self.source_query_task(query.clone())?;
        let result = match task {
            crate::table::SourceQueryTask::Blocking(task) => task()?,
            crate::table::SourceQueryTask::Async(task) => run_async(task)?,
        };
        Ok(SourceQueryExecution::SourceExecuted(result))
    }

    fn source_query_task(
        &mut self,
        query: SourceQuery,
    ) -> anyhow::Result<crate::table::SourceQueryTask> {
        let client = self.client.clone();
        let identity = self.identity.clone();
        let target = self.target.clone();
        let field_catalog = self.field_catalog.clone();
        let base = query
            .native_query
            .clone()
            .ok_or_else(|| anyhow::anyhow!("ES|QL base query is missing"))?;
        let filters = query
            .filters
            .iter()
            .map(|filter| {
                let SourceFilterScope::Column(column) = filter.scope else {
                    anyhow::bail!("ES|QL filters require a result column");
                };
                let name = self
                    .definition
                    .columns
                    .get(column.ordinal as usize)
                    .ok_or_else(|| anyhow::anyhow!("ES|QL filter column is unavailable"))?
                    .display_name
                    .clone();
                Ok(SourceFilterRequest {
                    column: name,
                    operator: filter.operator,
                    operand: filter.operand.clone(),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let sorts = query
            .order_by
            .iter()
            .map(|sort| {
                let name = self
                    .definition
                    .columns
                    .get(sort.column.ordinal as usize)
                    .ok_or_else(|| anyhow::anyhow!("ES|QL sort column is unavailable"))?
                    .display_name
                    .clone();
                Ok(SourceSortRequest {
                    column: name,
                    direction: sort.direction,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(crate::table::SourceQueryTask::Async(Box::pin(async move {
            let replacement_client = client.clone();
            let replacement_identity = identity.clone();
            let result = execute_esql_async(
                client,
                identity,
                target.clone(),
                SourceQuery {
                    native_query: Some(base),
                    ..query
                },
                filters,
                sorts,
            )
            .await?;
            let definition = result.definition;
            let store = Box::new(ElasticsearchTableStore {
                inner: result.table,
                definition: definition.clone(),
                client: replacement_client,
                identity: replacement_identity,
                target,
                field_catalog,
                active_query: result.query,
                extent: result.extent,
                is_partial: result.is_partial,
                warnings: result.warnings,
                provenance: result.provenance,
                identities: result.identities,
            }) as Box<dyn TableStore>;
            Ok(crate::table::SourceResult::from_store(definition, store))
        })))
    }

    fn result_extent(&self) -> Option<ResultExtent> {
        Some(self.extent)
    }

    fn query_provenance(&self) -> Option<&NativeQueryArtifact> {
        Some(&self.provenance)
    }

    fn result_is_partial(&self) -> bool {
        self.is_partial
    }

    fn result_warnings(&self) -> &[String] {
        &self.warnings
    }

    fn source_field_catalog(&self) -> Arc<[SourceFieldMetadata]> {
        self.field_catalog.clone()
    }

    fn stable_row_identity(
        &mut self,
        index: RowIndex,
    ) -> anyhow::Result<Option<StableRowIdentity>> {
        Ok(self.identities.get(index.0).cloned().flatten())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    fn mock_http_server(
        response: Option<&'static [u8]>,
        delay: Duration,
    ) -> (
        url::Url,
        mpsc::Receiver<String>,
        std::thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = url::Url::parse(&format!("http://{}", listener.local_addr().unwrap()))
            .expect("mock endpoint");
        let (request_sender, request_receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("mock request");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = stream.read(&mut chunk).expect("read mock request");
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..count]);
            }
            request_sender
                .send(String::from_utf8_lossy(&request).into_owned())
                .unwrap();
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
            if let Some(response) = response {
                stream.write_all(response).expect("write mock response");
            }
        });
        (endpoint, request_receiver, worker)
    }

    fn captured_info_request(settings: ElasticsearchTransportSettings) -> String {
        captured_client_request(|endpoint| {
            build_client_with_settings(endpoint, settings, Duration::from_secs(2)).unwrap()
        })
    }

    fn captured_client_request(build: impl FnOnce(&url::Url) -> Elasticsearch) -> String {
        let (endpoint, request, worker) = mock_http_server(
            Some(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}"),
            Duration::ZERO,
        );
        let client = build(&endpoint);
        run_async(async move {
            client.info().send().await?;
            Ok(())
        })
        .unwrap();
        let request = request.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
        request
    }

    #[test]
    fn context_transport_supports_each_context_authentication_mode() {
        let api_key = captured_client_request(|endpoint| {
            build_context_client(endpoint, elasticrc::Auth::api_key("context-key")).unwrap()
        });
        assert!(api_key.contains("authorization: ApiKey context-key"));

        let basic = captured_client_request(|endpoint| {
            build_context_client(endpoint, elasticrc::Auth::basic("elastic", "secret")).unwrap()
        });
        assert!(basic.contains("authorization: Basic ZWxhc3RpYzpzZWNyZXQ="));

        let unauthenticated = captured_client_request(|endpoint| {
            build_context_client(endpoint, elasticrc::Auth::None).unwrap()
        });
        assert!(!unauthenticated
            .to_ascii_lowercase()
            .contains("authorization:"));
    }

    #[test]
    fn transport_authentication_debug_never_prints_credentials() {
        let sentinel = "transport-secret-sentinel";
        let api_key = ElasticsearchTransportSettings {
            auth: Some(ElasticsearchAuth::ApiKey(sentinel.to_owned())),
            ca_cert: None,
        };
        let basic = ElasticsearchTransportSettings {
            auth: Some(ElasticsearchAuth::Basic {
                username: sentinel.to_owned(),
                password: sentinel.to_owned(),
            }),
            ca_cert: None,
        };
        assert!(!format!("{api_key:?}").contains(sentinel));
        assert!(!format!("{basic:?}").contains(sentinel));
    }

    #[test]
    fn context_request_failures_hide_server_output_and_endpoint_secrets() {
        let (endpoint, request, worker) = mock_http_server(
            Some(b"HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"error\":{\"reason\":\"transport-secret-sentinel\"}}"),
            Duration::ZERO,
        );
        let client = build_context_client(&endpoint, elasticrc::Auth::None).unwrap();
        let identity = ElasticsearchSourceIdentity {
            value: ".production.elasticsearch://".into(),
            context: true,
        };
        let error = run_async(async move {
            let response = client.info().send().await?;
            response_json(response, "read Elasticsearch info", &identity).await
        })
        .unwrap_err();
        let rendered = format!("{error:#}");
        assert!(rendered.contains(".production.elasticsearch://"));
        assert!(!rendered.contains("transport-secret-sentinel"));
        assert!(error.source().is_none());
        request.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();

        let identity = ElasticsearchSourceIdentity {
            value: ".production.elasticsearch://".into(),
            context: true,
        };
        let error = identity.failure(
            "execute ES|QL",
            anyhow::anyhow!("https://elastic.example/?key=transport-secret-sentinel"),
        );
        assert!(!format!("{error:#}").contains("transport-secret-sentinel"));
        assert!(error.source().is_none());
    }

    #[test]
    fn transport_settings_validate_environment_credential_modes() {
        assert_eq!(
            transport_settings(
                Some("encoded-key".to_owned()),
                None,
                None,
                Some("/tmp/ca.pem".into()),
            )
            .unwrap(),
            ElasticsearchTransportSettings {
                auth: Some(ElasticsearchAuth::ApiKey("encoded-key".to_owned())),
                ca_cert: Some("/tmp/ca.pem".into()),
            }
        );
        assert!(transport_settings(
            Some("key".to_owned()),
            Some("user".to_owned()),
            Some("password".to_owned()),
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("conflicts"));
        assert!(
            transport_settings(None, Some("user".to_owned()), None, None)
                .unwrap_err()
                .to_string()
                .contains("must be set together")
        );
    }

    #[test]
    fn official_transport_sends_api_key_and_basic_auth_headers() {
        let api_key = captured_info_request(ElasticsearchTransportSettings {
            auth: Some(ElasticsearchAuth::ApiKey("encoded-key".to_owned())),
            ca_cert: None,
        });
        assert!(api_key.contains("authorization: ApiKey encoded-key"));

        let basic = captured_info_request(ElasticsearchTransportSettings {
            auth: Some(ElasticsearchAuth::Basic {
                username: "elastic".to_owned(),
                password: "secret".to_owned(),
            }),
            ca_cert: None,
        });
        assert!(basic.contains("authorization: Basic ZWxhc3RpYzpzZWNyZXQ="));
    }

    #[test]
    fn transport_rejects_invalid_custom_ca_and_redacts_endpoint_credentials() {
        let ca = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(ca.path(), "not a PEM certificate").unwrap();
        let endpoint = url::Url::parse("https://elastic.example:9200").unwrap();
        let error = build_client_with_settings(
            &endpoint,
            ElasticsearchTransportSettings {
                auth: None,
                ca_cert: Some(ca.path().to_owned()),
            },
            Duration::from_secs(1),
        )
        .expect_err("invalid CA");
        assert!(error.to_string().contains("invalid ELASTIC_CA_CERT"));

        let endpoint =
            url::Url::parse("https://elastic:super-secret@elastic.example:9200").unwrap();
        let error = build_client_with_settings(
            &endpoint,
            ElasticsearchTransportSettings::default(),
            Duration::from_secs(1),
        )
        .expect_err("userinfo rejected");
        assert!(!error.to_string().contains("super-secret"));
        assert_eq!(
            InputSource::Url(endpoint).safe_identity(),
            "https://elastic.example:9200/"
        );
    }

    #[test]
    fn official_transport_enforces_request_timeout() {
        let (endpoint, request, worker) = mock_http_server(None, Duration::from_millis(150));
        let client = build_client_with_settings(
            &endpoint,
            ElasticsearchTransportSettings::default(),
            Duration::from_millis(20),
        )
        .unwrap();
        let started = std::time::Instant::now();
        let error = run_async(async move {
            client.info().send().await?;
            Ok(())
        })
        .unwrap_err();
        assert!(!error.to_string().is_empty());
        assert!(started.elapsed() < Duration::from_millis(120));
        request.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn discovery_filters_hidden_closed_dot_resources_aliases_and_backing_indices() {
        let value = json!({
            "indices": [
                {"name": "logs-a", "attributes": ["open"]},
                {"name": ".hidden", "attributes": ["open"]},
                {"name": "closed-a", "attributes": ["closed"]},
                {"name": "hidden-a", "attributes": ["open", "hidden"]},
                {"name": ".ds-logs-prod-000001", "attributes": ["open"]}
            ],
            "aliases": [{"name": "logs-current", "indices": ["logs-a"]}],
            "data_streams": [
                {"name": "logs-prod", "backing_indices": [".ds-logs-prod-000001"]},
                {"name": ".internal-stream", "hidden": false},
                {"name": "hidden-stream", "hidden": true}
            ]
        });
        assert_eq!(
            parse_resolved_targets(&value).unwrap(),
            vec![
                ElasticsearchTarget {
                    name: "logs-a".to_owned(),
                    kind: ElasticsearchTargetKind::Index,
                },
                ElasticsearchTarget {
                    name: "logs-prod".to_owned(),
                    kind: ElasticsearchTargetKind::DataStream,
                },
            ]
        );
    }

    #[test]
    fn field_catalog_flattens_objects_nested_multifields_runtime_and_conflicts() {
        let mappings = json!({
            "logs-a": {"mappings": {
                "properties": {
                    "message": {"type": "text", "fields": {"keyword": {"type": "keyword"}}},
                    "user": {"properties": {"name": {"type": "keyword"}}},
                    "events": {"type": "nested", "properties": {"kind": {"type": "keyword"}}}
                },
                "runtime": {"day": {"type": "keyword"}}
            }},
            "logs-b": {"mappings": {"properties": {"message": {"type": "long"}}}}
        });
        let caps = json!({"fields": {
            "message": {
                "text": {"searchable": true, "aggregatable": false},
                "long": {"searchable": true, "aggregatable": true}
            },
            "user.name": {"keyword": {"searchable": true, "aggregatable": true}}
        }});
        let catalog = parse_field_catalog(&mappings, &caps);
        assert!(catalog.fields["message"].conflict);
        assert!(catalog.fields["message.keyword"].multifield);
        assert!(catalog.fields["events"].nested);
        assert!(catalog.fields["day"].runtime);
    }

    #[test]
    fn esql_compiler_preserves_base_and_applies_operations_then_hard_limit() {
        let compiled = compile_esql(
            "FROM logs-* | EVAL level = TO_UPPER(log.level)",
            &[SourceFilterRequest {
                column: "level".to_owned(),
                operator: SourceFilterOperator::Equal,
                operand: Some(SourceOperand::Text("ERROR".to_owned())),
            }],
            &[SourceSortRequest {
                column: "@timestamp".to_owned(),
                direction: SortDirection::Descending,
            }],
            NonZeroUsize::new(100).unwrap(),
        )
        .unwrap();
        assert!(compiled
            .provenance
            .logical
            .contains("| WHERE `level` == ?v1"));
        assert!(compiled
            .provenance
            .logical
            .contains("| SORT `@timestamp` DESC"));
        assert!(compiled.provenance.logical.ends_with("| LIMIT 100"));
        assert!(compiled.execution.ends_with("| LIMIT 101"));
        assert_eq!(compiled.params["v1"], json!("ERROR"));
    }

    #[test]
    fn esql_request_body_omits_empty_params_and_uses_the_api_parameter_array() {
        assert_eq!(
            esql_request_body("FROM logs-a".to_owned(), BTreeMap::new()),
            json!({"query": "FROM logs-a"})
        );

        let params = BTreeMap::from([
            ("v2".to_owned(), json!(42)),
            ("v1".to_owned(), json!("error")),
        ]);
        assert_eq!(
            esql_request_body("FROM logs-a | WHERE latency == ?v2".to_owned(), params),
            json!({
                "query": "FROM logs-a | WHERE latency == ?v2",
                "params": [{"v1": "error"}, {"v2": 42}]
            })
        );
    }

    #[test]
    fn esql_values_preserve_scalars_structures_and_oversized_numbers() {
        assert_eq!(esql_cell(Value::Null), CellValue::Null);
        assert_eq!(esql_cell(json!(true)), CellValue::Boolean(true));
        assert_eq!(esql_cell(json!(42)), CellValue::Integer(42));
        assert_eq!(esql_cell(json!(4.5)), CellValue::Float(4.5));
        assert_eq!(
            esql_cell(json!(["a", "b"])),
            CellValue::Json("[\"a\",\"b\"]".to_owned())
        );
        assert_eq!(
            esql_cell(json!({"x": 1})),
            CellValue::Json("{\"x\":1}".to_owned())
        );
        let oversized: Value = serde_json::from_str("18446744073709551615").unwrap();
        assert_eq!(
            esql_cell(oversized),
            CellValue::Json("18446744073709551615".to_owned())
        );
    }

    #[test]
    fn explicit_alias_and_wildcard_targets_pass_safe_rendering_without_discovery() {
        assert_eq!(render_from_target("logs-current").unwrap(), "logs-current");
        assert_eq!(render_from_target("logs-*").unwrap(), "logs-*");
        assert!(render_from_target("logs-* | DROP *").is_err());
    }
}
