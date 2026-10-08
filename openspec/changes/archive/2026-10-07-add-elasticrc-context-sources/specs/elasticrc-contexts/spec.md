## Purpose

Allow Tview to open Elasticsearch services configured in Elastic CLI contexts without copying endpoints or credentials into another connection store.

## ADDED Requirements

### Requirement: Dot-context Elasticsearch source syntax
Tview SHALL accept `.es://` and `.elasticsearch://` for the current Elastic CLI context and `.name.es://` and `.name.elasticsearch://` for a named context. The rightmost segment SHALL identify the service; preceding segments SHALL form the exact context name, including dots. These targets SHALL imply Elasticsearch format under automatic or saved format selection. An explicit incompatible CLI format SHALL fail before configuration resolution. Dot-context syntax SHALL NOT make arbitrary HTTP(S) URLs imply Elasticsearch format.

#### Scenario: Current-context aliases
- **WHEN** the user supplies `.es://` or `.elasticsearch://` without `--format`
- **THEN** Tview selects Elasticsearch and the current context's Elasticsearch service

#### Scenario: Named-context aliases
- **WHEN** the user supplies `.production.es://` or `.production.elasticsearch://`
- **THEN** both select the Elasticsearch service from the exact context named `production`

#### Scenario: Dotted context name
- **WHEN** the user supplies `.production.us-west.es://`
- **THEN** Tview selects the context named `production.us-west`, not a truncated or nested context

#### Scenario: Context target overrides a saved format
- **WHEN** a matching saved view specifies a non-Elasticsearch format but the positional source is `.production.es://`
- **THEN** the positional context target selects Elasticsearch instead of the saved format

#### Scenario: Incompatible explicit format
- **WHEN** a dot-context target is combined with `--format json` or another non-Elasticsearch format
- **THEN** Tview rejects the combination without running credential resolvers or opening a source

#### Scenario: Unsupported service
- **WHEN** a dot-context target selects `kibana`, `cloud`, or an unknown service
- **THEN** Tview reports that the target must select `es` or `elasticsearch` and does not open it as a file

#### Scenario: Ordinary path and URL compatibility
- **WHEN** an input is a local path without dot-context syntax, stdin, a file URI, or an ordinary HTTP(S) URL
- **THEN** its existing format and source-selection behavior remains unchanged

### Requirement: Dot-context table suffix
A dot-context target SHALL accept an optional literal table or index-pattern suffix after `://`. A non-empty suffix SHALL have the same meaning and validation as an explicit `--table` value, not an HTTP path appended to the configured endpoint. It SHALL override saved table or query selection. Combining a non-empty suffix with explicit `--table` or `--query` SHALL fail before credential resolution. An empty suffix SHALL preserve normal CLI and saved table or query selection, interactive discovery, and batch-selection requirements.

#### Scenario: Index-pattern suffix
- **WHEN** the user supplies `.production.es://logs-*`
- **THEN** Tview opens the context's configured endpoint and selects `logs-*` as the Elasticsearch table target

#### Scenario: Saved query does not override a suffix
- **WHEN** `.production.es://logs-*` matches a view containing `source.query`
- **THEN** the explicit suffix selects `logs-*` and the saved native query does not become the base query

#### Scenario: Conflicting CLI selections
- **WHEN** a non-empty suffix is combined with explicit `--table` or `--query`
- **THEN** Tview reports conflicting source selections rather than silently choosing one

#### Scenario: Empty suffix with a saved query
- **WHEN** `.production.es://` matches a view containing an Elasticsearch `source.query`
- **THEN** Tview executes that saved query without requiring a CLI table selector

#### Scenario: Empty suffix without a selection
- **WHEN** a dot-context source has no suffix, CLI selection, or saved selection
- **THEN** interactive mode uses the existing Elasticsearch picker and batch mode reports the missing selection

### Requirement: Read-only context discovery and lazy resolution
Tview SHALL use `ELASTIC_CLI_CONFIG_FILE` when supplied; otherwise it SHALL use the first readable `.elasticrc`, `.elasticrc.json`, `.elasticrc.yaml`, or `.elasticrc.yml` in the user's home directory, in that order. It SHALL load configuration without executing resolvers in unselected services and SHALL resolve only the selected Elasticsearch service. Tview SHALL NOT read Elastic CLI configuration or run its resolvers for unrelated source types or explicit HTTP(S) endpoints. It SHALL NOT create or modify Elastic CLI configuration.

