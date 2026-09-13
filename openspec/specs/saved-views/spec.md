## Purpose

Define user-defined saved view configuration files, matching, validation, application, serialization, and writing behavior.

## Requirements

### Requirement: Saved view discovery
When compiled with the `saved-views` feature, the system SHALL discover user-defined saved view files from `$XDG_CONFIG_HOME/tview/views`, or `~/.config/tview/views` when `XDG_CONFIG_HOME` is unset, including files ending in `.yml` or `.yaml`.

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

### Requirement: Saved view schema
The system SHALL ship and document a schema for saved-view YAML with `name` and `filenames` at the document root, source-opening and source-query configuration under `source`, and source-independent presentation and local-operation configuration under `view`. `source` SHALL support `format`, `json_path`, `object_mode`, `table`, `schema_scan`, `limit`, `filters`, and `sort`. `view` SHALL support `locale`, `nulls`, `columns`, `filters`, and `sort`, including the existing column labels, visibility, type aliases, formatting, widths, alignment, conditional colors, numeric masks, and null-placement overrides.

#### Scenario: Editor validation
- **WHEN** a user configures a YAML language server with the shipped schema
- **THEN** a valid saved view with root identity fields and nested `source` and `view` sections validates without schema errors

#### Scenario: Invalid enum value
- **WHEN** a saved view sets `source.format`, `source.object_mode`, `source.schema_scan`, `view.nulls`, a column `nulls`, a column `type`, or a column `format` to an unsupported value
- **THEN** schema validation reports the field as invalid

#### Scenario: Invalid JSON pointer
- **WHEN** a saved view supplies a `source.json_path` that is not a valid RFC 6901 JSON Pointer
- **THEN** semantic validation records a non-fatal saved-view warning and does not apply the invalid path

#### Scenario: Legacy operation field at the root
- **WHEN** a saved view places `format`, `json_path`, `object_mode`, `table`, `schema_scan`, `limit`, `locale`, `columns`, `filters`, or `sort` at the document root
- **THEN** schema validation rejects the misplaced field and directs the user to `source` or `view`

### Requirement: Saved view validation
The system SHALL validate saved view files structurally and semantically before applying them, including validation of nested source and view configuration.

#### Scenario: Invalid YAML file
- **WHEN** a saved view file contains invalid YAML
- **THEN** the system ignores that view file, records a non-fatal warning, and continues opening the input

#### Scenario: Invalid regex pattern
- **WHEN** a saved view filename pattern or `view.filters` regex is invalid
- **THEN** the system ignores the invalid item, records a non-fatal warning, and continues evaluating other valid configuration

#### Scenario: Invalid source filter
- **WHEN** a saved view contains a `source.filters` predicate unsupported by the selected source
- **THEN** the system reports that source operation as unavailable and does not reinterpret it as a view filter

#### Scenario: Invalid numeric mask
- **WHEN** a number column uses `format: mask` with a mask outside the supported mask grammar
- **THEN** the system ignores the mask for that column, records a non-fatal warning, and falls back to plain display for that column

#### Scenario: Invalid POSIX locale
- **WHEN** a saved view sets an unsupported `view.locale`
- **THEN** the system logs the invalid locale, records a TUI warning, and falls back to `en_US`

#### Scenario: One view per file
- **WHEN** a saved view file is loaded
- **THEN** the system treats the file as exactly one saved view whose canonical name is the file stem

### Requirement: Filename matching
The system SHALL match saved views against the opened input basename using exact, glob, and regex filename patterns while following platform filename case behavior.

#### Scenario: Exact filename match
- **WHEN** a saved view includes `cat_shards.txt` in `filenames` and the opened input basename is `cat_shards.txt`
- **THEN** the saved view matches the input as an exact match

#### Scenario: Glob filename match
- **WHEN** a saved view includes `*shards*` in `filenames` and the opened input basename is `cat_shards.txt`
- **THEN** the saved view matches the input as a glob match

