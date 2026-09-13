## MODIFIED Requirements

### Requirement: Complete configured logical result
Batch output SHALL render the complete logical view after applying source options, the active bounded source result, and selected view configuration, including labels, column visibility and order, formats, widths, alignment, header visibility, view filters, view sort, null placement, and source-derived schema updates. Completion SHALL mean the entire active source result, not rows outside its configured source-query limit. Cursor position, viewport origin, selection styling, search state, and TUI-only start position SHALL NOT limit or decorate output. For direct table output with `--top-lines`, completion SHALL instead mean the selected preview prefix under the Table preview preparation requirement. Saved view sorting SHALL respect `--sorted`. All other output modes and invocations without a preview limit SHALL retain complete-result preparation.

#### Scenario: Saved view controls output
- **WHEN** output has no preview limit, sorting is enabled, and an automatically selected or explicitly named saved view configures source operations, hides columns, formats values, filters rows, and sorts the view
- **THEN** batch output contains every row and visible column in the final view of the bounded source result

#### Scenario: SQLite source limit bounds output
- **WHEN** SQLite batch output uses a source limit of 1000
- **THEN** completion traverses at most those 1000 source rows even when the selected table contains more rows

#### Scenario: View filter does not refill output
- **WHEN** a view filter leaves 17 rows from a limited 1000-row SQLite result
- **THEN** output contains those 17 rows and does not query for replacements

#### Scenario: No saved view uses defaults
- **WHEN** no saved view applies
- **THEN** batch output uses source-defined headers, visible columns, display formatting, width mode, alignment defaults, source order, and the source's default result limit

#### Scenario: Start position does not truncate output
- **WHEN** a batch invocation includes an existing start-position argument
- **THEN** the complete bounded logical result is emitted because start position is an interactive cursor setting

#### Scenario: Late schema is included
- **WHEN** output has no preview limit and an incremental source discovers additional columns while completing its active result
- **THEN** applicable saved configuration is resolved before final output layout

### Requirement: Stable complete-table widths
Before writing the first table line, table output SHALL complete the active bounded source result and resolve one stable display width per visible column. It SHALL NOT cross a source-query limit to discover wider values. Explicit per-column widths SHALL be honored; otherwise each column SHALL expand to the widest normalized header or rendered value in that active result. For direct table output with `--top-lines`, automatic widths SHALL instead use only the emitted prefix and its header; lookahead and omitted rows SHALL NOT affect widths. Explicit widths SHALL still apply.

#### Scenario: Later wide value affects initial lines
- **WHEN** output has no preview limit and a value near the end of the active result is wider than earlier values
- **THEN** the header and preceding rows use that final wider column width

#### Scenario: Wider value lies beyond SQLite limit
- **WHEN** a wider database value exists outside the active source result
- **THEN** it does not affect output width and is not fetched for profiling

#### Scenario: Explicit width clips values
- **WHEN** saved view or CLI width configuration is smaller than a rendered value
- **THEN** that cell is clipped without shifting later columns

#### Scenario: Incremental result is fully traversed
- **WHEN** an incremental SQLite store supplies table output without a preview limit
- **THEN** Tview traverses the complete bounded result for rows and width profiling without relying on a terminal viewport

### Requirement: Deterministic fixed-width text format
Plain table output SHALL emit zero or more newline-terminated physical lines. Each included header or data row SHALL contain visible cells in configured order, aligned and clipped by Unicode display width, separated by exactly the configured column gap, with no leading location field, borders, divider line, hidden-column markers, footer, or trailing spaces after the final cell. Direct table previews SHALL additionally append the remaining-row summary defined below when rows are omitted. This summary SHALL be the only exception to the no-footer rule.

#### Scenario: Header and rows
- **WHEN** header visibility is enabled for a table containing data
- **THEN** the first output line is the formatted header and each subsequent line is one formatted logical row

#### Scenario: Header is hidden
- **WHEN** header visibility is disabled
- **THEN** output begins with the first data row and contains no replacement heading or divider

#### Scenario: Empty result with header
- **WHEN** the logical result has zero rows but has visible columns and header visibility is enabled
- **THEN** output contains only the formatted header line

#### Scenario: Empty result without header
- **WHEN** the logical result has zero rows and header visibility is disabled
- **THEN** stdout receives zero bytes

#### Scenario: Right-aligned numeric cell
- **WHEN** a visible column resolves to right alignment
- **THEN** each shorter cell is left-padded to its resolved display width

#### Scenario: Left-aligned text cell
- **WHEN** a visible column resolves to left alignment and is followed by another column
- **THEN** each shorter cell is right-padded to its resolved display width before the column gap

