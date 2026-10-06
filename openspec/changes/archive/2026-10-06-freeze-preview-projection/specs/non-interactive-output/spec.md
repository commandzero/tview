# Spec Delta

## ADDED Requirements

### Requirement: Frozen projection preparation ownership
A single source-neutral preparation owner SHALL turn an activated source result and configured local view into an immutable complete or prefix projection. It SHALL own selection, schema acceptance, delayed operation handling, necessary traversal, presentation profile inputs, and remainder evidence. Preparation SHALL NOT change live view configuration or screen state, replace or detach its stores, or require a temporary preview mode while applying settings.

#### Scenario: Complete projection without changing the viewer
- **WHEN** a configured live view is prepared without a preview limit for table, JSON, or JSONL output
- **THEN** the projection contains the entire effective bounded result with late configuration resolved, while the live view retains its configuration, store attachments, cursor, viewport, and screen state

#### Scenario: Direct preview does not require a viewer
- **WHEN** an activated source result and configured local view facts are prepared with a direct table preview limit
- **THEN** preparation needs no interactive viewer or special viewport and returns only selected rows and frozen presentation plus remainder evidence

#### Scenario: Preparation failure preserves the viewer
- **WHEN** required traversal, operation resolution, or presentation preparation fails
- **THEN** no projection is published or stdout written, and the live view is not left truncated, detached, or configured for a temporary preview lifecycle

#### Scenario: Frozen output cannot resume ingestion
- **WHEN** a prepared projection is serialized or read repeatedly, or live presentation settings change after preparation
- **THEN** its rows, schema, presentation, and remainder evidence stay unchanged and the output consumer does not fetch, bind, or profile source rows

## MODIFIED Requirements

### Requirement: Modular output adapters
Tview SHALL dispatch each selected `OutputFormat` through a source-neutral output adapter in both direct and post-interactive lifecycles. Shared orchestration SHALL open the source, apply the saved or frozen live view, satisfy the adapter's declared preparation requirements through the frozen projection preparation owner, validate adapter capabilities, provide an immutable prepared projection, and own stdout, stderr, broken-pipe, and exit-status behavior. Format-specific adapters SHALL own only their layout, escaping, styling, and byte serialization rules. For direct table previews, the preparation owner SHALL provide an immutable prefix projection and remaining-row metadata instead of requiring a complete projection. Source opening SHALL receive the preview policy before ingestion or saved-view preparation begins. Orchestration and serialization SHALL NOT independently select rows, reconcile accepted versus lookahead schema, resolve deferred operations, or complete a prepared source; these responsibilities SHALL remain behind the preparation interface.

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

#### Scenario: Structured output remains complete
- **WHEN** direct or post-interactive output selects JSON or JSONL without a preview limit
- **THEN** the same preparation owner supplies all effective bounded rows and completed schema, preserving display strings without table clipping, padding, control replacement, or ANSI styling

### Requirement: Table preview preparation
Direct table output with `--top-lines N` SHALL emit the first N rows of the effective result, applying source operations and limits, then view filters, then enabled view sorting, before selecting the prefix. Without enabled local sorting, an active or pending numeric view filter, or a full-schema request, file preview preparation SHALL stop after N matching rows and at most one additional matching row, with bounded parser read-ahead. It SHALL NOT complete ingestion, indexing, schema scanning, width profiling, color profiling, or row counting solely to prepare a preview. This behavior SHALL apply regardless of file size and to stdin. Filters, delayed column resolution, and locating selected nested data may require scanning more input. Enabled sorting SHALL retain exact whole-active-result semantics. Every active or pending numeric view filter SHALL retain the existing conservative whole-active-result profiling before interpretation; prefix-only numeric interpretation is not introduced. A preview limit SHALL NOT be pushed ahead of local filters or enabled local sorting into a native source query; native adapters can eagerly receive their existing bounded result but SHALL NOT expand or refill it for preview preparation.

Default schema and type discovery SHALL use the selected rows and required bounded format detection. Under default scanning, omitted or lookahead rows SHALL NOT add preview columns. Under either scan policy, omitted or lookahead values SHALL NOT affect widths or automatic gradients. Explicit or saved full-schema scanning SHALL remain honored, including completing the active result when required even if the initial schema already appears complete. Full-schema discovery SHALL preserve existing source/view-filtered field-presence semantics: fields occurring only in rejected rows SHALL NOT become output columns; fields present in accepted rows beyond N SHALL remain eligible. Field presence SHALL use source row-presence evidence, not empty display strings, and established delimited header columns SHALL remain distinct from absent structured fields, subject to the existing empty view-filtered-result exception. Delayed filters SHALL apply to earlier deferred rows before selection is finalized; unresolved saved view operations at schema completion SHALL follow existing missing-column behavior, without discarding otherwise eligible rows. Automatic gradients SHALL profile emitted rows only; fixed rules and explicit widths SHALL remain honored. Plain output SHALL NOT perform color profiling. All required preparation and lookahead SHALL succeed before the first output byte. Unread trailing data SHALL NOT be validated merely to complete the preview.

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