#### Scenario: Regex filename match
- **WHEN** a saved view includes `^cat_.*txt$` in `filenames` and the opened input basename is `cat_shards.txt`
- **THEN** the saved view matches the input as a regex match

#### Scenario: Multiple matching views
- **WHEN** more than one saved view matches the opened input
- **THEN** the system chooses a deterministic view using exact matches before glob matches before regex matches, then lexicographic view file path order within the same match rank

#### Scenario: Parent directory ignored
- **WHEN** a saved view filename pattern matches a parent directory name but not the opened input basename
- **THEN** the saved view does not match the input

#### Scenario: Platform case behavior
- **WHEN** a saved view filename pattern differs from the opened input basename only by letter case
- **THEN** the system matches or rejects it according to the platform filename case behavior

### Requirement: Saved view selection overrides
When compiled with the `saved-views` feature, the system SHALL apply matching saved views automatically by default and SHALL provide CLI overrides to force a saved view by canonical name or disable saved views for the invocation.

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

### Requirement: Saved source options
A saved view SHALL apply source-opening and source-query options from `source` before constructing the active source result. Explicit CLI source options SHALL override matching saved values for that invocation.

#### Scenario: Saved JSON starting path
- **WHEN** a matching saved view sets `source.format: json` and `source.json_path: /hits/hits`
- **THEN** source opening selects that embedded JSON value before constructing table columns or rows

#### Scenario: Saved SQLite table
- **WHEN** a matching saved view sets `source.format: sqlite` and `source.table: users`
- **THEN** source opening selects the `users` ordinary table or compatible ordinary view and does not display the table-selection modal

#### Scenario: Saved full schema scan
- **WHEN** a matching saved view sets `source.schema_scan: full`
- **THEN** JSON schema discovery scans all selected rows before the table schema is marked complete

#### Scenario: Saved keyed-object interpretation
- **WHEN** a matching saved view sets `source.format: json` and `source.object_mode: entries`
- **THEN** the selected JSON object's direct members become rows before column configuration is resolved

#### Scenario: Incompatible saved object mode
- **WHEN** a saved view sets an explicit `source.object_mode` for a source shape that cannot interpret an object or map
- **THEN** normal source-option validation reports the incompatibility and does not reinterpret the value as view configuration

#### Scenario: Saved source limit
- **WHEN** a matching saved view sets `source.limit: 2500`
- **THEN** at most 2500 rows are requested for the active source result

#### Scenario: Default source limit
- **WHEN** a SQLite saved view omits `source.limit`
- **THEN** the active SQLite source query uses the default limit of 1000 rows

#### Scenario: CLI source option precedence
- **WHEN** both a saved view and an explicit CLI argument provide the same source option
- **THEN** the explicit CLI value takes precedence for that invocation

### Requirement: Column matching
The system SHALL apply column configuration sparsely using stable canonical source identity where available, with compatible header-label matching for delimited sources and unambiguous fallback matching for structured sources.

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

### Requirement: Pending late-column configuration
The system SHALL retain valid canonical column configuration that does not match the initial provisional schema until the schema becomes complete or the column is discovered.

#### Scenario: Configured column arrives late
- **WHEN** a saved view configures a canonical JSON pointer absent from the bounded initial scan and that pointer is discovered during later indexing
- **THEN** the system applies the pending configuration when the column is appended

#### Scenario: Configured column never arrives
- **WHEN** schema discovery reaches the selected table's end without finding a pending canonical column
- **THEN** the system records the normal non-fatal missing-column warning

### Requirement: Column display-label override
A saved view SHALL allow a column to override its rendered display label without changing source identity or raw data.

#### Scenario: JSON column label
- **WHEN** canonical JSON column `/_source/user/email` sets `label: User email`
- **THEN** the fixed header and column information use `User email` as the display label while saved-view resolution retains the canonical pointer

#### Scenario: Duplicate label override
- **WHEN** label overrides create duplicate rendered labels
- **THEN** stable source identity remains distinct and ambiguous label-based configuration fallback is disabled for those columns