#### Scenario: Unicode width and clipping
- **WHEN** a cell contains wide or combining Unicode characters
- **THEN** padding and clipping use terminal display width, preserve valid UTF-8, and do not exceed the resolved column width

#### Scenario: Embedded control characters
- **WHEN** a rendered cell contains newline, carriage-return, tab, escape, or another control character
- **THEN** table mode replaces it with a visible escaped representation so one logical row remains one physical output line

### Requirement: Modular output adapters
Tview SHALL dispatch each selected `OutputFormat` through a source-neutral output adapter in both direct and post-interactive lifecycles. Shared orchestration SHALL open the source, apply the saved or frozen live view, satisfy the adapter's declared preparation requirements, validate adapter capabilities, provide an immutable prepared projection, and own stdout, stderr, broken-pipe, and exit-status behavior. Format-specific adapters SHALL own only their layout, escaping, styling, and byte serialization rules. For direct table previews, shared orchestration SHALL prepare an immutable prefix projection and remaining-row metadata instead of requiring a complete projection. Source opening SHALL receive the preview policy before ingestion or saved-view preparation begins.

#### Scenario: Fixed-width table adapter
- **WHEN** resolved output format is `table`
- **THEN** the batch driver selects the fixed-width adapter and supplies its requested complete or preview projection and width/style preparation

#### Scenario: Interactive mode reuses selected adapter
- **WHEN** interactive mode quits normally with `--output table`
- **THEN** the shared output driver selects the same fixed-width table adapter using the frozen live view rather than a separate TUI exporter

#### Scenario: Future Markdown adapter
- **WHEN** a future `markdown` output value and adapter are added
- **THEN** it can reuse source opening, saved-view application, prepared projection, diagnostics, and stream handling while defining Markdown-specific escaping and layout without changing the TUI or table adapter

#### Scenario: Unsupported adapter capability
- **WHEN** an output option such as `--color always` is incompatible with the selected adapter
- **THEN** Tview rejects the invocation before writing stdout with a clear diagnostic on stderr

### Requirement: SQLite output query completion
Direct and post-interactive output SHALL prepare the latest requested bounded SQLite source result before passing an immutable projection to the selected output adapter. For direct table previews, required traversal SHALL be limited to the prefix and lookahead needed after filtering and enabled sorting; the bounded query must still succeed before output. Preparation SHALL NOT fetch the rest solely for layout or row counting.

#### Scenario: Direct output waits for initial query
- **WHEN** the initial SQLite source query is still running
- **THEN** the output driver writes no stdout until the query and required bounded traversal succeed

#### Scenario: Interactive export waits for latest query
- **WHEN** normal interactive quit requests final output while a newer source-query revision is pending
- **THEN** final preparation awaits the latest revision before freezing and serializing the view

#### Scenario: Query preparation fails
- **WHEN** the required SQLite query or bounded traversal fails
- **THEN** no table bytes are emitted and the existing output error contract applies

#### Scenario: Generated SQL stays out of adapter output
- **WHEN** SQLite query provenance exists during normal table serialization
- **THEN** stdout contains only the selected output adapter's bytes

### Requirement: Elasticsearch output query completion
Direct and post-interactive output SHALL await the latest required bounded Elasticsearch query and complete its active result before passing a source-neutral immutable projection to the selected output adapter. For direct table previews, the query response must still succeed, but local projection and profiling SHALL cover only the selected prefix and required lookahead. The native adapter may receive a complete bounded response; previews SHALL NOT expand its source limit or issue an extra query solely to count omitted rows.

#### Scenario: Direct output waits for ES|QL
- **WHEN** the initial ES|QL request is pending
- **THEN** the output driver writes no stdout until query execution and required bounded preparation succeed

#### Scenario: Interactive export waits for latest revision
- **WHEN** normal interactive quit requests final output while a newer Elasticsearch source revision is pending
- **THEN** final preparation awaits that latest revision before freezing and serializing the view

#### Scenario: Partial result policy
- **WHEN** Elasticsearch completes with an allowed partial result
- **THEN** stdout contains only the prepared result while partial-result warning metadata is reported on stderr

#### Scenario: Query preparation fails
- **WHEN** discovery, mappings, ES|QL, schema construction, or bounded traversal fails
- **THEN** no output-adapter bytes are emitted and the existing output failure contract applies

#### Scenario: Native query stays out of output
- **WHEN** ES|QL provenance exists during normal table serialization
- **THEN** stdout contains only the selected output adapter's bytes

## ADDED Requirements

