---
type: Guide
title: CLI output and compatibility
description: Output schemas, exit codes, stream behavior, and compatibility rules.
generated: { by: codex/gpt-6, at: 2026-09-13T19:15:03Z }
---

# CLI output and compatibility

## Interactive mode and output

With no mode options, Tview opens the viewer when stdout is a terminal and
writes a fixed-width table when stdout is redirected or piped.

| Options | Behavior |
| --- | --- |
| `--interactive`, `-i` | Open the viewer without exporting. |
| `--output table`, `-o table` | Write a fixed-width text table. |
| `--output json` or `--output jsonl` | Write structured output. |
| `--interactive --output <format>` | Open the viewer and export the final view after a normal quit. |

```sh
tview data.csv --output table
tview --interactive data.csv
tview --interactive --output table data.csv > selected.txt
tview --format ndjson - < records.ndjson
tview --interactive --output table - < data.csv > selected.txt
```

Interactive exports include the session's formatting, filters, sorting, and
visible columns. With stdin as the source, Tview reads keyboard input and draws
the UI through the controlling terminal. It keeps reading finite stdin in the
background and waits for EOF before exporting, so the export includes late rows
and columns. Use `tview --version` to print the package version.

## Text tables

Table output uses fixed-width text. It includes every configured row and visible
column without limiting the total table width to the terminal. `--width <number>`
and saved fixed widths can clip individual cell contents; otherwise downstream
tools handle wrapping, paging, or truncation. Batch output never enters raw mode
or the alternate screen.

`--color auto` and `--color never` write plain text. `--color always` adds ANSI
styling from the active theme. Table output does not preserve CSV or JSON
syntax.

## Fast table previews

Use a row limit and skip saved-view sorting for a file-manager preview:

```sh
tview --output table --sorted false --top-lines 30 data.csv
```

`-n 30` is the short form of `--top-lines 30`. The limit counts data rows after
source and view filters. The header and remaining-row summary are extra lines.
Omitting the limit keeps full output. The count must be a positive integer.

`--sorted true|false` defaults to `true`. Setting it to `false` skips saved
`view.sort` while keeping filters, formatting, column settings, and source
ordering. It does not remove sorting from a saved source query, SQL, or ES|QL.
These options apply only to direct table output, including automatic table
output when stdout is redirected. Tview rejects explicit use with interactive
mode, interactive export, JSON, or JSONL.

A truncated preview ends with `95 more rows...` when the exact number of omitted
filtered rows is already known. Otherwise it ends with `more rows...`. Tview
looks for one additional matching row but does not scan the rest just to count
it. The summary excludes rows outside the source-query limit.

Preview widths, inferred types, columns, and automatic color gradients use the
selected rows. Wider values and new fields in omitted rows do not change the
preview. Explicit widths and fixed color rules still apply. A requested full
schema scan still runs; use `--schema-scan default` to override one in a saved
view.

Saved sorting can require reading the full result. Selective filters can also
require a long scan to find enough matches. SQLite and Elasticsearch keep their
native query limits and may fetch a bounded response before rendering.

Stdin previews exit once enough rows and lookahead are available, even if the
producer has not closed its pipe. The producer may receive a broken pipe.
Previews do not validate unread trailing content. Errors encountered while
preparing the selected rows or lookahead leave stdout empty.

## JSON and JSONL

`--output json` emits one UTF-8 JSON document, followed by a newline:

```json
{"columns":["Name","Count"],"rows":[["alpha","2"],["beta","10"]]}
```

`--output jsonl` emits one JSON object per row, each followed by a newline:

```json
{"columns":["Name","Count"],"values":["alpha","2"]}
{"columns":["Name","Count"],"values":["beta","10"]}
```

Cells contain displayed strings. Native source types are not preserved.
Saved-view formatting, filtering, sorting, and hidden columns affect the result.
Width clipping, padding, control-character replacement, and ANSI styling do not.
JSON escapes embedded newlines and control characters. Ordered arrays retain
columns with duplicate names. `columns` is empty when the header is hidden or
absent; rows still contain positional values. Empty JSON results have an empty
`rows` array; empty JSONL results contain zero records. Column labels are
repeated in each JSONL record so each line describes itself.

The schemas are [JSON](../schemas/output.schema.json) and [JSONL
record](../schemas/output-record.schema.json). Additive fields may appear in
future versions; consumers should ignore unknown fields. Existing field meanings
and string cell types follow the compatibility policy below. Exporting native
types would require a separate documented format. Tview does not currently offer
TOON output.

Both formats reject `--color always`. Data goes to stdout and diagnostics go to
stderr. Structured batch output never prompts for source selection.

## Exit codes and partial output

| Code | Meaning |
|---|---|
| 0 | Success, help, version, normal interactive quit, or a consumer closing its pipe. |
| 1 | Source, configuration, runtime, output, or incompatible runtime-option error. |
| 2 | Command-line syntax or value rejected by the argument parser. |

OS signal termination retains OS-defined status. Cancelling an interactive
operation does not export an unfinished result. Broken pipes during writing or
flush count as success. Other write failures return 1 and may leave partial
bytes. A failure during source preparation produces no output. A process killed
during serialization can leave truncated JSON or JSONL; writes are not atomic.

Complete exports wait for source preparation and late schema discovery before
writing. Table previews use the bounded preparation described above. Complete
exports from stdin wait for EOF and can materialize the entire input. Some
sorts, filters, and exports can require full materialization even when initial
viewing was incremental. JSONL framing does not make ingestion bounded-memory.
Remote source limits and timeouts remain enforced.

## Writes and compatibility

The viewer reads source files and remote data. Saving a view writes local config
and prompts before overwriting it. Shell redirection opens its destination
before Tview runs; choose a different path from the input. Tview does not modify
source data.

Tview starts at 0.1.0. During 0.x, incompatible CLI or configuration changes
require a minor release and migration notes; compatible fixes use patch releases. Version 1.0.0 will establish the
supported public contract, after which incompatible changes require a major
release. Preserve existing defaults until a documented compatibility change. See
[migration](migration.md).