### Requirement: Column type metadata
The system SHALL support string, number, and boolean column type families with subtype aliases for text, date, float, integer, semantic version, IP address, character boolean, bit boolean, and word boolean.

#### Scenario: Broad type aliases
- **WHEN** a column sets `type: string`, `type: number`, or `type: boolean`
- **THEN** the system maps the value to the default subtype for that type family

#### Scenario: Subtype aliases
- **WHEN** a column sets `type: text`, `type: date`, `type: integer`, `type: semver`, `type: ip`, `type: char`, `type: bit`, or `type: word`
- **THEN** the system maps the value to the corresponding typed column subtype

#### Scenario: Type-aware sort
- **WHEN** a saved view gives a column an explicit type and the user sorts that column
- **THEN** the system uses the saved type metadata to select the appropriate comparison semantics when that subtype is implemented

#### Scenario: ISO 8601 date type
- **WHEN** a column sets `type: date`
- **THEN** the system parses ISO 8601 date/time values for chronological sorting where values parse successfully

#### Scenario: IP address type
- **WHEN** a column sets `type: ip`
- **THEN** the system treats the column as a string-family IP subtype and supports IPv4 and IPv6 parsing for IP-aware operations

#### Scenario: Loose semantic version type
- **WHEN** a column sets `type: semver`
- **THEN** the system parses values accepted by the selected SemVer parser, including loose version forms the parser supports

#### Scenario: Boolean subtype values
- **WHEN** a column sets `type: word`, `type: bit`, or `type: char`
- **THEN** the system recognizes `true`/`false` and `yes`/`no` for word booleans, `1`/`0` for bit booleans, and `y`/`n` for character booleans

### Requirement: Display formatting
The system SHALL apply display formatting from `view` and `view.columns` to rendered cell values without changing raw cell values.

#### Scenario: Plain format
- **WHEN** a column uses `format: plain`
- **THEN** the system renders cell values without display transformation

#### Scenario: Locale number format
- **WHEN** a number column uses `format: locale` and the saved view does not set `view.locale`
- **THEN** the system renders numeric values using the POSIX-style system locale, falling back to `en_US` if locale detection or lookup fails

#### Scenario: View locale override
- **WHEN** a saved view sets `view.locale: en_US` and a number column uses `format: locale`
- **THEN** the system renders locale-formatted values using the saved view locale

#### Scenario: Numeric mask format
- **WHEN** a number column uses `format: mask` and `mask: "0.00"`
- **THEN** the system renders numeric values with two decimal places

#### Scenario: Numeric mask overrides locale
- **WHEN** a saved view sets `view.locale: de_DE` and a number column uses `format: mask` with `mask: "#,##0.00"`
- **THEN** the system renders the value according to the mask grammar rather than substituting locale-specific separators

#### Scenario: String case format
- **WHEN** a string column uses `format: uppercase` or `format: lowercase`
- **THEN** the system renders that column's cell values using the requested case transformation

#### Scenario: Raw and rendered matching
- **WHEN** formatting changes the rendered value for a cell
- **THEN** search and `view.filters` can match either the raw cell value or the rendered cell value

### Requirement: Column width and alignment metadata
The system SHALL use saved column width and alignment metadata to initialize the table layout while preserving existing interactive layout controls.

#### Scenario: Numeric width
- **WHEN** a column sets `width: 20`
- **THEN** the system initializes that column width to 20 display characters subject to existing terminal constraints

#### Scenario: Header width
- **WHEN** a column sets `width: header`
- **THEN** the system initializes that column width from the display width of the header

#### Scenario: Content width
- **WHEN** a column sets `width: content`
- **THEN** the system initializes that column width from the widest materialized content value in that column

#### Scenario: Alignment override
- **WHEN** a number column sets `align: left`
- **THEN** the system left-aligns rendered data cells for that column instead of using the numeric default

#### Scenario: Interactive width changes still work
- **WHEN** a saved view initializes column widths and the user presses existing width adjustment keys
- **THEN** the system adjusts widths using the existing interactive behavior

