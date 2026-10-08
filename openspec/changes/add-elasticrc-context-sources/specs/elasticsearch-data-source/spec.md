## MODIFIED Requirements

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
