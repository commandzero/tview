# elasticsearch-data-source Specification

## Purpose
Define read-only Elasticsearch discovery, bounded ES|QL queries, authentication,
and source-result handling through the optional Elasticsearch adapter.

## Requirements

### Requirement: Elasticsearch compile feature
The system SHALL place Elasticsearch support, the official Elasticsearch Rust client, and Elastic CLI context resolution behind the optional `elasticsearch` Cargo feature.

#### Scenario: Elasticsearch feature enabled
- **WHEN** Tview is compiled with the `elasticsearch` feature
- **THEN** Elasticsearch format parsing, HTTP(S) and dot-context dispatch, target discovery, mappings, and ES|QL execution are available

#### Scenario: Elasticsearch feature disabled
- **WHEN** Tview is compiled without the `elasticsearch` feature
- **THEN** Elasticsearch client and context-resolution dependencies and Elasticsearch-specific format values, dispatch, discovery, and tests are omitted
- **AND** a dot-context source reports that Elasticsearch support is unavailable without reading configuration, executing resolvers, or attempting local file input

### Requirement: Elasticsearch endpoint resolution
The Elasticsearch adapter SHALL accept an HTTP(S) positional source target only after Elasticsearch is selected explicitly or by saved source configuration. It SHALL also accept the dot-context sources defined by `elasticrc-contexts`, which imply Elasticsearch format and resolve a configured HTTP(S) endpoint. It SHALL connect through the official client transport without treating these remote sources as local paths.

#### Scenario: Explicit Elasticsearch endpoint
- **WHEN** a user opens `https://elastic.example:9200` with `--format elasticsearch`
- **THEN** Tview constructs an Elasticsearch client for that endpoint

#### Scenario: Ambiguous HTTP URL
- **WHEN** an HTTP(S) target has no explicit or saved format
- **THEN** Tview does not assume that the endpoint is Elasticsearch and reports that the remote target requires an explicit format

#### Scenario: Unsupported target kind
- **WHEN** Elasticsearch format is selected for stdin or a local filesystem path
- **THEN** source opening fails with a clear endpoint-target diagnostic

#### Scenario: Dot-context endpoint
- **WHEN** `.production.es://logs-*` resolves an HTTP(S) Elasticsearch endpoint
- **THEN** Tview uses that endpoint with the existing Elasticsearch adapter and selects `logs-*` without requiring `--format elasticsearch`

### Requirement: Elasticsearch authentication and secret handling
The Elasticsearch adapter SHALL configure authenticated TLS transport from the documented environment variables for direct endpoints or from the selected Elastic CLI service for dot-context sources. It SHALL keep credentials and secret-bearing headers out of saved views, query provenance, status messages, and diagnostics. It SHALL NOT add source-specific credential CLI arguments or a separate Tview connection-profile store.

#### Scenario: Authenticated request
- **WHEN** the configured Elasticsearch transport includes supported credentials
- **THEN** discovery, mapping, and ES|QL requests use those credentials

#### Scenario: Environment-only configuration
- **WHEN** a user configures a direct HTTP(S) endpoint's authentication or a custom CA
- **THEN** Tview reads the documented environment variables without loading Elastic CLI configuration

#### Scenario: Context authentication
- **WHEN** a dot-context target selects an Elasticsearch service
- **THEN** Tview uses only that service's resolved authentication and retains the existing custom-CA environment setting

#### Scenario: Saved Elasticsearch view
- **WHEN** an authenticated Elasticsearch source is serialized as a saved view
- **THEN** non-secret source and query configuration may be persisted but credentials and authorization headers are omitted

#### Scenario: Request failure diagnostic
- **WHEN** an authenticated request fails
- **THEN** the error identifies the failed operation and safe source identity without exposing credential material

### Requirement: Elasticsearch target discovery
When Elasticsearch has neither a native query nor a selected table, the interactive application SHALL discover candidates through the structured resolve-index API with open wildcard expansion, expose non-hidden open indices and data streams whose names do not begin with `.`, omit aliases from the picker, and present the remaining resources in a typed target picker.

