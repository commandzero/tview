# Spec Delta

## ADDED Requirements

### Requirement: Invocation-consistent saved-view snapshot
The system SHALL use one selected validated saved-view snapshot for source configuration and presentation binding within an invocation. Changes to saved-view files after selection SHALL NOT alter that invocation's settings. A fresh invocation SHALL discover current files; data reload SHALL preserve active runtime view settings rather than reselecting or rereading saved-view YAML.

#### Scenario: Saved file changes during source opening
- **WHEN** a selected saved-view file is changed or removed after selection but before source opening finishes
- **THEN** source options and presentation settings both come from the originally selected validated contents, and the selected canonical name and authoring target remain consistent

#### Scenario: Fresh invocation observes edited files
- **WHEN** a saved-view file changes after an earlier invocation selected it and a new invocation opens the input
- **THEN** the new invocation discovers, validates, and selects the current files without reusing a process-wide cached snapshot

#### Scenario: Reload preserves live settings
- **WHEN** the user changes view sorting, filters, or presentation in the TUI, edits the saved YAML externally, and reloads data
- **THEN** reload restores the active runtime view settings under existing identity-restoration rules without applying the external YAML edits

#### Scenario: CLI source overrides do not replace presentation
- **WHEN** explicit CLI source options override values in the selected snapshot
- **THEN** the CLI source values are applied before opening, and presentation still binds from that same snapshot against the resulting schema

### Requirement: Equivalent initial and delayed saved operation binding
The system SHALL interpret saved view column metadata, ordered sorts, and filters equivalently for initially available and later discovered canonical columns. Metadata SHALL be available before type-aware sorting, null-placement resolution, and raw/rendered filtering. Successful delayed binding SHALL NOT duplicate active filters or reset unrelated live view settings. `--sorted false` SHALL disable immediate and delayed saved view sorts only.

#### Scenario: Initial and late canonical columns agree
- **WHEN** equivalent source rows and saved canonical column metadata, sort keys, and filters are supplied once with complete schema and once with provisional schema followed by discovery of the same columns
- **THEN** after equivalent schema and data are available the visible rows, sort precedence, formatting, visibility, type-aware comparisons, and null placement agree

#### Scenario: Late type and null override precede sorting
- **WHEN** a pending canonical column arrives with saved type metadata, a column null-placement override, and a type-aware saved sort
- **THEN** sorting uses that column's saved type and null override rather than pre-binding inference or the view default

#### Scenario: Repeated schema progress is idempotent
- **WHEN** further indexing or repeated schema-completion notifications occur after a saved filter successfully binds
- **THEN** the filter remains active exactly once and unrelated runtime settings are not reapplied from the snapshot

#### Scenario: Saved sort suppression leaves source ordering intact
- **WHEN** direct table output uses `--sorted false` and the selected saved view defines source ordering, immediate or pending view sorts, filters, and formatting
- **THEN** no saved view sort activates or causes scanning, source ordering remains intact, and filters and presentation remain active

#### Scenario: Raw and rendered filter semantics remain compatible
- **WHEN** an initially available or late canonical column has display formatting and a saved text or regex filter
- **THEN** the filter retains existing matching against raw and rendered values, and binding does not mutate raw values or reinterpret a view filter as a source predicate

## MODIFIED Requirements

### Requirement: Saved view discovery
When compiled with the `saved-views` feature, the system SHALL discover user-defined saved view files from `$XDG_CONFIG_HOME/tview/views`, or `~/.config/tview/views` when `XDG_CONFIG_HOME` is unset, including files ending in `.yml` or `.yaml` recursively within view bundles. Canonical names SHALL remain filename stems across bundles, with deterministic duplicate precedence.

#### Scenario: Discover views from config directory
- **WHEN** a user opens a file and saved views exist under `~/.config/tview/views`
- **THEN** the system loads candidate `.yml` and `.yaml` view files before initializing the table view

#### Scenario: Missing view directory
- **WHEN** the saved view directory does not exist
- **THEN** the system opens the input with existing default behavior