### Requirement: Saved null-placement policy
A saved view SHALL accept `view.nulls: first|last` and `view.columns.<key>.nulls: first|last`, with column configuration overriding the view default and omission using the built-in `last` default.

#### Scenario: View default
- **WHEN** a saved view sets `view.nulls: first`
- **THEN** every view-sorted column without an explicit column policy resolves to nulls first

#### Scenario: Column override
- **WHEN** a saved view sets `view.nulls: first` and `view.columns.deleted_at.nulls: last`
- **THEN** view sorting `deleted_at` places nulls last while other columns inherit nulls first

#### Scenario: Column inherits view policy
- **WHEN** a column omits `nulls`
- **THEN** its configuration retains inheritance so a later view-default change affects it

#### Scenario: Pending structured column policy
- **WHEN** a provisional structured schema has pending canonical column configuration with a `nulls` override
- **THEN** the override is applied when that column is discovered and is used by subsequent view sorting

#### Scenario: Serialize null placement
- **WHEN** the view or a column has an explicit null-placement policy
- **THEN** generated YAML writes it under `view.nulls` or `view.columns.<key>.nulls` and omits it for an inheriting column

### Requirement: Column visibility metadata
The system SHALL use saved column visibility metadata to initialize which columns are shown in the table viewport.

#### Scenario: Visible omitted defaults to true
- **WHEN** a saved view configures a column without `visible`
- **THEN** the system treats that column as visible

#### Scenario: Hidden column from saved view
- **WHEN** a saved view configures a column with `visible: false`
- **THEN** the system keeps the column in the table model but excludes it from viewport rendering and horizontal navigation

#### Scenario: Hidden column remains available to data operations
- **WHEN** a saved view hides a column
- **THEN** the system preserves that column's raw values for reload, sorting metadata, active filters, and future show-column commands

### Requirement: Saved view serialization
The system SHALL serialize the current runtime configuration as saved-view YAML conforming to the nested schema, with derived source-query output excluded from persisted configuration.

#### Scenario: Serialize loaded view
- **WHEN** a saved view was loaded from disk and the user opens the view modal
- **THEN** the displayed YAML reflects the current runtime source and view configuration and identifies the loaded saved-view filename

#### Scenario: Serialize new view placeholder
- **WHEN** no saved view was loaded and the user opens the view modal for `foo.bar.csv`
- **THEN** the displayed target filename is `foo.bar.yml` under the saved views directory

#### Scenario: Serialize interactive column changes
- **WHEN** the user changes column widths or visibility before opening the view modal
- **THEN** the YAML includes affected columns under `view.columns`

#### Scenario: Serialize only changed column state
- **WHEN** a column has no saved metadata and no interactive view-state changes
- **THEN** the YAML omits that column from `view.columns`

#### Scenario: Serialize current filename only
- **WHEN** a saved view loaded with multiple filename patterns is displayed in the view modal
- **THEN** the generated YAML includes only the current input filename in root `filenames`

#### Scenario: Serialize default locale omission
- **WHEN** locale formatting uses auto-detected or default behavior
- **THEN** the generated YAML omits `view.locale`

#### Scenario: Serialize placeholder name
- **WHEN** no saved view was loaded for `cat_shards.txt`
- **THEN** the generated YAML includes root `name: cat_shards`

#### Scenario: Serialize layered operations
- **WHEN** source filters, source sort, source limit, view filters, or view sort are active
- **THEN** the generated YAML writes them under their corresponding `source` or `view` section and excludes search state

#### Scenario: Serialize resolved object mode
- **WHEN** an object-capable source resolves automatic or explicit object interpretation to `record` or `entries`
- **THEN** generated YAML writes the resolved value under `source.object_mode`

#### Scenario: Derived SQL is not persisted
- **WHEN** a SQLite source exposes the SQL generated from saved source operations
- **THEN** serialization persists the structured source operations rather than a duplicated generated SQL string

