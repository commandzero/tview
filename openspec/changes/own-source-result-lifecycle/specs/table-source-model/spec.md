# Spec Delta

## MODIFIED Requirements

### Requirement: Atomic asynchronous source replacement
Source-query execution SHALL be revisioned so slow or superseded asynchronous results cannot overwrite newer operation state. A successful replacement SHALL atomically publish its committed source configuration, result schema, row store, metadata, and reconstructed view only after result loading and reconstruction succeed; acceptance or task completion alone SHALL NOT commit a replacement.

#### Scenario: Current revision completes
- **WHEN** the latest source query completes successfully
- **THEN** its result schema, store, extent, provenance, cursor reconciliation, and view transform are published together
- **AND** its committed source configuration becomes the single save and reload authority in the same publication

#### Scenario: Current revision changes schema
- **WHEN** the latest native query completes successfully with added, removed, renamed, reordered, or retyped result columns
- **THEN** a new table definition and compatible remapped view state are published atomically with its store, extent, and provenance
- **AND** the active query uses the adapter-derived generation and column identities of that result rather than stale identities from the draft

#### Scenario: Stale revision completes
- **WHEN** an earlier source query completes after a newer revision was requested
- **THEN** the stale table definition and result are discarded together
- **AND** neither its requested source configuration nor its outcome changes the current committed configuration or latest-revision status

#### Scenario: Replacement fails
- **WHEN** a replacement query fails
- **THEN** the prior successful table definition, source result, and view remain usable while the error is reported
- **AND** the prior committed source configuration remains the save and reload authority

#### Scenario: Task succeeds but result loading fails
- **WHEN** the latest query task returns a result but loading its initial rows or schema fails
- **THEN** the replacement is reported as failed, no part of its configuration or result becomes active, and the prior committed configuration, result, and view remain usable

#### Scenario: View reconstruction fails
- **WHEN** the latest replacement result loads but applying compatible local filters, sorting, or required identity reconciliation fails
- **THEN** the replacement is reported as failed without partially publishing its source configuration, table definition, store, or reconstructed view

#### Scenario: Newer request supersedes a ready candidate
- **WHEN** a replacement candidate has completed execution but a newer revision is requested before publication
- **THEN** the older candidate cannot publish its configuration or reconstructed result

#### Scenario: Latest failure outlives stale success
- **WHEN** the latest revision fails and an older superseded revision subsequently succeeds
- **THEN** the last committed configuration and result remain active and the latest failure remains authoritative

## ADDED Requirements

### Requirement: Committed source configuration
The active source result SHALL retain the effective opening settings and source-native input configuration required to reopen it. Draft and pending configuration SHALL remain distinct from committed source configuration. Durable source operands SHALL resolve through the active result's source identities, and derived query provenance SHALL NOT replace the native base query or generated relation selection.

#### Scenario: Draft or pending request leaves committed configuration intact
- **WHEN** source settings are edited or a validated replacement request is accepted but has not successfully activated
- **THEN** the active result's opening settings, selected relation or native base query, source filters, source ordering, and effective limit remain unchanged

#### Scenario: Opening settings survive replacement
- **WHEN** a source replacement changes filters, ordering, limit, or native base query
- **THEN** unchanged effective format, delimited interpretation, JSON path, object interpretation, schema-scan policy, and other opening-only settings remain associated with the committed result
- **AND** generated relation selection is retained unless a successfully activated native-query selection replaces it

#### Scenario: Generated source configuration
- **WHEN** a selected relation executes through adapter-generated native query text
- **THEN** committed configuration retains the resolved relation selection and structured source operations without treating composed query provenance as user-supplied native input

#### Scenario: Native source configuration
- **WHEN** a user-supplied native base query successfully activates with structured source filters, sort keys, and a limit
- **THEN** committed configuration retains the configured base text separately from those structured operations and the composed inspection artifact

#### Scenario: File opening retains unbounded omission
- **WHEN** a file source was opened without a finite limit and uses the existing unbounded sentinel internally
- **THEN** replacement, save configuration, and reload preserve that choice through the existing omission behavior, while query-native sources retain their supported finite bounds

#### Scenario: View changes do not alter source configuration
- **WHEN** local view filters, sorting, labels, formatting, or visibility change
- **THEN** committed source configuration and the source-result boundary do not change and no replacement source query is requested

