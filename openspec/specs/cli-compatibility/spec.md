## Purpose

Define the supported command-line compatibility surface for the Rust `tview` replacement.

## Requirements

### Requirement: Replacement binary name
The Rust implementation SHALL install and run as a `tview` executable.

#### Scenario: User invokes tview
- **WHEN** a user runs `tview <filename>` after installing the Rust package
- **THEN** the Rust executable opens the target file in the terminal viewer

### Requirement: Existing CLI arguments
The Rust executable SHALL accept the existing command-line interface: positional filename, `-` for stdin, `--encoding`/`-e`, `--delimiter`/`-d`, `--quoting`, `--start_pos`/`-s`, `--width`/`-w`, `--double_width`, `--quote-char`/`-q`, and extra classic start-position arguments in `+y:x` form, plus `--format`, `--json-path`, and `--schema-scan` source options. A build with the default-enabled `sqlite` feature SHALL additionally accept `--table`.

#### Scenario: Current README invocation remains valid
- **WHEN** a user runs `tview sample/data_ohlcv.csv --start_pos 6,5 --encoding utf-8`
- **THEN** the command is accepted and the viewer starts at row 6, column 5 using the requested encoding

#### Scenario: Classic start position remains valid
- **WHEN** a user runs `tview sample/data_ohlcv.csv +6:5`
- **THEN** the viewer starts at row 6, column 5

#### Scenario: Existing CSV options remain valid
- **WHEN** a user supplies existing delimiter, quoting, quote-character, or encoding options for delimited input
- **THEN** those options retain their established meaning

### Requirement: Input format option
The Rust executable SHALL accept `--format auto|delimited|json|ndjson`, using `auto` by default, and SHALL reject incompatible format-specific argument combinations clearly. A build with the `sqlite` feature SHALL additionally accept `sqlite`, and a build with the `elasticsearch` feature SHALL additionally accept `elasticsearch`. When `--format` is omitted, an unambiguous registered URL scheme MAY resolve the effective format before existing local probing.

#### Scenario: Force JSON format
- **WHEN** a user runs `tview --format json response.data`
- **THEN** the JSON adapter is selected without relying on the filename extension

#### Scenario: Force delimited format
- **WHEN** a `.json`-named file actually contains delimited data and the user runs `tview --format delimited data.json`
- **THEN** the delimited adapter is selected

#### Scenario: Force SQLite format
- **WHEN** a user runs `tview --format sqlite --table users application.data`
- **THEN** the SQLite adapter is selected without relying on the filename extension

#### Scenario: Infer SQLite from LibSQL
- **WHEN** a user supplies a `libsql://` target without `--format`
- **THEN** the effective format resolves to SQLite before adapter capability validation

#### Scenario: Force Elasticsearch format
- **WHEN** a user runs `tview https://elastic.example:9200 --format elasticsearch`
- **THEN** the Elasticsearch adapter is selected without attempting content probing

#### Scenario: Ambiguous remote format
- **WHEN** a user supplies an HTTP(S) target without explicit or saved format
- **THEN** startup fails clearly rather than guessing a remote adapter

#### Scenario: SQLite feature is disabled
- **WHEN** a user runs a binary compiled without `sqlite`
- **THEN** `--format sqlite` is rejected as unavailable and `--table` is exposed only if another enabled source feature supports relation selection

#### Scenario: Feature-disabled format
- **WHEN** a user requests a format whose Cargo feature is disabled
- **THEN** that format is rejected as unavailable and its source-specific dispatch is absent

#### Scenario: Incompatible delimiter option
- **WHEN** a user combines `--format json` with `--delimiter`
- **THEN** argument or source-option validation rejects the incompatible combination with a clear error

### Requirement: JSON starting-path option
The Rust executable SHALL accept `--json-path <pointer>` using RFC 6901 syntax and SHALL apply it before JSON table construction.

#### Scenario: Select Elasticsearch hits
- **WHEN** a user runs `tview --format json --json-path /hits/hits response.json`
- **THEN** the embedded search-hit array is used as the table

#### Scenario: Invalid JSON pointer
- **WHEN** `--json-path` is not a valid RFC 6901 JSON Pointer
- **THEN** the invocation fails with a clear validation error

#### Scenario: JSON path with non-JSON format
- **WHEN** a user supplies `--json-path` while explicitly selecting `delimited`
- **THEN** validation rejects the incompatible source options

### Requirement: Schema scan option
The Rust executable SHALL accept `--schema-scan default|full`, using the bounded format default when omitted, with explicit CLI values overriding matching saved-view values.

#### Scenario: Force full JSON schema scan
- **WHEN** a user runs `tview --schema-scan full records.ndjson`
- **THEN** the selected structured adapter scans through the selected table's end before marking its schema complete