### Requirement: Table preview preparation
Direct table output with `--top-lines N` SHALL emit the first N rows of the effective result, applying source operations and limits, then view filters, then enabled view sorting, before selecting the prefix. Without enabled local sorting or a full-schema request, file preview preparation SHALL stop after N matching rows and at most one additional matching row, with bounded parser read-ahead. It SHALL NOT complete ingestion, indexing, schema scanning, width profiling, color profiling, or row counting solely to prepare a preview. This behavior SHALL apply regardless of file size and to stdin. Filters and locating selected nested data may require scanning more input. Enabled sorting SHALL retain exact whole-result semantics even when it requires full traversal.

Default schema and type discovery SHALL use the selected rows and required bounded format detection. Omitted or lookahead rows SHALL NOT add preview columns or affect widths or automatic gradients. Explicit or saved full-schema scanning SHALL remain honored. Automatic gradients SHALL profile emitted rows only; fixed rules and explicit widths SHALL remain honored. Plain output SHALL NOT perform color profiling. All required preparation and lookahead SHALL succeed before the first output byte. Unread trailing data SHALL NOT be validated merely to complete the preview.

#### Scenario: Large unsorted file stops early
- **WHEN** a large CSV with no filters is opened with `--output table --sorted false -n 30`
- **THEN** Tview prepares 30 rows and one matching lookahead row with bounded parser read-ahead, writes the preview, and exits without reading the rest

#### Scenario: Small-file threshold does not force completion
- **WHEN** a file below the normal incremental-store threshold contains many more rows than the requested preview
- **THEN** preview preparation still stops after its prefix and lookahead rather than eagerly loading the whole file

#### Scenario: Sorted top rows are exact
- **WHEN** a saved view sorts numerically descending and a preview uses default sorting
- **THEN** the preview contains the highest N matching rows from the active result even if they occur at the end of the input

#### Scenario: Filtering precedes the preview limit
- **WHEN** the first 100 source rows fail a view filter and the next 11 pass with `-n 10 --sorted false`
- **THEN** output contains those first 10 matching rows and confirms another matching row without counting the remaining input

#### Scenario: Source limit remains authoritative
- **WHEN** a source limit admits 100 rows, only 5 pass the view filter, and the preview requests 10
- **THEN** output contains 5 rows without querying beyond that source limit or reporting rows outside it as omitted

#### Scenario: Late fields and wide values
- **WHEN** default-scan JSON preview rows have a narrower schema and values than omitted rows
- **THEN** only the selected prefix determines preview columns, inferred types, and automatic widths

#### Scenario: Full schema remains explicit
- **WHEN** CLI or saved configuration requests a full schema scan with a preview limit
- **THEN** Tview completes that scan before output and applies the preview limit to emitted data rows

#### Scenario: Stdin producer has not closed
- **WHEN** stdin has supplied N matching rows and one additional match but the producer remains open
- **THEN** an unsorted default-scan preview finishes without waiting for EOF

#### Scenario: Required preparation fails
- **WHEN** decoding the prefix or required lookahead fails
- **THEN** stdout remains empty and Tview reports the error on stderr with exit code 1

#### Scenario: Malformed unread suffix
- **WHEN** malformed content lies beyond all input needed for an unsorted default-scan preview
- **THEN** Tview does not read that suffix merely to validate the whole source

### Requirement: Preview remaining-row summary
A truncated table preview SHALL append one newline-terminated, unstyled stdout line. If the exact effective filtered result count T is already available, the line SHALL be `<T - emitted> more rows...`. If another matching row is confirmed but the exact count is unknown, the line SHALL be `more rows...`. Tview SHALL NOT scan the remaining input or issue a separate count query just to obtain a numeric summary, and SHALL NOT substitute an unfiltered or estimated count. If no result rows are omitted, no summary SHALL be emitted. The header and summary SHALL NOT count toward N. Existing escaping SHALL keep each logical data row on one physical line.

#### Scenario: Known remainder
- **WHEN** an effective result has exactly 125 rows and a preview emits 30
- **THEN** the final line is `95 more rows...`

#### Scenario: Unknown remainder
- **WHEN** a preview emits 30 rows and confirms one extra match without knowing the total
- **THEN** the final line is `more rows...` and no counting pass occurs

#### Scenario: Exact boundary or shorter result
- **WHEN** the effective result has N or fewer rows
- **THEN** output contains all result rows and no remaining-row summary

#### Scenario: Empty result
- **WHEN** no row matches
- **THEN** output follows the existing header visibility and empty-result rules with no summary

#### Scenario: Multiline cells and header
- **WHEN** `-n 2` previews quoted multiline CSV records with a visible header and additional rows
- **THEN** output has one header line, two escaped data lines, and one summary line
