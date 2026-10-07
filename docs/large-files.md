---
type: Guide
title: Large files and schema discovery
description: Indexing, schema scan limits, and operations that read the full input.
generated: { by: openai-codex/gpt-6.1-sol, at: 2026-10-07T03:44:18Z }
---

# Large files and schema discovery

Tview indexes seekable delimited and JSON/NDJSON inputs as you navigate. Quoted
multiline CSV records remain one row. Navigation indexes additional ranges;
drawing the table, opening a cell popup, and copying a cell do not clone it.

JSON and TOON discover schema from up to 100 MiB of selected logical-row
payload by default, finishing the row that crosses the bound. Late paths append
on the right; earlier rows get nulls, while existing labels and order stay
fixed. `--schema-scan full` scans the required active result for every eligible
column and its source type, even for a short preview. Accepted rows beyond
the preview can add fields; rejected and out-of-limit rows cannot. Explicit
null and empty fields count as present. Delimited headers keep their
empty-result behavior. `--schema-scan default` overrides a saved full scan.

Automatic JSON/TOON object-mode selection examines at most 64 entries or 1 MiB
of logical entry payload, finishing the entry that crosses the byte bound. It
chooses `entries` only with at least three sampled members, all sampled values
objects, and at least 75 percent sharing a direct child field of the same kind.
Use `--object-mode record|entries` to avoid sample-dependent row shapes.

Direct table previews fix rows and presentation before writing. Streaming
previews read at most one extra matching row to detect more rows. With default
schema discovery, columns, widths, types, and automatic gradients use emitted
rows only; explicit widths and fixed color rules still apply. A known filtered
total yields an exact remainder. Otherwise an extra match confirms
`more rows...`. Unsorted default-scan stdin previews can stop before EOF,
leaving unread content uncounted and unvalidated.

Complete output prepares all effective rows before writing, and stdin exports
wait for EOF. JSONL does not make ingestion bounded-memory. TOON always
decodes and validates the entire document in memory before any preview or
source limit. Its schema and object-detection budgets measure compact
JSON-equivalent logical payload, not compressed bytes or memory use.

Exact local sorting and filtering, maximum-width calculation, and full
auto-range profiling can require the whole selected table, even for a short
preview. Numeric view filters need a whole-result profile. Saved filters on
late columns can recheck earlier rows. Bounded native results do not refill
after local filtering. Stdin and encodings without safe byte offsets use
materialized storage.

Locating nested data and late columns may also take long scans. SQLite and
Elasticsearch can fetch a bounded response before rendering; native limits
and timeouts still apply. See [file input](file-input.md) for preview commands
and [saved views](saved-views.md) for source and local filters.