### Requirement: Saved view writing
The system SHALL save the current runtime view configuration to `config_dir/tview/views` from the view modal.

#### Scenario: Save loaded view
- **WHEN** a view was loaded from `/home/user/.config/tview/views/cat-shards.yml` and the user saves from the view modal
- **THEN** the system writes the current view configuration atomically to that file after any required overwrite confirmation while preserving the header comment block and matching inline comments

#### Scenario: Save new placeholder view
- **WHEN** no view was loaded for `foo.bar.csv` and the user saves from the view modal
- **THEN** the system writes the current view configuration atomically to `~/.config/tview/views/foo.bar.yml`

#### Scenario: Create saved view directory on save
- **WHEN** the saved views directory does not exist and the user saves from the view modal
- **THEN** the system creates the directory and writes the saved view file

#### Scenario: Ask before overwrite
- **WHEN** the target saved view file already exists and the user saves from the view modal
- **THEN** the system asks for overwrite confirmation using `y` and `n` before replacing the file

#### Scenario: Decline overwrite
- **WHEN** the target saved view file already exists and the user declines overwrite confirmation
- **THEN** the system leaves the existing file unchanged and returns to the view modal

#### Scenario: Save failure
- **WHEN** writing the saved view file fails
- **THEN** the system logs the error, reports it through the modal or footer message line, keeps the modal open, and keeps the viewer running

#### Scenario: No-view disables saving
- **WHEN** the user invoked `tview --no-view data.csv`
- **THEN** saved view authoring and saving are disabled for that session

### Requirement: Non-fatal saved view failures
The system SHALL treat saved view loading, validation, matching, and application failures as non-fatal unless the user explicitly requests a missing view through `--view`.

#### Scenario: Bad view does not block data
- **WHEN** one or more saved view files are malformed
- **THEN** the system logs the failure, records a TUI warning, and still opens the requested input file if the input itself can be loaded

#### Scenario: No matching view
- **WHEN** no saved view matches the opened input
- **THEN** the system opens the input with existing default behavior and does not report an error

### Requirement: Column conditional color metadata
The system SHALL allow saved view column definitions to include conditional color formatting rules that apply to rendered cell styles without changing raw or rendered cell values.

#### Scenario: Conditional color field validates
- **WHEN** a saved view column defines valid `colors` rules using `gradient`, `match`, `range`, or `identifiers`
- **THEN** the saved view schema accepts the column definition

#### Scenario: Conditional color does not change values
- **WHEN** a conditional color rule matches a cell
- **THEN** sorting, filtering, searching, copying, and popup display continue to use the raw and rendered cell values without including style metadata

#### Scenario: Invalid conditional color is non fatal
- **WHEN** a saved view column defines an invalid conditional color rule
- **THEN** the system ignores that rule, records a non-fatal warning, and continues applying the rest of the saved view

### Requirement: Conditional color precedence
The system SHALL resolve multiple conditional color rules for a column deterministically using saved view order.

#### Scenario: First matching rule wins
- **WHEN** a cell matches more than one conditional color rule in the same column
- **THEN** the system applies the first matching rule in the column's `colors` list

#### Scenario: No matching rule
- **WHEN** a cell matches no conditional color rule
- **THEN** the cell uses the normal theme style for that row and selection state

#### Scenario: Selection preserves readability
- **WHEN** a conditionally colored cell is also the selected cell
- **THEN** the selected-cell theme background or modifier is preserved and the conditional color is applied only where it remains readable

### Requirement: Gradient conditional colors
The system SHALL support numerical `gradient` conditional colors with `mode: fixed` and `mode: auto`.

#### Scenario: Fixed gradient ranges
- **WHEN** a numeric column defines a fixed gradient with stop entries `0: green`, `50: yellow`, and `100: red`
- **THEN** values greater than or equal to `0` and less than `50` use the first stop color, values greater than or equal to `50` and less than `100` use the second stop color, and values greater than or equal to `100` use the final stop color