#### Scenario: Duplicate yml and yaml stems
- **WHEN** both `cat-shards.yml` and `cat-shards.yaml` exist in the saved view directory
- **THEN** the system loads `cat-shards.yml`, ignores `cat-shards.yaml`, logs the conflict, and records a TUI warning

#### Scenario: Saved views feature disabled
- **WHEN** the binary is compiled without the `saved-views` feature
- **THEN** the system does not discover or apply saved views

#### Scenario: Recursive bundle discovery
- **WHEN** a valid saved view exists in a nested directory beneath the configured views directory
- **THEN** it participates in normal forced-name and filename matching using its filename stem, without adding bundle prefixes or inheritance

#### Scenario: Duplicate names across bundles
- **WHEN** multiple bundles contain the same canonical filename-stem name under platform case rules
- **THEN** `.yml` takes precedence over `.yaml`, otherwise the lexically first file path wins, and the conflict produces one non-fatal discovery warning

### Requirement: Saved view selection overrides
When compiled with the `saved-views` feature, the system SHALL apply matching saved views automatically by default and SHALL provide CLI overrides to force a saved view by canonical name or disable saved views for the invocation. Disabled saved views SHALL skip discovery, binding, and saved-view authoring. Forced missing selection SHALL fail before source opening.

#### Scenario: Automatic view selection
- **WHEN** a user opens an input whose basename matches a valid saved view and no saved view override flag is present
- **THEN** the system applies the matching saved view automatically

#### Scenario: Force saved view by name
- **WHEN** a user runs `tview --view cat-shards cat_nodes.txt` and `cat-shards.yml` exists
- **THEN** the system applies that saved view even if the input basename does not match the view's `filenames`

#### Scenario: Force saved view with extension
- **WHEN** a user runs `tview --view cat-shards.yaml cat_nodes.txt` and `cat-shards.yml` exists
- **THEN** the system normalizes away the `.yaml` extension and applies the `cat-shards` saved view

#### Scenario: Disable saved views
- **WHEN** a user runs `tview --no-view cat_shards.txt`
- **THEN** the system opens the input without discovering or applying saved views

#### Scenario: Missing forced view
- **WHEN** a user runs `tview --view missing data.txt` and no saved view has that name
- **THEN** the system reports a clear CLI error and does not start the viewer

#### Scenario: Missing forced view does not open the source
- **WHEN** a forced saved view is missing and the input source would otherwise be opened or queried
- **THEN** selection fails before opening or querying that source and batch stdout remains empty

#### Scenario: Disabled invocation does not inspect or author views
- **WHEN** the user invokes `--no-view` with malformed saved files present
- **THEN** no discovery warnings are produced, no saved settings are applied, and saved-view authoring and saving remain disabled

### Requirement: Column matching
The system SHALL apply column configuration sparsely using stable canonical source identity where available, with compatible header-label matching for delimited sources and unambiguous fallback matching for structured sources. Saved view sort and filter references SHALL follow the same identity and ambiguity rules; an ambiguous structured label SHALL NOT fall back to delimited header matching.

#### Scenario: Exact column key wins
- **WHEN** `columns` contains both `count` and `*count` and a compatible delimited table has a `Count` header
- **THEN** the system applies the exact `count` configuration to that column

#### Scenario: Wildcard column key
- **WHEN** `columns` contains `*count` and a compatible delimited table has `docs_count` and `store_count` headers
- **THEN** the system applies the wildcard configuration to both matching columns unless an exact configuration also exists

#### Scenario: Exact JSON pointer
- **WHEN** a JSON saved view configures canonical pointer `/_source/user/id`
- **THEN** the system matches that source column case-sensitively regardless of its compact display label

#### Scenario: Unambiguous JSON display label
- **WHEN** a JSON saved view uses a display label that identifies exactly one loaded column and no canonical source key matches
- **THEN** the system may apply that configuration as a compatibility fallback

#### Scenario: Ambiguous JSON display label
- **WHEN** a configured display label could refer to more than one structured source column
- **THEN** the system does not guess and records a non-fatal warning that recommends canonical source pointers

