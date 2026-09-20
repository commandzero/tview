## Purpose

Define table model behavior and user-facing table operations for the Rust `tview` viewer.

## Requirements

### Requirement: Header row behavior
The system SHALL render and toggle a fixed header from source column definitions while preserving compatible delimited first-row classification and the selected data cell where possible.

#### Scenario: Non-numeric delimited first row
- **WHEN** a multi-row delimited table has a first row with no numeric cells and no explicit header policy overrides classification
- **THEN** the delimited adapter consumes the first row as column names and shows those names as the fixed header by default

#### Scenario: Structured source columns
- **WHEN** JSON, NDJSON, or another structured adapter defines source columns without a header record
- **THEN** the viewer renders those column display names without removing the first data row

#### Scenario: Headerless generated columns
- **WHEN** a source has no named columns and the adapter generates stable column definitions
- **THEN** the adapter can keep the generated header hidden by default while retaining column identity

#### Scenario: Toggle header
- **WHEN** a user presses `t` for a table with renderable column definitions
- **THEN** the fixed header row is toggled on or off while preserving the selected data cell where possible

### Requirement: Operations over partial stores
Table operations SHALL run against an explicit active source result. Source operations MAY query or incrementally scan the underlying source up to their configured limit; view operations, search, navigation, and reductions SHALL remain bounded by the active source result unless the user explicitly changes the source query.

#### Scenario: Search incrementally reaches a later source-result row
- **WHEN** the next search result lies beyond the indexed portion of the active source result
- **THEN** search indexes forward until it finds the matching cell or reaches the end of that bounded result

#### Scenario: View filter requires complete result scan
- **WHEN** a view filter uses the generic local executor
- **THEN** the viewer may scan or materialize the complete active source result but does not read beyond its source-query boundary

#### Scenario: Source query result is incremental
- **WHEN** a store executes a source query and can stream its bounded result
- **THEN** the viewer may present rows incrementally while preserving the query limit and result-extent metadata

#### Scenario: Operation failure preserves state
- **WHEN** source execution or local view materialization fails
- **THEN** the previously successful source result, view order, cursor, filters, and sort state remain valid

#### Scenario: Current-cell yank remains local
- **WHEN** a user yanks the current raw or rendered cell
- **THEN** the viewer does not clone, materialize, or fetch unrelated rows

### Requirement: Source-neutral table queries
The system SHALL model persistent table operations as a source query followed by a view transform. Both layers SHALL use stable column identity independently of source format, display label, and visible position, while allowing the source layer to expose only operations its adapter can execute natively.

#### Scenario: Column label changes
- **WHEN** a label is overridden or a visible column moves while either layer references that column
- **THEN** the operation continues to reference the same stable source column

#### Scenario: Multiple operation clauses
- **WHEN** a layer contains multiple filters and sort keys
- **THEN** it preserves that layer's filter-combination behavior and multi-sort precedence in one complete request

#### Scenario: Layer ordering
- **WHEN** both layers are active
- **THEN** source filters and source sort run before the source limit, and view filters and view sort run afterward

#### Scenario: Structured column identity
- **WHEN** an operation references a structured or relational column
- **THEN** it resolves stable source identity rather than executing against the display label

### Requirement: Canonical execution and store fallback
The system SHALL use the generic local executor as the canonical implementation of view filters and view sorts. A source query SHALL use adapter-defined native semantics, and unsupported source operations SHALL NOT fall back to unbounded local materialization.

#### Scenario: Operation validation precedes execution
- **WHEN** either layer contains a stale or unknown column ID, invalid predicate, unsupported mode, or wrong source-generation reference
- **THEN** validation fails before execution and the active result and view remain unchanged

#### Scenario: Source store does not support an operation
- **WHEN** a store reports a requested source filter or source sort as unsupported
- **THEN** the viewer reports the capability limitation and retains the previous successful source query

#### Scenario: View operation executes locally
- **WHEN** a view filter or view sort is applied to any source
- **THEN** the generic local executor applies it to the active bounded source result

#### Scenario: File source capability
- **WHEN** a file adapter supports a source filter over logical records
- **THEN** it may execute that filter before the limit while continuing to use the canonical view executor for view operations

#### Scenario: Execution fails
- **WHEN** a store accepts a source query but execution fails, or local view execution fails
- **THEN** the error is reported and the previously successful operation state, result, cursor, and viewport remain active

