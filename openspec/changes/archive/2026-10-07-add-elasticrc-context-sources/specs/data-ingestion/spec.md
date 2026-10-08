## MODIFIED Requirements

### Requirement: Input source support
The system SHALL represent positional source targets as filesystem paths, `file://` URI paths, standard input, parsed remote URLs, or dot-context Elasticsearch references. It SHALL recognize dot-context syntax before generic URL and filesystem handling and SHALL pass each target to the resolved format adapter without interpreting a remote URL or context reference as a local path. Merely parsing a context target SHALL NOT load configuration or execute credential resolvers.

#### Scenario: File URI path
- **WHEN** a user runs `tview file:///tmp/data.csv`
- **THEN** the system reads `/tmp/data.csv`

#### Scenario: Standard input target
- **WHEN** a user runs `tview -`
- **THEN** the system treats standard input as the source byte stream

#### Scenario: Remote URL target
- **WHEN** a user supplies a syntactically valid non-file URL
- **THEN** the system retains its scheme, authority, path, and safe display form for adapter resolution

#### Scenario: Remote URL is not a path
- **WHEN** an HTTP(S) or `libsql://` target is parsed
- **THEN** Tview does not call local filesystem metadata or file-opening operations for that target

#### Scenario: Dot-context is not a path
- **WHEN** `.production.elasticsearch://logs-*` is parsed
- **THEN** Tview retains a context reference and table suffix, selects Elasticsearch, and does not perform file probing or URL scheme parsing on the reference

#### Scenario: Dot-prefixed local filename
- **WHEN** a user opens a path such as `.es`, `.production.es`, or `./.production.es`
- **THEN** Tview treats it as a local path because it lacks the dot-context `://` delimiter

#### Scenario: Parse without resolver side effects
- **WHEN** help, option validation, or source parsing examines a dot-context target
- **THEN** it does not read Elastic CLI configuration or execute resolvers