#### Scenario: Fixed gradient requires stops
- **WHEN** a fixed gradient omits user-defined numeric stop values
- **THEN** the system rejects that rule as invalid and records a non-fatal warning

#### Scenario: Auto gradient default steps
- **WHEN** a numeric column defines an auto gradient with two or more colors and no `steps`
- **THEN** the system distributes eight inclusive/exclusive buckets across the observed minimum and maximum parseable numeric values for that column

#### Scenario: Auto gradient custom steps
- **WHEN** a numeric column defines an auto gradient with `steps = 5`
- **THEN** the system distributes five inclusive/exclusive buckets across the observed minimum and maximum parseable numeric values for that column

#### Scenario: Auto gradient ignores non numeric values
- **WHEN** an auto gradient column contains values that cannot be parsed as numbers
- **THEN** those values are ignored when calculating the column minimum and maximum and receive no gradient color unless another rule matches

### Requirement: Match conditional colors
The system SHALL support universal `match` conditional colors for discrete values across string, number, and boolean columns.

#### Scenario: Boolean match
- **WHEN** a column defines `match` with `true: green`
- **THEN** boolean true values in that column render with green conditional styling

#### Scenario: Numeric match
- **WHEN** a column defines `match` with `0: yellow`
- **THEN** numeric zero values in that column render with yellow conditional styling

#### Scenario: String match
- **WHEN** a column defines `match` with `active: green`
- **THEN** rendered values equal to `active` under the column's type normalization render with green conditional styling

#### Scenario: Multiple match entries
- **WHEN** a column defines one `match` rule with multiple value/color entries
- **THEN** the system evaluates entries in saved-view order and applies the first matching entry color

### Requirement: Range conditional colors
The system SHALL support numerical `range` conditional colors for explicit numeric intervals where unmatched values are left uncolored.

#### Scenario: Lower bound range
- **WHEN** a percentage column defines a range entry `"<10": red`
- **THEN** parseable values lower than `10` render with red conditional styling and values greater than or equal to `10` are not colored by that rule

#### Scenario: Upper bound range
- **WHEN** a percentage column defines a range entry `">=90": red`
- **THEN** parseable values greater than or equal to `90` render with red conditional styling and values lower than `90` are not colored by that rule

#### Scenario: Bounded range
- **WHEN** a numeric column defines a range entry `">=50 <75": yellow`
- **THEN** parseable values greater than or equal to `50` and lower than `75` match that range

#### Scenario: Range leaves gaps uncolored
- **WHEN** a numeric column defines only ranges for `<10` and `>=90`
- **THEN** parseable values from `10` through values lower than `90` receive no color from those range rules

### Requirement: Identifier conditional colors
The system SHALL support string-mode `identifiers` conditional colors that assign unique rendered column values to generated colors from theme-level or view-level color families.

#### Scenario: Unique identifiers get stable colors
- **WHEN** a string or IP column defines `identifiers: {}`
- **THEN** each unique rendered value in that column receives a deterministic color reference from the active theme identifier families

#### Scenario: Repeated identifiers reuse colors
- **WHEN** a column with `identifiers: {}` contains the same rendered value in multiple rows
- **THEN** every occurrence of that value receives the same color

#### Scenario: Theme automatic identifier colors
- **WHEN** a column defines `identifiers: { colors: auto }`
- **THEN** identifier colors are generated from the active theme `[identifiers].colors` families

#### Scenario: View override identifier colors
- **WHEN** a column defines `identifiers: { colors: [cyan, "palette(198)", "#25A39AFF"] }`
- **THEN** identifier colors for that column are generated from the view-defined color families instead of the active theme families

### Requirement: Saved object mode
A saved view SHALL accept `source.object_mode: auto|record|entries` as a format-neutral source-opening option, validate it in the shipped schema and semantic parser, and apply it before an object-capable adapter constructs its table, with explicit CLI values taking precedence.

#### Scenario: Saved entries mode
- **WHEN** a matching saved view sets `source.format: json` and `source.object_mode: entries`
- **THEN** the selected JSON object's direct members become rows before `view.columns` is resolved