#### Scenario: Restore default scan for invocation
- **WHEN** a saved view requests a full scan and the user supplies `--schema-scan default`
- **THEN** the invocation uses the bounded default schema scan policy

### Requirement: Default column width mode
The Rust executable SHALL use `mode` as the default column width mode when `--width` is not provided.

#### Scenario: Width omitted
- **WHEN** a user runs `tview sample/data_ohlcv.csv` without `--width`
- **THEN** the viewer computes variable column widths using mode-based sizing

### Requirement: Python-style quoting names
The Rust executable SHALL accept Python CSV quoting names used by the existing CLI, including `QUOTE_MINIMAL`, `QUOTE_NONNUMERIC`, `QUOTE_ALL`, and `QUOTE_NONE`.

#### Scenario: MySQL pager quoting mode
- **WHEN** a user runs `tview -d '\t' --quoting QUOTE_NONE -`
- **THEN** the command is accepted and stdin is parsed with tab delimiters and no quote interpretation

### Requirement: Standard input mode
The Rust executable SHALL support `-` as the filename to read data from standard input while still allowing interactive terminal input for the TUI.

#### Scenario: Pipe into tview
- **WHEN** data is piped into `tview -`
- **THEN** the data is loaded from stdin and the TUI remains interactive after loading

### Requirement: Python import API removal
The Rust rewrite SHALL NOT provide or promise compatibility for the upstream Python import API.

#### Scenario: Documentation describes supported surface
- **WHEN** users read the Rust rewrite installation and usage documentation
- **THEN** the documented supported interface is the `tview` CLI, not a Python module API

### Requirement: Saved view CLI overrides
When compiled with the `saved-views` feature, the Rust executable SHALL accept saved view override arguments that force a named saved view or disable saved view application for the current invocation.

#### Scenario: Force saved view
- **WHEN** a user runs `tview --view cat-shards sample/data.csv`
- **THEN** the command is accepted and saved view selection uses the saved view named `cat-shards`

#### Scenario: Force saved view with extension
- **WHEN** a user runs `tview --view cat-shards.yml sample/data.csv`
- **THEN** the command is accepted and saved view selection uses the saved view named `cat-shards`

#### Scenario: Disable saved views
- **WHEN** a user runs `tview --no-view sample/data.csv`
- **THEN** the command is accepted and saved view discovery and application are skipped

#### Scenario: Conflicting saved view flags
- **WHEN** a user runs `tview --view cat-shards --no-view sample/data.csv`
- **THEN** argument parsing rejects the invocation with a clear error

#### Scenario: Saved views feature disabled
- **WHEN** the binary is compiled without the `saved-views` feature
- **THEN** the saved view override arguments are not part of the supported command-line surface

### Requirement: Format-neutral object mode option
The Rust executable SHALL accept `--object-mode auto|record|entries` and use `auto` when omitted. The shared CLI and source-option names SHALL be independent of any one serialization format so object-capable adapters, including future YAML and TOON adapters, can reuse them. After format resolution and structured-value selection, an adapter SHALL apply the mode only to a selected object/map and SHALL reject explicit incompatible formats or selected shapes clearly. This option SHALL NOT alter stdin buffering or imply an input format.

#### Scenario: Force keyed entries
- **WHEN** a user runs `tview --format json --object-mode entries repositories.json`
- **THEN** the selected JSON object's direct members become table rows without automatic shape inference

#### Scenario: Preserve record behavior
- **WHEN** a user runs `tview --format json --object-mode record object.json`
- **THEN** the selected object is represented as one flattened row

#### Scenario: Default automatic mode
- **WHEN** a user opens an object-capable structured input without supplying `--object-mode`
- **THEN** the effective object mode is `auto`

#### Scenario: Incompatible delimited format
- **WHEN** a user combines an explicit non-default object mode with `--format delimited`
- **THEN** argument or source-option validation rejects the combination with a clear error

#### Scenario: Incompatible NDJSON format
- **WHEN** a user combines `--format ndjson` with explicit `record` or `entries` object mode
- **THEN** argument or source-option validation rejects the combination because NDJSON retains one row per logical document

#### Scenario: Incompatible selected shape
- **WHEN** an object-capable adapter selects an array or scalar and the CLI explicitly requests `record` or `entries`
- **THEN** source opening fails with a clear incompatible-shape error

#### Scenario: Object mode does not imply stdin format
- **WHEN** stdin format remains unresolved while `--object-mode` is supplied
- **THEN** this option does not change the stdin buffering or format-resolution policy owned by the non-interactive input workflow

#### Scenario: CLI overrides saved mode
- **WHEN** a saved view selects one object mode and the user supplies a different `--object-mode`
- **THEN** the explicit CLI value takes precedence for that invocation

