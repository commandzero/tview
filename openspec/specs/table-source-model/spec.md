## Purpose

Define format-aware source opening and the stable, typed table definition and row-store contract.

## Requirements

### Requirement: Format-aware source opening
The system SHALL resolve an input format and open it through a format adapter rather than applying delimited parsing to every source.

#### Scenario: Explicit format selection
- **WHEN** a user explicitly selects a supported input format
- **THEN** the system uses that adapter without content-based format inference

#### Scenario: Automatic format selection
- **WHEN** no explicit or saved-view format is selected
- **THEN** the system resolves a supported adapter from the source name and bounded content probing

### Requirement: Opened table contract
Every opened table SHALL provide a table definition and a row store as separate responsibilities.

#### Scenario: Source constructs table definition
- **WHEN** an adapter opens a table
- **THEN** it supplies ordered column definitions, schema completeness, and source metadata without asking `TableView` to consume a data row as a header

#### Scenario: Store supplies rows
- **WHEN** the viewer requests table data
- **THEN** it obtains logical rows, row-count state, indexing progress, and materialization behavior through the store boundary

### Requirement: Stable column definitions
Each source column SHALL have stable internal identity, source identity, display name, type metadata, and first-seen source order as applicable to its format.

#### Scenario: Duplicate display names
- **WHEN** two columns have identical or ambiguous source names
- **THEN** stable column identity remains distinct from the rendered display name

#### Scenario: View changes presentation
- **WHEN** a saved view changes a column label, type interpretation, format, width, alignment, or visibility
- **THEN** the source identity and raw typed values remain unchanged

### Requirement: Generation-scoped row and column identity
Every opened relation SHALL have a source generation, every row SHALL have opaque identity within that generation, and derived state SHALL NOT apply generation-scoped identities to another generation.

#### Scenario: File-backed row identity
- **WHEN** a delimited, JSON, or NDJSON adapter identifies a logical source row
- **THEN** it assigns a row identity derived from that logical source position and preserves it in filtered or sorted results

#### Scenario: Reload creates a generation
- **WHEN** the source is reloaded
- **THEN** the system creates a new source generation, discards old row identities and query result stores, and re-resolves durable column configuration through source identity

#### Scenario: Source changes during incremental access
- **WHEN** an adapter detects that a seekable source was replaced, truncated, or changed incompatibly after its generation opened
- **THEN** it fails the affected operation without mixing versions or activating partial derived state and reports that reload is required

### Requirement: Typed cell values
The opened-table model SHALL preserve null, boolean, integer, floating-point, text, binary, and structured JSON distinctions until presentation formatting is applied.

#### Scenario: Null differs from empty text
- **WHEN** a source contains both a null value and an empty string
- **THEN** the table model represents them as distinct raw values

#### Scenario: Numeric value remains numeric
- **WHEN** JSON supplies a native integer or floating-point value
- **THEN** the table model preserves its numeric kind without first converting it to display text

### Requirement: Schema completeness and updates
The table definition SHALL represent whether its schema is complete or provisional and SHALL support append-only schema updates from incremental discovery.

#### Scenario: Bounded discovery is provisional
- **WHEN** an adapter stops schema discovery before the selected table ends
- **THEN** it marks the schema provisional

#### Scenario: End of table completes schema
- **WHEN** indexing or a full schema scan reaches the selected table's end
- **THEN** the adapter marks the schema complete

#### Scenario: Schema delta reaches the view
- **WHEN** incremental indexing discovers a new source column
- **THEN** the store reports an append-only schema delta so view metadata can be extended without rebuilding existing column identity

### Requirement: Selectable multi-table source
A source containing multiple user-visible tables or views SHALL expose its catalog and allow exactly one selectable table to be opened lazily as an `OpenedTable`. Catalog entries MAY be unavailable for selection when the adapter retains an actionable capability reason.

#### Scenario: Relational source awaits selection
- **WHEN** an adapter discovers multiple selectable tables or views and merged source options do not select one
- **THEN** `OpenedSource` exposes the catalog and a source-owned selection mechanism without opening row stores for every candidate

#### Scenario: Relational source selection
- **WHEN** the application selects one selectable catalog entry
- **THEN** the source opens exactly that table or view as an `OpenedTable`

#### Scenario: Unavailable relational entry
- **WHEN** an adapter discovers a user-visible relation that it cannot query safely or compatibly
- **THEN** the catalog may retain the entry as unavailable with a reason without allowing it to create an `OpenedTable`

#### Scenario: Existing implicit source is opened
- **WHEN** a delimited, JSON, or NDJSON adapter opens its single implicit table
- **THEN** it continues to produce one selected `OpenedTable` through the same contract

### Requirement: Relational column source identity
Columns supplied by a relational adapter SHALL use stable source identity containing the selected table, source ordinal, and source name.

#### Scenario: Duplicate relational names
- **WHEN** a table exposes two columns with the same source name
- **THEN** their distinct source ordinals keep their source identities and runtime column IDs distinct

#### Scenario: Relational reload
- **WHEN** the same compatible table is reopened into a new source generation
- **THEN** durable column configuration is remapped through relational source identity rather than stale generation-scoped column IDs

#### Scenario: Database metadata bypasses delimited header inference
- **WHEN** a relational adapter supplies column metadata independently of result rows
- **THEN** the first result row remains data and the supplied definitions are used without delimited header classification

### Requirement: Source query and view transform boundary
The table model SHALL represent source operations separately from view operations. A `SourceQuery` SHALL contain source-native filters, source-native sort keys, and a row limit; a `ViewTransform` SHALL contain source-independent filters and sort keys applied only to the active source result.