### Requirement: Deterministic typed operation semantics
The system SHALL use one canonical comparator and predicate behavior for view operations across all sources. Source operations SHALL use documented source-native typed semantics and SHALL NOT be required to reproduce advanced view behavior such as rendered-value matching, Rust regex, natural sort, or Tview-specific numeric parsing.

#### Scenario: Equal view-sort keys
- **WHEN** two rows compare equal under every active view-sort key
- **THEN** their relative order matches active source-result order

#### Scenario: Default view null placement
- **WHEN** null and non-null cells are view-sorted without view or column null-placement configuration
- **THEN** null cells appear after non-null cells in either direction and remain distinct from empty text

#### Scenario: Typed view comparison
- **WHEN** a view operation receives native integer, floating-point, text, boolean, blob, or null cells
- **THEN** it applies the canonical local typed behavior without converting source semantics into SQL

#### Scenario: Canonical view text and regex behavior
- **WHEN** view text, lexical, natural, or regex behavior is evaluated
- **THEN** it uses the existing local case sensitivity, string ordering, natural tokenizer, Rust `regex` behavior, and requested raw or rendered domain

#### Scenario: Native source semantics differ
- **WHEN** SQLite collation, null placement, type affinity, or comparison semantics differ from Tview's view semantics
- **THEN** the source operation uses SQLite behavior and the UI identifies it as a source operation

### Requirement: Configurable sort null placement
The viewer SHALL support direction-independent `first` or `last` null placement as a view-wide sorting default with an optional per-column override, and SHALL include the resolved policy in every sort key.

#### Scenario: View-wide nulls first
- **WHEN** a view configures `nulls: first` and a sorted column has no override
- **THEN** null cells sort before non-null cells for both ascending and descending direction

#### Scenario: Column overrides view default
- **WHEN** a view configures `nulls: first` and the sorted column configures `nulls: last`
- **THEN** that column's null cells sort after non-null cells in both directions

#### Scenario: Multi-column null policies
- **WHEN** active sort keys have different effective null-placement policies
- **THEN** each comparison key applies its own resolved policy in multi-sort precedence order

#### Scenario: Null policy changes during active sort
- **WHEN** the effective null placement changes for a column already in the active sort query
- **THEN** the viewer rebuilds and atomically re-executes the query while preserving prior state on failure

#### Scenario: Textual null placeholder
- **WHEN** a delimited text cell contains a placeholder such as `null`
- **THEN** null placement does not treat it as `CellValue::Null`; existing type-specific placeholder ordering remains applicable

### Requirement: Derived query results preserve source order
The system SHALL apply view filters and view sorts to a derived logical row set without mutating active source-result order, and SHALL replace the source result only when a source operation changes.

#### Scenario: View sort is cleared
- **WHEN** the user clears all view-sort keys and no view filters remain
- **THEN** rows return to active source-result order without reopening the source

#### Scenario: Source sort is cleared
- **WHEN** the user clears a source-sort key
- **THEN** the system executes a replacement source query and then reapplies the current view transform

#### Scenario: One view operation remains active
- **WHEN** the user clears one view sort or filter while other view clauses remain
- **THEN** the remaining view transform atomically replaces the previous derived result

#### Scenario: Replacement construction fails
- **WHEN** source execution, indexing, materialization, or local view execution fails before a replacement is complete
- **THEN** the previous source result and derived view remain unchanged

### Requirement: Query transitions preserve row identity
The viewer SHALL track cursor selection and marks by stable row and column identity across successful view transitions and compatible source-query replacements when the source provides stable row identity.

#### Scenario: Selected row moves after view sorting
- **WHEN** a successful view sort moves the selected row
- **THEN** the cursor follows that row identity and retains the selected column identity when visible

#### Scenario: Selected row is filtered out
- **WHEN** either filtering layer excludes the selected row
- **THEN** the viewer clamps the previous visible position into the new result

#### Scenario: Marked row temporarily leaves the view
- **WHEN** a view filter excludes a marked row
- **THEN** its mark remains associated with the stable row identity and becomes reachable again if a later view includes it

#### Scenario: Source has no stable row identity
- **WHEN** a source-query replacement cannot correlate rows with the prior result
- **THEN** row-bound cursor following and marks are reset rather than attached by result position

#### Scenario: Generation changes
- **WHEN** reload opens a new source generation
- **THEN** old row identities and marks are invalidated

### Requirement: Operation categories remain distinct
The system SHALL keep source queries, view transforms, progressive navigation, and scan or reduction operations distinct.