### Requirement: Composable interactive and output options
The Rust executable SHALL accept `--interactive`/`-i` as a runtime-mode flag and `--output <format>`/`-o <format>` as a serialization-format option. This change SHALL support `table`; future values such as `csv` and `markdown` SHALL extend the output format without becoming runtime modes. With neither option, Tview SHALL retain automatic terminal detection.

#### Scenario: Explicit table output
- **WHEN** a user runs `tview -o table data.json`
- **THEN** the command writes one formatted table to stdout and exits without starting the TUI

#### Scenario: Explicit view-only interaction
- **WHEN** a user runs `tview -i data.csv`
- **THEN** the command starts the interactive viewer and does not serialize the final live view after quitting

#### Scenario: View-only interaction with redirected stdout
- **WHEN** a user runs `tview -i data.csv > unused.txt` from a controlling terminal without `--output`
- **THEN** Tview uses the controlling terminal for the viewer and leaves redirected stdout empty

#### Scenario: Automatic interactive output is view-only
- **WHEN** a user runs `tview data.csv` with terminal stdout and neither output option
- **THEN** the command starts the interactive viewer and does not serialize the final live view after quitting

#### Scenario: Composed interactive table transform
- **WHEN** a user runs `tview -i -o table data.csv > edited.txt` from a controlling terminal
- **THEN** Tview uses the controlling terminal for interaction and writes only the final live view to redirected stdout after a normal quit

#### Scenario: Future composed CSV transform
- **WHEN** a future CSV adapter is available and a user runs `tview -i -o csv data.csv > edited.csv`
- **THEN** the same interactive runtime writes the final live view through the CSV adapter without changing the meaning of `-i`

#### Scenario: Invalid output value
- **WHEN** a user supplies an unsupported `--output` value
- **THEN** argument parsing rejects the invocation and lists the currently supported values without preventing new adapter values from being added later

#### Scenario: Redirect without explicit option
- **WHEN** a user runs `tview data.csv > table.txt` without `--output`
- **THEN** automatic runtime resolution selects default `table` output

### Requirement: Color mode option
The Rust executable SHALL accept `--color auto|always|never` and use `auto` when omitted, with table-mode color disabled unless `always` is explicitly selected.

#### Scenario: Force colored table
- **WHEN** a user runs `tview --output table --color always data.json`
- **THEN** stdout contains theme-derived ANSI table styling

#### Scenario: Force plain table
- **WHEN** a user runs `tview --color never data.csv` with redirected stdout
- **THEN** stdout contains no ANSI escape sequences

#### Scenario: Invalid color value
- **WHEN** a user supplies an unsupported `--color` value
- **THEN** argument parsing rejects the invocation and lists the supported values

### Requirement: Pipeline-compatible standard input
The existing `-` input mode SHALL compose with runtime and format selection so piped input remains interactive when stdout is a terminal, becomes non-interactive when stdout is redirected or piped under automatic mode, and can be interactively transformed when `-i` and `-o <format>` are combined.

#### Scenario: Piped input to interactive viewer
- **WHEN** a user runs `producer | tview -` with terminal stdout and default output mode
- **THEN** Tview materializes or opens stdin data and starts the interactive viewer

#### Scenario: Piped conversion
- **WHEN** a user runs `producer | tview - | consumer`
- **THEN** Tview reads source data from stdin and writes a plain formatted table to stdout without competing for terminal input

#### Scenario: Piped interactive transformation
- **WHEN** a user runs `producer | tview -i -o table - > transformed.txt` from a controlling terminal
- **THEN** Tview drains stdin as table data, uses the controlling terminal for UI events and drawing, and writes the final live view to `transformed.txt` after normal quit

### Requirement: Relation selection argument
When at least one relational or query-native source feature is enabled, the Rust executable SHALL accept `--table <name>` as a generic relation or target selector. The resolved adapter SHALL define which catalog entries are selectable and SHALL reject the option for non-relational sources.

#### Scenario: Select SQLite table
- **WHEN** a user runs `tview application.db --table users`
- **THEN** the command opens `users` when the input is SQLite and that relation is selectable

#### Scenario: Select Elasticsearch index
- **WHEN** a user runs `tview https://elastic.example:9200 --format elasticsearch --table application-events`
- **THEN** the command uses `application-events` as the generated bounded ES|QL `FROM` target without requiring picker discovery

#### Scenario: Select Elasticsearch data stream
- **WHEN** `--table` names a visible Elasticsearch data stream
- **THEN** the command selects the data stream as the ES|QL `FROM` target

#### Scenario: Select Elasticsearch alias
- **WHEN** `--table` names an Elasticsearch alias
- **THEN** the command passes the alias through as the ES|QL `FROM` target and lets Elasticsearch validate it

