# Spec Delta

## ADDED Requirements

### Requirement: Latest source result activation before export
Final interactive export SHALL await successful activation of the latest required source revision, not merely successful task execution. Execution, result-loading, and view-reconstruction failures SHALL prevent adapter bytes on stdout, produce a diagnostic on stderr, and exit nonzero. A retained prior result SHALL NOT silently substitute for a failed latest revision. Superseded revision outcomes SHALL NOT override the latest outcome.
This source-neutral activation rule refines the existing SQLite and Elasticsearch output-query completion requirements: waiting for their latest revision includes successful activation, not task completion alone. Their adapter-specific bounded-response, partial-result, and provenance rules remain unchanged.

#### Scenario: Latest replacement still pending at quit
- **WHEN** normal interactive quit requests final output while the latest source replacement is pending
- **THEN** final preparation awaits that revision's execution, result loading, and view reconstruction before freezing the view and writing adapter bytes

#### Scenario: Latest task fails before quit
- **WHEN** the latest source query failed during interaction and the prior result remains visible when final output is requested
- **THEN** stdout remains empty, stderr reports the failure, and the process exits nonzero rather than exporting the retained prior result

#### Scenario: Latest task succeeds but result loading fails
- **WHEN** the latest source task completes successfully but loading the returned result fails before activation
- **THEN** final export emits no stdout, reports the loading failure on stderr, and exits nonzero

#### Scenario: Latest view reconstruction fails
- **WHEN** the latest replacement's result loads but reconstruction of the local view or required identity reconciliation fails
- **THEN** final export emits no stdout, reports the reconstruction failure on stderr, and exits nonzero

#### Scenario: Newer successful revision supersedes failure
- **WHEN** an earlier source revision fails and a newer required revision successfully activates before final preparation
- **THEN** final output serializes the newer committed result and stale failure state does not prevent export

#### Scenario: Reload supersedes pending replacement before export
- **WHEN** reload supersedes a pending query by reopening committed source configuration and then final output is requested
- **THEN** final preparation uses the reload's activation outcome, not completion or failure of the superseded query
- **AND** failed reload emits no stdout while successful reload permits export of its committed result