#### Scenario: Late filter rechecks preceding rows
- **WHEN** a saved view filter refers to a field first discovered after earlier rows were deferred
- **THEN** those earlier rows are evaluated with the resolved filter in source order, accepted rows contribute at most N emitted rows, and later matches provide remainder evidence rather than expanding the prefix

#### Scenario: Missing saved filter at completion
- **WHEN** a saved view filter's column never appears before schema completion
- **THEN** the existing missing-column policy leaves otherwise eligible rows available for preview instead of discarding deferred rows

#### Scenario: Numeric filtering uses late evidence
- **WHEN** an active or pending numeric view filter exists, including one whose units or profile evidence appears later in the source result
- **THEN** preparation completes the existing whole-active-result numeric profiling and evaluates the filter before selecting N rows, without inferring a prefix-sufficient fast path

#### Scenario: Saved sort override includes delayed binding
- **WHEN** `--sorted false` suppresses a saved view sort whose column appears late
- **THEN** saved local sorting remains suppressed without suppressing view filters or presentation, changing native source order, or rewriting the saved file

#### Scenario: Rejected fields do not leak
- **WHEN** JSON rows rejected by source or view filters contain fields absent from all selected rows in default-scan mode
- **THEN** those fields do not appear in the frozen preview even if their discovery was necessary to evaluate filters or find lookahead

#### Scenario: Full schema includes later accepted fields only
- **WHEN** a full-schema JSON preview emits an initial accepted row and later accepted rows introduce one field while rejected rows introduce another
- **THEN** the later accepted field is an output column, the rejected-only field is not, and omitted values still do not influence widths or automatic gradients

#### Scenario: Full scan cannot short-circuit on initial schema
- **WHEN** a full-schema request initially sees all currently known columns and later accepted rows add a field or resolve a pending filter
- **THEN** preparation traverses the required active result before freezing rather than stopping because the initial columns are already known

#### Scenario: Structured presence is not display emptiness
- **WHEN** an accepted structured row explicitly contains a null or empty field and another field appears only in rejected or default-scan lookahead rows
- **THEN** the explicitly present field remains eligible and the rejected or lookahead-only field does not become eligible merely through null padding

#### Scenario: Empty filtered structured result
- **WHEN** a JSON view filter rejects every row and the resulting preview has no accepted fields
- **THEN** rejected initial or late fields do not produce a header, stdout is empty, and there is no summary

#### Scenario: Empty view-filtered delimited result
- **WHEN** a local view filter rejects every delimited row and no source filter is active
- **THEN** the existing empty-result policy suppresses all output columns, stdout is empty, and there is no summary even when an input header was established


#### Scenario: Empty source-filtered delimited result
- **WHEN** source filtering rejects every delimited row under full-schema scanning but the input has an established header
- **THEN** output retains the established header columns under the existing header policy, excludes rejected-only extra columns, and emits no summary

#### Scenario: Required matching lookahead may skip rejected rows
- **WHEN** N rows have been selected and following rows fail the effective filters before the next matching row
- **THEN** preparation checks through that next match or effective EOF, does not expose those rows' presentation, and any encountered required parse error leaves stdout empty

#### Scenario: Exact stdin boundary requires evidence
- **WHEN** live stdin has supplied exactly N matching rows but neither another match nor EOF
- **THEN** preparation waits for necessary remainder evidence rather than claiming completion; after one additional match it closes input without waiting for producer EOF

#### Scenario: Bounded eager native result
- **WHEN** a native adapter receives a complete bounded response and local filters leave fewer than N rows
- **THEN** preview uses only those rows, issues no refill/count query, and preserves source-native ordering when local sorting is disabled

#### Scenario: Frozen selected-row presentation
- **WHEN** omitted rows contain wider values or numeric extremes and forced color is enabled
- **THEN** selected rows and their header alone supply automatic-width and automatic-gradient inputs, while explicit widths and fixed rules still apply and the summary stays unstyled

#### Scenario: Plain preview skips color profiling
- **WHEN** preview uses `auto` or `never` color output with configured conditional colors
- **THEN** preparation performs no color-profile traversal and table output retains existing Unicode clipping, control escaping, newline termination, and plain summary behavior
