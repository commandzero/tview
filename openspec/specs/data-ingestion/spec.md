## Purpose

Define data source, decoding, parsing, normalization, and large-file groundwork behavior for the Rust `tview` implementation.

## Requirements

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

### Requirement: Encoding detection and override
The system SHALL use the provided encoding when `--encoding` is set and SHALL otherwise attempt the compatibility encoding set with specific encodings before permissive single-byte fallbacks. The compatibility set SHALL include locale encoding, `utf-8`, `utf-16`, `iso8859-1`, `iso8859-2`, `cp720`, and `latin-1`, with `latin-1` as a late fallback.

#### Scenario: Latin-1 sample file
- **WHEN** a Latin-1 file is opened without `--encoding`
- **THEN** the file is decoded using a compatible fallback encoding if more specific encodings do not match

#### Scenario: Explicit encoding wins
- **WHEN** a user passes `--encoding iso8859-1`
- **THEN** the system decodes input with `iso8859-1` rather than sniffing another encoding

### Requirement: CSV delimiter and quoting compatibility
The system SHALL parse CSV-like input using explicit delimiter, quote character, and quoting options when provided, and SHALL infer a delimiter when no delimiter is provided.

#### Scenario: Explicit delimiter
- **WHEN** a user passes `--delimiter '\t'`
- **THEN** the parser treats tab as the field delimiter

#### Scenario: Explicit quote character
- **WHEN** a user passes `--quote-char "'"`
- **THEN** the parser uses `'` as the quote character

### Requirement: Space-delimited normalization
The system SHALL preserve the current space-delimited behavior: normalize repeated whitespace using shell-like quote parsing and strip a leading `#` or `%` only from the first line when space-delimited data is detected.

#### Scenario: Annotated numeric sample
- **WHEN** the annotated numeric sample begins with `#` and uses aligned spaces
- **THEN** the first line comment marker is stripped and repeated spacing is normalized before parsing

### Requirement: Row normalization
The system SHALL normalize parsed rows to a rectangular table by padding shorter rows with empty cells.

#### Scenario: Uneven rows
- **WHEN** input rows contain different numbers of fields
- **THEN** all parsed rows expose the same column count and missing cells are empty strings

### Requirement: Large-file groundwork
The system SHALL use the previously introduced lazy threshold and store abstractions in the live format-aware table-opening path rather than leaving them as an unused prototype.

#### Scenario: Lazy threshold is centralized
- **WHEN** a size-based format adapter decides whether a seekable file requires incremental handling
- **THEN** the default lazy threshold is available as a named configurable constant set to 100 MiB

#### Scenario: Live viewer uses table store
- **WHEN** any supported source is opened for the interactive viewer
- **THEN** row access is routed through the selected in-memory or incremental table store

#### Scenario: Existing delimited compatibility remains
- **WHEN** an existing CSV-like input is opened with encoding, delimiter, quote, or quoting options
- **THEN** format-aware opening preserves the established decoding, parsing, normalization, and header-classification behavior

### Requirement: URL scheme format inference
Format resolution SHALL use explicit CLI or saved format first, then an unambiguous registered URL-scheme mapping, then existing local signature, extension, and bounded content probing. It SHALL NOT infer a source format from an ambiguous remote scheme.

#### Scenario: Explicit format wins
- **WHEN** a target has a recognized scheme and the user supplies a compatible explicit format
- **THEN** the explicit format selects the adapter

#### Scenario: LibSQL scheme
- **WHEN** a target uses `libsql://` and no format is supplied
- **THEN** format resolution selects SQLite before the SQLite adapter reports whether remote execution is supported

#### Scenario: Ambiguous HTTPS scheme
- **WHEN** an HTTPS target has no explicit or saved format
- **THEN** Tview requires format selection instead of guessing Elasticsearch, JSON, or another HTTP-backed source

#### Scenario: Local format probing remains
- **WHEN** the target is a local path without an explicit format
- **THEN** existing signature, extension, and bounded content probing behavior remains authoritative

### Requirement: Strict TOON 4.1 input
The system SHALL read TOON 4.1 UTF-8 documents in strict mode through its typed structured-table pipeline. Under `auto`, a `.toon` extension SHALL select TOON without ambiguous content probing unless delimited-only options override extension detection; explicit `--format toon` SHALL support local files and stdin. Plain stdin under `auto` SHALL retain its delimited default. TOON output SHALL NOT be added by the reader.

#### Scenario: Explicit delimited options override extension detection
- **WHEN** a local file has a `.toon` extension, the selected format is `auto`, and delimited-only options are supplied
- **THEN** the reader selects delimited input rather than TOON, subject to the existing SQLite signature precedence

#### Scenario: Nested tabular input
- **WHEN** a TOON document contains tabular arrays with nested header fields, comments, an initial BOM, or CRLF
- **THEN** the reader preserves ordered structured column identities and native null, boolean, numeric, and text cells
- **AND** displayed TOON column labels retain the complete nested field path relative to the selected table, including fields discovered after bounded schema discovery
- **AND** a later field whose qualified label conflicts with an existing column uses its canonical pointer label without renaming previously assigned labels

#### Scenario: Selected keyed objects
- **WHEN** a TOON object is selected with `--json-path` and `--object-mode entries`
- **THEN** its direct members become rows with synthetic `@key` identity using the shared object-mode rules
- **AND** automatic detection uses the shared three-member minimum, 64-entry maximum, and 1 MiB logical-entry byte budget

#### Scenario: Malformed or unrepresentable input
- **WHEN** TOON input has invalid UTF-8, strict structural errors, duplicate keys or header fields, mismatched counts or widths, or nonfinite numeric overflow
- **THEN** preparation fails before serialized output
- **AND** signed 64-bit integers remain exact, other finite numbers may use floating approximation, and numeric underflow may become zero

#### Scenario: Eager validation and schema discovery
- **WHEN** a TOON source is opened with a preview, source limit, or bounded schema scan
- **THEN** the reader decodes and validates the complete document before limiting rows
- **AND** schema discovery measures compact JSON-equivalent logical row bytes rather than evenly distributing file bytes

#### Scenario: Filtered preview schema
- **WHEN** a TOON preview uses source filters
- **THEN** displayed columns exclude fields present only in rejected rows
- **AND** a bounded preview discovers fields from its accepted prefix
- **AND** a full-schema preview includes fields from later accepted rows only within the source cap, without relaxing full-document validation