#### Scenario: Visible open index
- **WHEN** resolve-index returns an open non-hidden index named `application-events`
- **THEN** the picker includes `application-events` in its Indices section

#### Scenario: Visible data stream
- **WHEN** resolve-index returns a non-hidden data stream named `logs-nginx.access-prod`
- **THEN** the picker includes it in its Data streams section

#### Scenario: Dot-prefixed resource
- **WHEN** discovery returns an index or data stream whose name begins with `.`
- **THEN** Tview excludes it from the picker even if the endpoint returns it

#### Scenario: Hidden or closed index
- **WHEN** an index is hidden or is not open
- **THEN** Tview does not offer it as a selectable target

#### Scenario: Alias is returned
- **WHEN** resolve-index returns an alias
- **THEN** Tview does not add that alias to the picker

#### Scenario: Discovery has no candidates
- **WHEN** discovery returns no selectable indices or data streams
- **THEN** Tview reports that no visible ES|QL targets are available

### Requirement: Elasticsearch target and query selection
The adapter SHALL execute a complete user-supplied ES|QL query when `source.query` is present; otherwise it SHALL generate a bounded `FROM` query from a selected index or data stream.

#### Scenario: Complete ES|QL query
- **WHEN** `source.query` contains `FROM logs-* | WHERE log.level == "error"`
- **THEN** that ES|QL pipeline is the base native query and no target picker is displayed

#### Scenario: Selected index
- **WHEN** `source.table` selects `application-events` and no native query is supplied
- **THEN** Tview generates a bounded ES|QL query whose `FROM` source is that index

#### Scenario: Selected data stream
- **WHEN** the picker selects `logs-nginx.access-prod`
- **THEN** Tview generates a bounded ES|QL query whose `FROM` source is that data stream

#### Scenario: Explicit target pass-through
- **WHEN** `source.table` contains a target that was not offered by the picker
- **THEN** Tview safely uses that value as the generated ES|QL `FROM` target without pre-validating its existence, visibility, or resource kind

#### Scenario: Explicit alias
- **WHEN** `source.table` names an Elasticsearch alias accepted by mappings, field capabilities, and ES|QL
- **THEN** the alias works as the generated query target even though aliases are absent from the picker

#### Scenario: Explicit target rejected by Elasticsearch
- **WHEN** mappings, field capabilities, or ES|QL rejects an explicit target
- **THEN** Tview reports the Elasticsearch error through normal source-opening failure handling

### Requirement: Mapping-aware Elasticsearch schema
For an explicitly selected index or data stream, Tview SHALL read mappings and field capabilities to construct a complete field catalog with mapped type, multivalue-compatible metadata, and cross-index conflicts, while treating the ES|QL response `columns` array as authoritative for the active rendered result.

#### Scenario: Selected index mapping
- **WHEN** an index is selected before query execution
- **THEN** mapped leaf fields and their source types are available to source configuration

#### Scenario: Selected data stream mapping
- **WHEN** a data stream is selected
- **THEN** Tview obtains its effective field catalog from its backing-index mappings and field capabilities

#### Scenario: Conflicting field mappings
- **WHEN** a target pattern maps the same field incompatibly across concrete indices
- **THEN** the field catalog preserves the conflict and does not silently choose one mapped type

#### Scenario: ES|QL transforms columns
- **WHEN** ES|QL uses `KEEP`, `DROP`, `RENAME`, `EVAL`, `STATS`, or another schema-shaping command
- **THEN** the active table columns and types come from the successful ES|QL response rather than the base mapping catalog

#### Scenario: Query-only source
- **WHEN** a complete ES|QL query is supplied without `source.table`
- **THEN** Tview can open the result from response column metadata without implementing an ES|QL parser to infer every `FROM` target

### Requirement: Bounded ES|QL execution
Every Elasticsearch source result SHALL have an explicit positive application limit applied after application-composed source operations, defaulting to 1,000 rows, and SHALL distinguish limit truncation from Elasticsearch partial execution.

#### Scenario: Default generated query
- **WHEN** a selected Elasticsearch target has no configured source limit
- **THEN** the generated ES|QL pipeline uses a 1,000-row application limit