#### Scenario: Source query is persistent
- **WHEN** source filters, source sort, or source limit are active
- **THEN** they determine persistent membership, ordering, and maximum size of the active source result

#### Scenario: View transform is persistent
- **WHEN** view filters or view sort are active
- **THEN** they determine persistent membership and ordering only within the active source result

#### Scenario: Search and skip remain progressive
- **WHEN** search or skip-to-change traverses the active view
- **THEN** it uses bounded row scans without adding a source or view filter or sort clause

#### Scenario: Column analysis is a reduction
- **WHEN** width calculation, profiling, range analysis, or identifier analysis inspects many rows
- **THEN** it uses sampled or exact scan/fold behavior over the active source result without replacing it

#### Scenario: View does not refill source result
- **WHEN** view operations reduce the visible row count below the source limit
- **THEN** the viewer does not request additional source rows automatically

### Requirement: Column sizing controls
The system SHALL support fixed, mode, and max column width modes plus interactive width and gap adjustments using `z` and `Z` for the former all-column and current-column width commands.

#### Scenario: Increase current column width
- **WHEN** a user presses `.`
- **THEN** the current column width increases and the viewport layout is recalculated

#### Scenario: Set fixed width with modifier
- **WHEN** a user presses `20z`
- **THEN** all columns use fixed width 20 subject to terminal constraints

#### Scenario: Toggle all-column width mode
- **WHEN** a user presses `z` without a numeric prefix
- **THEN** the viewer toggles variable column width mode between `mode` and `max`

#### Scenario: Maximize current column
- **WHEN** a user presses `Z` without a numeric prefix
- **THEN** the current column width is maximized using existing max-content sizing behavior

#### Scenario: Set current column width with modifier
- **WHEN** a user presses `20Z`
- **THEN** the current column uses fixed width 20 subject to terminal constraints

### Requirement: Sort operations
The system SHALL support ascending and descending lexical, natural, numeric, and type-aware multi-level sort on the current column using the existing keybindings plus composable column sort commands. Numeric sort SHALL treat plain numbers, recognized suffixed numbers, and multi-dot numeric values as numeric values, while leaving non-numeric values after numeric values in ascending order. Shortcut sort operations SHALL maintain an ordered sort list with at most three entries.

#### Scenario: Numeric ascending sort
- **WHEN** a user presses `#`
- **THEN** rows are sorted by the current column using numeric comparison where values parse as numbers, and the current column becomes the primary sort key

#### Scenario: Scientific and byte suffix numeric sort
- **WHEN** numeric sort is applied to values with scientific suffixes from nano through exa, byte suffixes such as `kb`, `MB`, `GiB`, and `MiB`, or decimal percent suffixes such as `2.5%`
- **THEN** those values are compared using their numeric magnitude, with `%` using no multiplier beyond the numeric value itself

#### Scenario: Time-context suffix numeric sort
- **WHEN** a numeric column contains explicit time suffixes such as `ns`, `us`, `ms`, `s`, `min`, `h`, `d`, or `y`, or the column header suggests time-like data such as duration, latency, elapsed, runtime, uptime, timeout, or interval
- **THEN** numeric sort treats bare `m` as minutes for that column

#### Scenario: Non-time bare m numeric sort
- **WHEN** a numeric column does not have time-context evidence
- **THEN** numeric sort treats bare `m` as the scientific milli suffix

#### Scenario: Multi-dot numeric sort
- **WHEN** numeric sort is applied to values with multiple dot-separated numeric groups such as IP addresses or semantic versions
- **THEN** those values are compared component-by-component numerically

#### Scenario: Placeholder values in numeric columns
- **WHEN** a numeric column contains placeholder values such as `null`, `n/a`, `na`, `none`, `nil`, or `nan`
- **THEN** those placeholders do not prevent the column from being treated as numeric and sort after numeric values in ascending order

#### Scenario: Sticky numeric column profile
- **WHEN** the viewer classifies a column as time-context or default numeric context
- **THEN** that numeric interpretation remains stable for subsequent sorts and rendering until the table is reloaded or reclassified

#### Scenario: Numeric column alignment
- **WHEN** a visible column contains only numeric values, empty cells, or recognized placeholder values
- **THEN** data cells in that column are right-aligned while headers remain left-aligned

#### Scenario: Shortcut sort keeps last three keys
- **WHEN** a user sorts columns A, B, C, and D using `s/S`, `a/A`, or `#/@` shortcuts
- **THEN** the sort list keeps D as the primary key followed by C and B as trailing sort keys, and drops A