#### Scenario: Explicit configuration path
- **WHEN** `ELASTIC_CLI_CONFIG_FILE` points to a valid configuration outside the home directory
- **THEN** Tview resolves the selected context from that file instead of home discovery

#### Scenario: Home discovery order
- **WHEN** multiple supported configuration files are readable in the home directory
- **THEN** Tview uses the first file in the documented discovery order

#### Scenario: Unselected resolver is inert
- **WHEN** another context or a Kibana or Cloud service contains a command resolver
- **THEN** opening the selected Elasticsearch context does not execute that resolver

#### Scenario: Direct URL does not load configuration
- **WHEN** a user opens an explicit Elasticsearch HTTP(S) endpoint while an Elastic CLI config is missing or invalid
- **THEN** the endpoint retains existing environment-based behavior without loading that config

### Requirement: Context-owned authentication
For dot-context sources, Tview SHALL use the selected service's API-key, basic, or unauthenticated mode without merging or validating Tview's `ELASTIC_API_KEY`, `ELASTIC_USERNAME`, or `ELASTIC_PASSWORD` as transport overrides. Environment values explicitly referenced by a context resolver SHALL remain available to that resolver. The existing `ELASTIC_CA_CERT` setting and certificate validation SHALL continue to apply. Resolved secrets SHALL remain runtime-only and SHALL NOT appear in debug output, saved views, query provenance, stdout, or diagnostics.

#### Scenario: Selected API key wins over unrelated transport variables
- **WHEN** a context resolves an API key while Tview credential variables describe another authentication mode
- **THEN** requests use only the context API key without an environment-mode conflict

#### Scenario: Unauthenticated context
- **WHEN** the selected context has no authentication and Tview credential variables are present
- **THEN** requests remain unauthenticated rather than inheriting those variables

#### Scenario: Explicit environment resolver
- **WHEN** the selected context explicitly resolves a password or API key from an environment variable
- **THEN** Tview uses the value returned by that resolver as context authentication

#### Scenario: Custom certificate authority
- **WHEN** a context endpoint uses a certificate trusted through `ELASTIC_CA_CERT`
- **THEN** Tview uses the configured CA without disabling certificate validation

#### Scenario: Secret-bearing resolver failure
- **WHEN** a selected credential resolver fails with secret-bearing output
- **THEN** the diagnostic identifies the failing context or resolution stage without displaying the secret or raw resolver output

### Requirement: Connection snapshot for an invocation
Tview SHALL select the concrete context name, resolve its service, and bind its endpoint and authentication once for an invocation before opening Elasticsearch results. Source-query changes and reloads SHALL reuse that connection snapshot and SHALL NOT reread configuration, rerun secret resolvers, or switch to a newly selected current context. A new invocation SHALL discover and resolve current configuration again. The established latest-successful-result and export-failure rules SHALL remain in force.

#### Scenario: Current context changes during viewing
- **WHEN** `.es://` selected `production` and the config's current context later changes to `staging`
- **THEN** reload and source-query replacement continue using the original production connection

#### Scenario: Credential resolver is not repeated per request
- **WHEN** discovery, mapping, queries, and reload all use a dot-context source
- **THEN** they reuse the resolved connection rather than executing a credential resolver for each operation

#### Scenario: New invocation observes configuration changes
- **WHEN** the user starts Tview again after changing the selected context or credentials
- **THEN** Tview resolves the updated configuration for that invocation

### Requirement: Context resolution failures
Missing configuration, missing current or named contexts, missing Elasticsearch services, failed resolvers, and unsupported or credential-bearing endpoint URLs SHALL fail source preparation with a safe actionable diagnostic. A batch preparation failure SHALL leave stdout empty and return exit code 1. Dot-context syntax or conflicting CLI selections SHALL be rejected before any resolver side effects. There SHALL be no fallback to another context, an environment-authenticated connection, or a filesystem path.

#### Scenario: Missing named context
- **WHEN** `.missing.es://` names a context absent from the selected file
- **THEN** source preparation fails without falling back to the current context

#### Scenario: Missing Elasticsearch service
- **WHEN** the selected context has only Kibana or Cloud configuration
- **THEN** source preparation reports that its Elasticsearch service is missing

#### Scenario: Unsupported resolved endpoint
- **WHEN** the selected service resolves to a non-HTTP(S) URL or a URL containing userinfo
- **THEN** Tview rejects it before an Elasticsearch request

#### Scenario: Batch resolution failure
- **WHEN** a selected context cannot be loaded or resolved during batch output
- **THEN** Tview returns 1, writes a safe diagnostic to stderr, and writes nothing to stdout
