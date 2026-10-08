## MODIFIED Requirements

### Requirement: Saved remote source target
Saved-view matching and serialization SHALL support the safe textual identity of remote URLs and dot-context sources while excluding URL userinfo, credentials, authorization headers, resolver expressions, and other transport secrets. Dot-context identity SHALL retain current-context or named-context selection and any explicit table suffix, normalize the service alias to `elasticsearch`, and exclude the resolved endpoint and credentials. Context names SHALL match exactly; service alias variants SHALL use the same saved-view identity.

#### Scenario: Match Elasticsearch endpoint
- **WHEN** a saved view targets an Elasticsearch endpoint and its safe target pattern matches the invocation
- **THEN** normal saved-view selection and source-option merging apply

#### Scenario: Serialize remote target
- **WHEN** a saved view is generated for a direct remote URL source
- **THEN** its matching target uses the endpoint's safe non-secret representation

#### Scenario: Secret-bearing URL
- **WHEN** a supplied remote URL contains user information or another secret-bearing component
- **THEN** generated YAML, diagnostics, and query artifacts omit or redact that component

#### Scenario: Equivalent context aliases
- **WHEN** invocations use `.production.es://logs-*` and `.production.elasticsearch://logs-*`
- **THEN** both use the same canonical context identity for saved-view matching and generated filenames

#### Scenario: Save a context source
- **WHEN** the user saves a view opened with `.production.es://logs-*`
- **THEN** its matching identity uses `.production.elasticsearch://logs-*`, its source section retains committed query or table settings, and no resolved endpoint, credentials, or resolver expression is copied into YAML

#### Scenario: Save current-context selection
- **WHEN** the user saves a view opened with `.es://`
- **THEN** its matching identity retains current-context selection as `.elasticsearch://`
- **AND** a subsequent invocation resolves the then-current context while reload in the original invocation stays on its pinned connection

#### Scenario: Matching precedes credential resolution
- **WHEN** Tview selects a saved view for a dot-context source
- **THEN** it uses the non-secret reference identity without running a credential resolver to discover the view