#### Scenario: Configured limit
- **WHEN** `source.limit` is a positive supported value
- **THEN** the final composed ES|QL query uses that value as the active source-result boundary

#### Scenario: Truncation probe
- **WHEN** the endpoint permits an `N + 1` probe for a configured limit of `N`
- **THEN** Tview retains at most `N` rows and reports whether another result row existed

#### Scenario: Partial Elasticsearch result
- **WHEN** Elasticsearch returns `is_partial: true`
- **THEN** Tview reports the result as partial independently of whether the application limit was reached

#### Scenario: View filter reduces result
- **WHEN** a local view filter hides rows from a bounded ES|QL result
- **THEN** Tview does not issue another ES|QL request to refill the visible result

### Requirement: Typed ES|QL values
The adapter SHALL map ES|QL response values into typed cells without first converting the response to delimited text and SHALL preserve multivalued and otherwise structured JSON values losslessly.

#### Scenario: Scalar values
- **WHEN** a response row contains null, boolean, integral, floating-point, and string values
- **THEN** Tview preserves their corresponding typed value categories

#### Scenario: Multivalued field
- **WHEN** an ES|QL cell is a JSON array
- **THEN** Tview preserves the complete array as a structured cell rather than selecting one member or joining it ambiguously

#### Scenario: Source-specific type
- **WHEN** a response column has an ES|QL type such as `date`, `date_nanos`, `ip`, `version`, `geo_point`, or `unsupported`
- **THEN** Tview retains the raw ES|QL type for inspection while mapping display and local-operation behavior conservatively

### Requirement: Elasticsearch result identity
Document-producing ES|QL results SHALL use `_index` plus `_id` as stable row identity when both metadata fields are present and unique; results without usable document identity SHALL explicitly reset row-bound state across replacement.

#### Scenario: Document metadata present
- **WHEN** a generated query includes `_index` and `_id` metadata and each tuple is unique
- **THEN** cursor and mark reconciliation may use that tuple across compatible source-query replacements

#### Scenario: Aggregated result
- **WHEN** `STATS` or another transformation removes document metadata
- **THEN** stable row identity is unavailable and replacement resets cursor-following and marks

#### Scenario: Duplicate metadata tuple
- **WHEN** a result repeats the same purported `_index` and `_id` tuple
- **THEN** Tview does not silently treat it as stable identity

### Requirement: Asynchronous ES|QL replacement
Elasticsearch discovery and query work SHALL run without blocking the terminal event loop; native-query replacement SHALL retain the last successful result, activate only the latest successful revision, and cancel or discard superseded remote work.

#### Scenario: Slow ES|QL query
- **WHEN** an ES|QL request remains in progress
- **THEN** the TUI stays responsive and displays pending state while retaining the prior result

#### Scenario: Newer query supersedes request
- **WHEN** a newer source revision is requested before an older ES|QL request completes
- **THEN** the older request is cancelled when supported or its response is discarded

#### Scenario: Query failure
- **WHEN** ES|QL validation, transport, authentication, timeout, or execution fails
- **THEN** the prior successful result and view remain active and the failure is reported without exposing secrets

#### Scenario: Successful schema-changing replacement
- **WHEN** the latest ES|QL revision succeeds with a different result schema
- **THEN** its table definition, store, result metadata, provenance, and remapped compatible view state become active atomically

### Requirement: ES|QL query provenance
Every successful Elasticsearch result SHALL expose the complete logical ES|QL text, its bound value and identifier parameters, and a safe copyable representation independently of private truncation probes.

#### Scenario: Display active ES|QL
- **WHEN** the user opens native query information for an Elasticsearch result
- **THEN** the UI identifies the language as ES|QL and displays the active logical query and parameters

#### Scenario: Source operation changes
- **WHEN** a source filter, source sort, or source limit changes
- **THEN** the ES|QL artifact is regenerated from the complete composed source request

#### Scenario: Local view changes
- **WHEN** only local view operations or search change
- **THEN** ES|QL provenance remains unchanged and the UI identifies those operations as local