#### Scenario: Table option on non-relational input
- **WHEN** a user supplies `--table` for delimited, JSON, NDJSON, or stdin input
- **THEN** startup fails with a clear message that the resolved source does not support relation selection

#### Scenario: Delimited option on SQLite input
- **WHEN** a user supplies `--encoding`, `--delimiter`, `--quoting`, or `--quote-char` for SQLite input
- **THEN** startup fails with a clear message identifying the incompatible option

#### Scenario: Classified unsupported relation
- **WHEN** a selected adapter recognizes a name but classifies it as unavailable
- **THEN** startup reports the classified reason distinctly from a missing name

### Requirement: Local SQLite CLI scope
The SQLite CLI surface SHALL accept local filesystem and `file://` inputs but SHALL NOT interpret stdin, Turso Cloud, or `libsql://` URLs as supported SQLite sources in this change.

#### Scenario: File URI database
- **WHEN** a user supplies a `file://` URI whose resolved file is selected as SQLite
- **THEN** the system opens it through the local SQLite adapter

#### Scenario: SQLite from stdin
- **WHEN** a user explicitly selects SQLite for `-`
- **THEN** the system reports that SQLite requires a local path

#### Scenario: Remote URL
- **WHEN** a user supplies a Turso Cloud or `libsql://` URL
- **THEN** the system reports that remote database access is unsupported rather than attempting a local open

### Requirement: Generic native query option
When at least one native-query source feature is enabled, the Rust executable SHALL accept `--query <string>` and pass the string as the native query language selected by the resolved source format. `--query` and `--table` SHALL be mutually exclusive.

#### Scenario: Elasticsearch ES|QL
- **WHEN** a user supplies `--format elasticsearch --query 'FROM logs-* | LIMIT 10'`
- **THEN** the Elasticsearch adapter receives the string as complete ES|QL and does not display a target picker

#### Scenario: SQLite SQL
- **WHEN** a user supplies `--format sqlite --query 'SELECT id, name FROM users'`
- **THEN** the SQLite adapter receives the string as a confined row-producing SQL query

#### Scenario: Table and query conflict
- **WHEN** a user supplies both `--table` and `--query`
- **THEN** argument validation rejects the invocation before opening the source

#### Scenario: Query on non-native source
- **WHEN** a user supplies `--query` for delimited, JSON, NDJSON, or stdin input
- **THEN** startup fails with a clear message that the resolved source does not support native queries

#### Scenario: Query feature unavailable
- **WHEN** all compiled source adapters lack native-query support
- **THEN** `--query` is omitted from the compiled CLI surface

### Requirement: Table output sorting option
The CLI SHALL accept `--sorted true|false` with an explicit value and SHALL default to `true`. Explicit use SHALL be valid only for resolved direct table output. The implicit default SHALL NOT change or invalidate other modes. `false` SHALL suppress saved view sorting for the invocation without altering source ordering or other saved configuration.

#### Scenario: Default retains saved sorting
- **WHEN** direct table output omits `--sorted` or supplies `--sorted true`
- **THEN** configured saved view sorting is applied

#### Scenario: Disable saved sorting
- **WHEN** direct table output supplies `--sorted false`
- **THEN** saved view sort keys are not applied and rows retain the source result order after filtering

#### Scenario: Invalid sorting value
- **WHEN** `--sorted` lacks a value or its value is not `true` or `false`
- **THEN** argument parsing fails with exit code 2 before reading input or writing stdout

### Requirement: Table preview row limit option
The CLI SHALL accept equivalent `-n <count>` and `--top-lines <count>` options with a positive integer representable by the implementation's row-count type. Omitting the option SHALL impose no additional output limit. The count SHALL limit logical data rows, excluding the header and remaining-row summary. Explicit use of either preview option SHALL require direct table output after normal mode resolution. Incompatible modes SHALL fail with exit code 1 before source consumption or stdout output; invalid argument values SHALL fail with exit code 2.

#### Scenario: Short and long forms agree
- **WHEN** otherwise identical direct table invocations use `-n 30` and `--top-lines 30`
- **THEN** both emit the same first 30 result rows or all result rows if fewer exist

#### Scenario: Automatic table mode accepts preview options
- **WHEN** stdout is redirected, no output option is supplied, and `--sorted false -n 30` is supplied
- **THEN** normal mode resolution selects table output and applies the preview options

#### Scenario: Invalid row limit
- **WHEN** the count is missing, zero, negative, fractional, nonnumeric, or too large for the row-count type
- **THEN** argument parsing fails with exit code 2 and stdout remains empty

#### Scenario: Incompatible mode
- **WHEN** either option is explicitly supplied with automatic or explicit TUI mode, interactive export, JSON output, or JSONL output
- **THEN** Tview rejects the combination before consuming input and explains that the option requires direct table output