#### Scenario: Shortcut sort removes duplicate column
- **WHEN** a user sorts column A, then column B, then column A again using `s/S`, `a/A`, or `#/@` shortcuts
- **THEN** column A becomes the primary sort key and its previous trailing entry is removed

#### Scenario: Repeated shortcut toggles sort off
- **WHEN** a column is already sorted with the same kind and direction requested by `s`, `S`, `a`, `A`, `#`, or `@`
- **THEN** pressing that shortcut again removes that column from the sort list

#### Scenario: Column sort ascending command
- **WHEN** a user presses `csk`
- **THEN** the current column becomes the primary ascending sort key using numeric sort for number-family columns and lexical sort for all other columns

#### Scenario: Column sort descending command
- **WHEN** a user presses `csj`
- **THEN** the current column becomes the primary descending sort key using numeric sort for number-family columns and lexical sort for all other columns

#### Scenario: Column sort clear command
- **WHEN** a user presses `csx`
- **THEN** the current column is removed from the sort list without changing the remaining sort key order

#### Scenario: Sorted header indicators
- **WHEN** a visible column participates in the sort list
- **THEN** its header displays `▲` for ascending sort or `▼` for descending sort

### Requirement: Search traversal
The system SHALL preserve current forward and reverse search traversal results, including wraparound through rows and columns, without mutating table row or cell order during traversal.

#### Scenario: Next search result
- **WHEN** a search string is active and the user presses `n`
- **THEN** the cursor moves to the next matching cell after the current cell, wrapping when needed

#### Scenario: Previous search result
- **WHEN** a search string is active and the user presses `N`
- **THEN** the cursor moves to the previous matching cell, wrapping when needed

#### Scenario: Reverse search preserves table order
- **WHEN** a user presses `N` to search backward
- **THEN** the table row order and cell order remain unchanged after the search completes

### Requirement: Search match values
The system SHALL match search queries against both raw cell values and saved-view-rendered cell values.

#### Scenario: Search matches raw value
- **WHEN** a saved view renders raw value `1000` as `1,000` and the user searches for `1000`
- **THEN** the cell is included in search traversal results

#### Scenario: Search matches rendered value
- **WHEN** a saved view renders raw value `1000` as `1,000` and the user searches for `1,000`
- **THEN** the cell is included in search traversal results

#### Scenario: Search highlights matching cell
- **WHEN** a search query matches either the raw value or rendered value of a cell
- **THEN** search traversal highlights that cell regardless of which representation matched

### Requirement: Column visibility controls
The system SHALL support composable column show and hide commands under the `c` prefix, using `h` for hide, `H` for show, and directional suffixes.

#### Scenario: Hide current column
- **WHEN** a user presses `chj` or `chk`
- **THEN** the current column is hidden and the cursor moves to the nearest visible column when possible

#### Scenario: Hide columns to the right
- **WHEN** a user presses `10chl`
- **THEN** the system hides up to 10 visible columns to the right of the current column, nearest first

#### Scenario: Hide columns to the left
- **WHEN** a user presses `chh`
- **THEN** the system hides one visible column to the left of the current column when one exists

#### Scenario: Show hidden columns to the left
- **WHEN** a user presses `cHh`
- **THEN** the system shows the nearest hidden column immediately adjacent to the left of the current column in source order when one exists

#### Scenario: Show hidden columns to the right
- **WHEN** a user presses `5cHl`
- **THEN** the system shows up to 5 hidden columns immediately adjacent to the right of the current column in source order, nearest first

#### Scenario: Prevent hiding every column
- **WHEN** a column hide command would hide the last visible column
- **THEN** the viewer leaves at least one column visible and reports the condition through the footer message line

#### Scenario: Hidden column header indicator
- **WHEN** one or more hidden source columns exist between visible headers or beyond a visible edge
- **THEN** the header row displays a `|` indicator at that boundary

### Requirement: Sort persistence in saved views
The system SHALL serialize active source sort and view sort as separate ordered lists under `source.sort` and `view.sort`.

#### Scenario: Persist source sort
- **WHEN** a source sort is active and the user opens the saved-view modal
- **THEN** generated YAML includes its source column, direction, and source-supported kind under `source.sort`

#### Scenario: Persist view sort
- **WHEN** a view sort is active and the user opens the saved-view modal
- **THEN** generated YAML includes its source column, direction, and canonical local kind under `view.sort`