#### Scenario: Source query replacement
- **WHEN** a source filter, source sort key, or source limit changes
- **THEN** the store executes a replacement source query and atomically publishes its result when complete

#### Scenario: View transform replacement
- **WHEN** only a view filter or view sort key changes
- **THEN** the viewer recomputes the derived view from the active source result without reopening or expanding the source query

#### Scenario: Stable column operands
- **WHEN** either operation layer references a column
- **THEN** it uses stable column identity independently of display label and visible position

### Requirement: Bounded source result
An opened table SHALL expose an active source result bounded by its source query, and source-query execution SHALL NOT fall back to unbounded materialization when a store cannot execute an operation.

#### Scenario: Store supports source query
- **WHEN** a store accepts a complete source query
- **THEN** it returns no more rows than the query limit and records the result extent

#### Scenario: Store rejects source query
- **WHEN** a store cannot execute a requested source filter or source sort
- **THEN** the operation is reported as unsupported and the previous successful result remains active

#### Scenario: Local view execution
- **WHEN** a view transform is applied
- **THEN** the canonical local executor may materialize only the already bounded active source result

### Requirement: Source result metadata and provenance
An active source result SHALL report whether it is known complete, limited, or partial and MAY expose a language-neutral source-native query artifact for inspection and reuse.

#### Scenario: Result reaches source end
- **WHEN** fewer rows satisfy the source query than its limit and the store reaches the end
- **THEN** the result extent is `Complete`

#### Scenario: Result reaches limit
- **WHEN** the store returns the configured number of rows without proving source exhaustion
- **THEN** the result extent is `Limited`

#### Scenario: Source reports partial execution
- **WHEN** a remote source reports that only a partial result was computed
- **THEN** result metadata preserves partial status independently of complete-or-limited extent

#### Scenario: SQLite query provenance
- **WHEN** a SQLite source result is active
- **THEN** its metadata identifies SQL as the native language and exposes logical text, bound values, and a copyable representation using the configured source limit

#### Scenario: Elasticsearch query provenance
- **WHEN** an Elasticsearch source result is active
- **THEN** its metadata identifies ES|QL as the native language and exposes logical text, bound value and identifier parameters, and a copyable representation

#### Scenario: File source has no query language
- **WHEN** a file-backed source does not have a native query artifact
- **THEN** result metadata omits query text without affecting source or view operations

### Requirement: Relational row identity
A relational store SHALL preserve stable row identity across successful source-query replacements when the selected table exposes a usable primary key or row identifier, and SHALL explicitly reset row-bound state when it cannot establish stable identity.

#### Scenario: Stable keyed table row
- **WHEN** a SQLite table has a usable primary key or `rowid`
- **THEN** the adapter derives row identity from that key and retains it through view transformations and compatible source-query replacements

#### Scenario: Keyless result
- **WHEN** a selected view or `WITHOUT ROWID` table does not expose a usable stable key
- **THEN** the result reports that stable row identity is unavailable and replacement invalidates cursor-following and marks tied to prior rows

#### Scenario: Duplicate key metadata
- **WHEN** a purported identity is not unique in the active result
- **THEN** the store does not silently use it as stable identity

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

### Requirement: Native query request model
The source model SHALL represent an optional native base query separately from generic source operations and SHALL let each adapter validate and compose its own query language without exposing source-specific CLI fields in the table model.

#### Scenario: Source has native query
- **WHEN** a SQLite or Elasticsearch source receives `source.query`
- **THEN** the adapter receives the unchanged configured text as its native base query

#### Scenario: Source has generated query
- **WHEN** a query-native source receives a selected relation and no native query
- **THEN** the adapter generates its native base query from resolved source metadata

#### Scenario: Source has no native language
- **WHEN** a file-backed adapter receives a native query
- **THEN** capability validation rejects it instead of interpreting the text as a file filter

### Requirement: Native query result definition
An adapter whose native query can shape columns SHALL construct `TableDefinition` from prepared or returned result metadata and SHALL assign a new source generation when replacement changes the result definition.

#### Scenario: SQLite prepared columns
- **WHEN** a native SQLite query is prepared successfully
- **THEN** its result column metadata defines the opened implicit table

#### Scenario: ES|QL response columns
- **WHEN** an ES|QL request succeeds
- **THEN** its returned column names and types define the active table

#### Scenario: Same names but changed types
- **WHEN** a replacement retains column names but changes source types
- **THEN** Tview treats the result definition as changed rather than retaining stale type metadata

#### Scenario: Saved column remapping
- **WHEN** a new result definition retains unambiguous compatible source identities
- **THEN** saved and active presentation state is remapped to those columns while stale identities are reported or omitted according to existing policy

### Requirement: Asynchronous source task kinds
The source-query coordinator SHALL support asynchronous network jobs and blocking local jobs through one revision contract without blocking the terminal event loop or creating an asynchronous call per rendered cell.

#### Scenario: Remote async job
- **WHEN** Elasticsearch performs discovery or ES|QL execution
- **THEN** its client future runs on the application Tokio runtime

#### Scenario: Blocking local job
- **WHEN** a local source operation uses blocking work
- **THEN** it runs through the runtime's blocking boundary while publishing the same revisioned completion event

#### Scenario: Render cached result
- **WHEN** the table body renders a completed remote source result
- **THEN** cells are read from the active store without issuing per-row or per-cell HTTP requests

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