#### Scenario: Saved record mode
- **WHEN** a matching saved view sets `source.object_mode: record`
- **THEN** a selected JSON object retains single-row flattened-record interpretation

#### Scenario: Invalid saved mode
- **WHEN** a saved view sets `source.object_mode` to an unsupported value
- **THEN** schema or semantic validation records a non-fatal warning and does not apply that value

#### Scenario: Saved option incompatible with source
- **WHEN** a saved view combines explicit `record` or `entries` mode with a row-stream source or non-object selected shape
- **THEN** source-option validation reports or records the normal incompatibility without treating it as view configuration

#### Scenario: Serialize resolved mode
- **WHEN** a saved view is written for a selected object or map
- **THEN** generated YAML writes the effective `record` or `entries` value under `source.object_mode`

#### Scenario: Saved mode remains authoritative
- **WHEN** `source.object_mode` contains explicit `record` or `entries` and automatic detection changes later
- **THEN** the saved mode remains authoritative unless explicit CLI configuration overrides it

#### Scenario: Omit mode for non-object source
- **WHEN** saved-view YAML is generated for an array, scalar, or row stream
- **THEN** it omits `source.object_mode`

### Requirement: Saved views in non-interactive output
When compiled with saved-view support, batch output SHALL perform the same saved-view selection and apply nested `source` configuration before opening and nested `view` configuration before emitting stdout. For direct table output, `--sorted false` SHALL suppress all saved `view.sort` application, including pending sort keys, while preserving filters, columns, formatting, and nested source configuration. Preview limits SHALL apply after the effective view operations. Neither option SHALL modify the saved file.

#### Scenario: Automatically selected view
- **WHEN** redirected output opens a filename matching a saved view
- **THEN** its source query and view transform control the bounded output

#### Scenario: Forced named view
- **WHEN** batch output uses `--view <name>`
- **THEN** that named view controls source and view configuration even when its filename patterns do not match

#### Scenario: Saved views disabled
- **WHEN** batch output uses `--no-view`
- **THEN** no saved source table or other saved configuration is applied

#### Scenario: Saved SQLite table selection
- **WHEN** a database has multiple selectable candidates and a matching saved view sets `source.table`
- **THEN** batch output opens that table without interactive selection

#### Scenario: Pending column configuration
- **WHEN** bounded result traversal discovers a column whose `view.columns` configuration was pending
- **THEN** the configuration is applied before final widths and rows are rendered

#### Scenario: View filter produces no rows
- **WHEN** `view.filters` excludes every row from the bounded source result
- **THEN** batch output follows configured header visibility and empty-result rules without refilling

#### Scenario: Interactive transformation starts from saved view
- **WHEN** `--interactive` and `--output <format>` are combined
- **THEN** the TUI starts from nested saved configuration and final output uses subsequent live changes

#### Scenario: Preview disables saved sort only
- **WHEN** direct table output uses `--sorted false -n 30` with a saved view that defines source ordering, view sorting, filters, and formatting
- **THEN** source ordering, filters, and formatting remain active, view sorting is skipped, and at most 30 matching rows are emitted

#### Scenario: Late sort key stays disabled
- **WHEN** `--sorted false` is active and schema discovery resolves a pending saved view sort column
- **THEN** Tview does not apply that sort or trigger a full scan for it

### Requirement: Layered saved operations
Saved views SHALL represent source filtering and sorting separately from view filtering and sorting. Source operations SHALL determine the bounded source result before `source.limit`; view operations SHALL transform only that result.

#### Scenario: Source operations precede limit
- **WHEN** a saved SQLite view has source filters, source sort, and `source.limit: 1000`
- **THEN** the generated query applies filtering and sorting before limiting the source result

#### Scenario: View operations follow source limit
- **WHEN** the same saved view also has view filters and view sort
- **THEN** those operations execute locally over at most the rows returned by the source query