#### Scenario: Restore layered sort
- **WHEN** a saved view contains both lists and their source columns exist
- **THEN** source sort is applied before the source limit and view sort is applied to the resulting rows

#### Scenario: Search is not persisted
- **WHEN** a search query is active and the user opens the saved-view modal
- **THEN** generated YAML omits the search query

### Requirement: Skip-to-change operations
The system SHALL support skipping to the next or previous change in row or column value using `[`, `]`, `{`, and `}` with optional numeric modifiers.

#### Scenario: Skip to next row value change
- **WHEN** a user presses `]`
- **THEN** the cursor moves downward in the current column to the next row whose value differs from the starting cell

### Requirement: Clipboard operation
The system SHALL include clipboard support in default builds. It SHALL support yanking the rendered current cell contents with `y` and the raw current cell contents with `Y` when compiled with clipboard support, and SHALL fail non-fatally when clipboard support is disabled or unavailable.

#### Scenario: Default installation includes clipboard
- **WHEN** Tview is built with default Cargo features or installed from a standard release archive
- **THEN** clipboard support is enabled without additional feature flags

#### Scenario: Clipboard enabled rendered yank
- **WHEN** clipboard support is enabled and the user presses `y`
- **THEN** the rendered current cell contents are copied to the system clipboard

#### Scenario: Clipboard enabled raw yank
- **WHEN** clipboard support is enabled and the user presses `Y`
- **THEN** the raw current cell contents are copied to the system clipboard

#### Scenario: Clipboard unavailable
- **WHEN** clipboard support is disabled or unavailable and the user presses `y` or `Y`
- **THEN** the viewer continues running without corrupting state

### Requirement: Source and View configuration modals
The interactive runtime SHALL provide separate Source and View configuration modals. Source configuration SHALL own construction of the bounded source result and MAY vary by adapter capability; View configuration SHALL summarize source-neutral local transformation and provide common local actions without changing the source result.

#### Scenario: Open Source configuration
- **WHEN** the user opens Source configuration
- **THEN** it shows source identity, source limit and result extent, source filters, source sort, capability status, and query provenance when the adapter provides it

#### Scenario: SQLite Source configuration
- **WHEN** the active source is SQLite
- **THEN** Source configuration offers supported SQLite-native predicates and sorting and shows the generated SQL representation

#### Scenario: File Source configuration
- **WHEN** the active source is delimited, JSON, or NDJSON
- **THEN** Source configuration offers supported logical-record filters, omits SQL, and explains when source sorting is unavailable

#### Scenario: Apply Source draft
- **WHEN** the user changes multiple source filters, sort keys, or the limit and confirms Apply
- **THEN** the complete draft is validated and starts at most one asynchronous source-query replacement

#### Scenario: Cancel Source draft
- **WHEN** the user cancels Source configuration
- **THEN** no draft source operation is applied and the active result remains unchanged

#### Scenario: Source replacement is pending
- **WHEN** an applied source draft is still executing
- **THEN** the previous successful result remains visible and Source configuration exposes pending or failure state

#### Scenario: Open View configuration
- **WHEN** the user opens View configuration for any supported source
- **THEN** it summarizes the active view filters, ordered sort keys, and view-wide null placement and offers actions to clear operations, toggle null placement, or open Column Info

#### Scenario: Clear View operations
- **WHEN** the user clears filters and sorts through View configuration
- **THEN** the local view is recomputed over the fixed active source result without re-querying the source

#### Scenario: Toggle View null placement
- **WHEN** the user toggles view-wide null placement through View configuration
- **THEN** active local sorts use the new policy without re-querying or expanding the source result

#### Scenario: Same column in both scopes
- **WHEN** a column participates in a source filter or sort
- **THEN** the user may independently configure a view filter or sort for that column

#### Scenario: Quick filter and sort commands
- **WHEN** the user invokes an existing current-column filter prompt or sort shortcut
- **THEN** it changes View configuration and never implicitly starts a source query

#### Scenario: Column Info applies a View filter
- **WHEN** the user adds, edits, or clears a filter for the current column through Column Info
- **THEN** the corresponding `ViewFilter` changes and the View Configuration summary reflects that change

#### Scenario: Column Info applies a View sort
- **WHEN** the user changes the current column's sort through Column Info
- **THEN** the corresponding `ViewSort` changes using established View sort precedence and the View Configuration summary reflects that change