#### Scenario: Missing configured column
- **WHEN** a saved view configures a column key that matches no loaded column after a complete schema scan
- **THEN** the system ignores that column configuration and records a non-fatal warning

#### Scenario: Ambiguous operation label is not guessed
- **WHEN** a saved view sort or filter references a structured display label shared by multiple columns
- **THEN** the operation is not applied to an arbitrary column and one non-fatal ambiguity warning recommends a canonical source key

#### Scenario: Canonical operation reference wins
- **WHEN** a saved sort or filter uses a canonical JSON pointer or relational occurrence key and rendered labels are ambiguous or overridden
- **THEN** the operation binds to the identified source column without using the rendered label as identity

### Requirement: Pending late-column configuration
The system SHALL retain valid canonical column configuration and unresolved canonical view sort and filter references against a provisional schema until discovered or schema completion. Delayed binding SHALL preserve existing filter interpretation and numeric availability rules. Missing references and operations still unavailable or invalid at completion SHALL be ignored with one non-fatal warning per affected item.

#### Scenario: Configured column arrives late
- **WHEN** a saved view configures a canonical JSON pointer absent from the bounded initial scan and that pointer is discovered during later indexing
- **THEN** the system applies the pending configuration when the column is appended

#### Scenario: Configured column never arrives
- **WHEN** schema discovery reaches the selected table's end without finding a pending canonical column
- **THEN** the system records the normal non-fatal missing-column warning

#### Scenario: Pending filter waits across provisional schema
- **WHEN** a saved canonical filter column is absent during several provisional schema updates and later appears
- **THEN** the filter remains pending without a premature missing-column warning, binds under normal filter rules when available, and filters the bounded source result without requesting replacement source rows

#### Scenario: Deferred numeric filter remains temporarily unavailable
- **WHEN** a pending saved numeric filter's column appears but its numeric profile is not yet available under existing filter rules
- **THEN** the filter can remain pending until normal preparation supplies the required profile, without changing numeric comparison semantics or reporting it as a missing column

#### Scenario: Completion without appended columns
- **WHEN** schema completion arrives without adding any columns and saved columns or operations are still pending
- **THEN** each unresolved item is finalized and produces its normal non-fatal diagnostic once

#### Scenario: Missing and invalid items are distinguished
- **WHEN** completion finds both an absent canonical filter column and a present column whose saved filter cannot be applied under normal filter rules
- **THEN** diagnostics distinguish the missing reference from the invalid or unavailable operation and neither item remains indefinitely pending

### Requirement: Non-fatal saved view failures
The system SHALL treat saved view loading, validation, matching, and application failures as non-fatal unless the user explicitly requests a missing view through `--view`. Initial and delayed binding SHALL use consistent diagnostics, delivered once per affected item through existing TUI warning and stderr routes, never stdout. This SHALL NOT suppress source-opening, query, ingestion, or output failures.

#### Scenario: Bad view does not block data
- **WHEN** one or more saved view files are malformed
- **THEN** the system logs the failure, records a TUI warning, and still opens the requested input file if the input itself can be loaded

#### Scenario: No matching view
- **WHEN** no saved view matches the opened input
- **THEN** the system opens the input with existing default behavior and does not report an error

#### Scenario: Early and late warnings agree
- **WHEN** the same saved item fails binding in an initially complete schema or after provisional schema completion
- **THEN** both paths report the same reason and item identity once without preventing other valid saved settings from applying

#### Scenario: Batch preparation surfaces late warnings
- **WHEN** direct output discovers missing or invalid saved binding items during final traversal or required preview preparation
- **THEN** all such warnings are emitted once on stderr before output emission and never inserted into table, JSON, or JSONL stdout

#### Scenario: Interactive warnings are not overwritten
- **WHEN** one schema-completion event finalizes both missing column metadata and missing or invalid saved operations
- **THEN** all warnings are retained for normal interactive diagnostic delivery rather than one status message replacing another, and later polling does not repeat them

#### Scenario: Binding does not hide data errors
- **WHEN** source opening or required output preparation fails while saved settings are being applied
- **THEN** normal error handling remains in force and no partial batch stdout is emitted