### Requirement: Reload committed source configuration
Reload SHALL reopen the last committed source configuration and supersede any pending replacement before it can publish. A successful reload SHALL atomically activate a new source generation and compatible durable view state. A failed reload SHALL leave the prior committed aggregate unchanged, record the failure, and prevent superseded work from activating afterward before propagating the error through the existing fatal interactive reload route. Interactive reload failures SHALL remain fatal. Reloading stdin SHALL retain its existing no-op behavior.

#### Scenario: Reload while replacement is pending
- **WHEN** a reload is requested while a different source query is pending
- **THEN** the reload opens the last committed source configuration rather than the pending query
- **AND** completion or failure of the superseded pending query cannot replace the reloaded result or alter the reload outcome

#### Scenario: Reload after rejected native query
- **WHEN** a native query replacement fails and the source is reloaded
- **THEN** the prior successful native base query and source operations are used and the rejected native text is not retried

#### Scenario: Successful reload refreshes identities
- **WHEN** reloading the committed source configuration succeeds
- **THEN** a new source generation is activated, old generation-scoped row identities and derived stores are discarded, and durable column state is re-resolved through compatible source identities

#### Scenario: Reload opening or reconstruction fails
- **WHEN** committed source reopening, result loading, or compatible view reconstruction fails during reload
- **THEN** the prior committed result and configuration remain unchanged until normal error propagation ends the interactive session, and the reload failure is retained as the latest lifecycle outcome
- **AND** a previously pending query cannot later activate even though reload failed

#### Scenario: Reload stdin
- **WHEN** reload is requested for a stdin source
- **THEN** it performs no reopen or new generation activation

### Requirement: Compatible replacement identity reconciliation
Replacement SHALL remap identity-backed column state only through unambiguous compatible source identities, not unrelated positions or display labels. Cursor-following and marks SHALL survive compatible query replacement only through adapter-proven stable row identities; unavailable, missing, or nonunique identities SHALL reset row-bound state. Reload SHALL discard old generation-scoped row identities.

#### Scenario: Retained columns move
- **WHEN** replacement reorders columns while retaining unambiguous compatible source identities
- **THEN** widths, labels, formatting, visibility, local filter and sort operands, and cursor-column selection follow those identities rather than the old visible positions

#### Scenario: Unrelated column occupies previous position
- **WHEN** replacement removes or renames a source column and a different identity occupies the same ordinal
- **THEN** the previous column's local operations and presentation overrides do not migrate to that unrelated column and normal stale-configuration reporting or omission applies

#### Scenario: Ambiguous or incompatible column identity
- **WHEN** replacement returns duplicate identities or incompatible type lineage for previous identity-backed state
- **THEN** the system does not guess a target by display label or ordinal and omits or reports that stale state according to existing policy

#### Scenario: Keyed rows survive compatible replacement
- **WHEN** the adapter proves stable unique row identity for both results and the selected or marked row remains in the new source result and local view
- **THEN** cursor-following and marks refer to that same row after replacement regardless of its new position

#### Scenario: Keyless or removed rows reset state
- **WHEN** replacement cannot prove stable identity, exposes nonunique identity, or no longer includes the selected or marked row
- **THEN** affected cursor-following and marks reset rather than following the same row ordinal or a similarly named arbitrary key field

#### Scenario: Identity lookup fails rather than proving absence
- **WHEN** reading the replacement store to reconcile a required stable identity fails
- **THEN** the replacement fails without committing instead of treating the read error as an absent row and silently activating it

### Requirement: Revision worker lifetime
Revision coordination SHALL retain ownership of asynchronous and blocking source work until cancellation or completion releases its resources. Superseded work SHALL NOT publish configuration, results, or errors over the latest revision. Interactive execution SHALL remain responsive while replacement runs, and shutdown SHALL NOT detach running blocking source work from its required source and runtime lifetime.

#### Scenario: Supersede asynchronous source request
- **WHEN** a newer revision supersedes a remote asynchronous request
- **THEN** the earlier request is cancelled when supported and any completion that still occurs cannot publish or poison the latest outcome

#### Scenario: Supersede blocking local request
- **WHEN** a newer revision supersedes already running blocking source work that cannot be cancelled
- **THEN** the work retains its owned resources until completion and its stale result or error cannot activate

#### Scenario: Shutdown with blocking source work
- **WHEN** the application shuts down while blocking source work is running
- **THEN** shutdown retains worker and runtime ownership until the work finishes and releases its resources without later publication

#### Scenario: Shutdown with asynchronous source work
- **WHEN** the application shuts down while asynchronous source work is running
- **THEN** shutdown cancels supported work or awaits necessary completion, releases owned resources, and prevents publication after the lifecycle closes