#### Scenario: Open Column Info from View configuration
- **WHEN** the user opens Column Info from View configuration
- **THEN** Column Info displays and edits the current column's active View operation state rather than maintaining a separate copy

#### Scenario: Source operation in Column Info
- **WHEN** the current column participates in a source filter or source sort
- **THEN** Column Info may summarize that source operation but requires Source Configuration to edit it

#### Scenario: Saved View modal remains distinct
- **WHEN** the user opens the Saved View YAML modal
- **THEN** it serializes current runtime Source and View state without replacing either runtime configuration modal

### Requirement: SQLite source SQL output
When a SQLite source query is active, the user SHALL be able to inspect and copy the final source SQL together with its bound values or an equivalent executable SQL representation.

#### Scenario: Source operations change
- **WHEN** a source filter, source sort, table selection, or source limit changes
- **THEN** the displayed SQL is regenerated from the complete active source query

#### Scenario: View operations change
- **WHEN** only a view filter, view sort, or search changes
- **THEN** the source SQL remains unchanged and the output identifies those operations as local view behavior

#### Scenario: Identifier and value safety
- **WHEN** table names, column names, or filter values require quoting
- **THEN** identifiers are quoted by the SQL renderer and values remain bound parameters in execution metadata

### Requirement: Native base query composition
For a query-native source, Source Configuration SHALL compose supported source filters, source sorting, and the hard source limit around the native base query using adapter-native semantics. It SHALL NOT parse or rewrite a complete native query using another source's grammar.

#### Scenario: Compose ES|QL source operations
- **WHEN** an Elasticsearch source filter, sort, or limit is applied
- **THEN** the adapter appends a parameterized ES|QL stage in the defined source-operation order

#### Scenario: Compose SQLite source operations
- **WHEN** a SQLite native query receives a source filter, sort, or limit
- **THEN** the adapter treats the row-producing SQL as a bounded derived-table input and applies parameterized outer SQL

#### Scenario: Native query already contains limiting behavior
- **WHEN** a user-supplied native query contains its own `LIMIT` or equivalent stage
- **THEN** that behavior remains part of the opaque base query and Tview still applies its final hard source-result limit

#### Scenario: Unsupported composition
- **WHEN** an adapter cannot represent a requested source operation safely over the native base query
- **THEN** Source Configuration rejects the operation without materializing an unbounded remote or local source

#### Scenario: Local view remains separate
- **WHEN** a view filter, view sort, presentation change, or search is applied
- **THEN** it operates only over the fixed active native-query result and does not modify or re-execute the native source query

### Requirement: Elasticsearch Source configuration
When the active source is Elasticsearch, Source Configuration SHALL expose the endpoint's safe identity, selected target or native query, mappings-backed result fields when available, ES|QL-native filter/sort capabilities, limit, partial/extent state, and query provenance.

#### Scenario: Mapping-aware configuration
- **WHEN** Elasticsearch was opened from a selected index or data stream
- **THEN** Source Configuration can select fields from its mapping and field-capability catalog

#### Scenario: Query-only configuration
- **WHEN** Elasticsearch was opened from a complete ES|QL query without `source.table`
- **THEN** Source Configuration uses the active result columns and does not require a reconstructed mapping target

#### Scenario: Apply ES|QL draft
- **WHEN** the user applies a valid Elasticsearch source draft
- **THEN** at most one asynchronous replacement starts and the prior result remains visible until success

### Requirement: Native source query output
When a query-native source is active, the user SHALL be able to inspect and copy its final native query, language, and bound parameters or an equivalent safe executable representation.

#### Scenario: SQLite query artifact
- **WHEN** the active query language is SQL
- **THEN** the query UI labels and displays SQL rather than assuming the artifact was application-generated

#### Scenario: Elasticsearch query artifact
- **WHEN** the active query language is ES|QL
- **THEN** the query UI labels and displays ES|QL and its value or identifier parameters

#### Scenario: Source operations change
- **WHEN** a native base query, source filter, source sort, target selection, or source limit changes
- **THEN** the displayed artifact is regenerated from the complete active source request

#### Scenario: View operations change
- **WHEN** only a view filter, view sort, presentation option, or search changes
- **THEN** the native query artifact remains unchanged and the output identifies those operations as local view behavior

#### Scenario: Secret redaction
- **WHEN** native query information is displayed, copied, logged, or included in an error
- **THEN** transport credentials and authorization headers are absent