#### Scenario: View filter reduces visible rows
- **WHEN** a view filter hides 700 rows from a 1000-row source result
- **THEN** 300 rows remain visible and the system does not fetch replacement rows

#### Scenario: Search is transient
- **WHEN** search is active while a saved view is serialized
- **THEN** search remains a transient navigation operation and is omitted from both sections

### Requirement: Relational saved-view column matching
The system SHALL project relational column source identities into deterministic keys under `view.columns` without using display labels as runtime identity.

#### Scenario: Unique SQLite column name
- **WHEN** a selected table contains exactly one column named `email` and `view.columns.email` is configured
- **THEN** that configuration applies to the column with the matching relational source identity

#### Scenario: Duplicate SQLite column name
- **WHEN** a selected table contains duplicate `name` columns and `view.columns.name#2` is configured
- **THEN** that configuration applies to the second occurrence in source order

#### Scenario: Ambiguous unsuffixed SQLite name
- **WHEN** `view.columns.name` is configured and the selected table contains multiple columns with that name
- **THEN** the system does not guess and records a non-fatal warning recommending a deterministic occurrence key

#### Scenario: Serialize selected table
- **WHEN** the current view is opened from a SQLite table and saved-view YAML is generated
- **THEN** the YAML includes `source.format: sqlite`, `source.table`, and canonical relational keys under `view.columns`

### Requirement: Saved native source query
The nested saved-view schema SHALL accept `source.query` as native query text interpreted by `source.format`, with explicit CLI `--query` taking precedence and `source.table` remaining mutually exclusive.

#### Scenario: Saved ES|QL
- **WHEN** a matching saved view sets `source.format: elasticsearch` and `source.query: FROM logs-* | LIMIT 25`
- **THEN** source opening executes that text as ES|QL without displaying the Elasticsearch target picker

#### Scenario: Saved SQLite SQL
- **WHEN** a matching saved view sets `source.format: sqlite` and a valid read-only `source.query`
- **THEN** source opening executes the query through the confined SQLite native-query path

#### Scenario: Saved table and query conflict
- **WHEN** a saved source contains both `table` and `query`
- **THEN** saved-view validation reports the conflict and does not choose one silently

#### Scenario: CLI query precedence
- **WHEN** a saved view contains `source.query` and the user supplies `--query`
- **THEN** the CLI query replaces the saved query for that invocation

#### Scenario: Serialize native query
- **WHEN** the active source was opened from a user-supplied native query
- **THEN** generated saved-view YAML persists the configured base query under `source.query` rather than only the derived composed artifact

### Requirement: Saved remote source target
Saved-view matching and serialization SHALL support the safe textual identity of a remote positional source target while excluding URL userinfo, credentials, authorization headers, and other transport secrets.

#### Scenario: Match Elasticsearch endpoint
- **WHEN** a saved view targets an Elasticsearch endpoint and its safe target pattern matches the invocation
- **THEN** normal saved-view selection and source-option merging apply

#### Scenario: Serialize remote target
- **WHEN** a saved view is generated for a remote source
- **THEN** its matching target uses the endpoint's safe non-secret representation

#### Scenario: Secret-bearing URL
- **WHEN** a supplied remote URL contains user information or another secret-bearing component
- **THEN** generated YAML, diagnostics, and query artifacts omit or redact that component

### Requirement: Saved native query serialization
Saved-view serialization SHALL persist source-native input configuration separately from derived query provenance.

#### Scenario: Source operations around native query
- **WHEN** a native base query has source filters, source sort, or source limit
- **THEN** YAML stores the base under `source.query` and structured operations under their existing source fields

#### Scenario: Derived ES|QL is excluded
- **WHEN** Elasticsearch query provenance includes application-composed stages
- **THEN** generated YAML does not duplicate the final composed ES|QL as a second configuration value

#### Scenario: Derived SQLite SQL is excluded
- **WHEN** SQLite query provenance includes an outer bounded query
- **THEN** generated YAML does not replace the configured base query with the derived SQL
