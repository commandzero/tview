# Spec Delta

## MODIFIED Requirements

### Requirement: Saved native query serialization
Saved-view serialization SHALL persist source-native input configuration separately from derived query provenance, using the committed source configuration of the last successfully activated result rather than draft or pending settings.

#### Scenario: Source operations around native query
- **WHEN** a native base query has source filters, source sort, or source limit
- **THEN** YAML stores the base under `source.query` and structured operations under their existing source fields

#### Scenario: Derived ES|QL is excluded
- **WHEN** Elasticsearch query provenance includes application-composed stages
- **THEN** generated YAML does not duplicate the final composed ES|QL as a second configuration value

#### Scenario: Derived SQLite SQL is excluded
- **WHEN** SQLite query provenance includes an outer bounded query
- **THEN** generated YAML does not replace the configured base query with the derived SQL

#### Scenario: Rejected SQLite native query is not saved
- **WHEN** `SELECT name AS label FROM events ORDER BY id` is committed, replacement with `SELECT missing AS label FROM events ORDER BY id` fails, and the user saves the current view
- **THEN** YAML persists `SELECT name AS label FROM events ORDER BY id` and the committed source operations, not the rejected text
- **AND** reopening that saved view succeeds against the unchanged fixture and returns the successful query's rows

#### Scenario: Save while replacement is pending
- **WHEN** a new source query is pending and saved-view YAML is generated
- **THEN** its entire `source` section describes the last successfully activated result without waiting for, applying, or mixing in the pending request
- **AND** its `view` section describes current successfully applied local settings over that active result

#### Scenario: Save after loading or reconstruction failure
- **WHEN** source execution succeeds but result loading or view reconstruction fails and the current view is saved
- **THEN** the saved source configuration remains the prior committed configuration and no candidate settings are serialized

## ADDED Requirements

### Requirement: Successful source configuration serialization
Saved-view authoring SHALL derive its entire `source` section from one committed source configuration. It SHALL preserve effective opening choices, generated relation or native base-query selection, source operations using durable active-result column keys, and effective limits in the existing YAML structure. Local view operations SHALL remain under `view`; transport secrets SHALL remain excluded.

#### Scenario: Preserve opening-only settings
- **WHEN** a result was opened with effective format, JSON path, resolved object mode, schema-scan policy, or a selected relation and then successfully replaced
- **THEN** generated YAML retains the applicable opening settings required to reopen that committed result under the existing serialization rules

#### Scenario: Save generated SQLite or Elasticsearch query
- **WHEN** a committed source result comes from a selected relation without user-supplied native query text
- **THEN** YAML persists the effective format, resolved `source.table`, and structured source operations
- **AND** it does not convert adapter-generated base or composed native text into `source.query`

#### Scenario: Save successfully changed native selection
- **WHEN** an existing generated relation query is replaced successfully with a user-supplied native base query
- **THEN** YAML writes that configured base under `source.query`, omits the mutually exclusive `source.table`, and preserves opening settings and structured operations

#### Scenario: Serialize adapter-remapped source operands
- **WHEN** successful replacement changes the source generation or column positions and its source filters or sorting are remapped by the adapter
- **THEN** YAML uses durable keys resolved from the committed active result rather than rejected draft IDs, stale ordinals, or generated placeholder column names

#### Scenario: Preserve finite source limit
- **WHEN** the committed source result uses a finite limit and pending or failed replacement requests a different limit
- **THEN** YAML retains the committed finite limit and reopening applies that same source-result bound

#### Scenario: Preserve existing unbounded file opening
- **WHEN** a committed file source was opened without a finite limit and uses the existing unbounded sentinel internally
- **THEN** generated YAML omits the limit under the existing file-source rule and replay remains unbounded
- **AND** native-query results retain their finite committed bounds without introducing extreme-integer unbounded serialization

#### Scenario: Source and view operations stay isolated
- **WHEN** the active source has source filters and sorting and the current local view has different filters and sorting
- **THEN** YAML serializes each layer under its corresponding section and reopening does not expand the source result to compensate for view filtering
