---
type: Guide
title: Large files and schema discovery
description: Indexing, schema scan limits, and operations that read the full input.
generated: { by: openai-codex/gpt-6.1-sol, at: 2026-10-05T04:25:43Z }
---

# Large files and schema discovery

Tview indexes large seekable delimited and JSON/NDJSON inputs as you navigate.
CSV offsets come from the CSV parser, so quoted multiline records remain one row. Navigation requests
additional bounded ranges; drawing the table, opening a cell popup, and copying
a cell do not clone the full table.

JSON and TOON schema discovery examine up to 100 MiB of selected logical-row payload by
default, finishing the row crossing that boundary. If discovery stops at that
limit, newly encountered canonical paths append on the right, earlier rows
receive nulls, and existing labels and order remain fixed. Use
`--schema-scan full` when every eligible column and inferred source type must be known before
output. A full-schema table preview traverses the required active source
result even when the initial schema looks complete. Fields in accepted rows
beyond the emitted prefix can join its schema; rejected-only fields cannot.
Explicit null and empty structured fields still count as present, rather than
being mistaken for absent fields from their display strings. An established
delimited header keeps its existing empty-result policy.

Direct table previews prepare an immutable prefix, with at most one additional
matching row for remainder evidence on the streaming path. Default-scan preview
columns, widths, inferred types, and automatic gradients come from emitted
rows, not omitted or lookahead values. A complete output prepares all effective
rows; output serialization does not fetch more data. Early unsorted previews
may close stdin before EOF. A numeric remainder needs an exact effective total
already known; otherwise a confirmed extra match produces `more rows...`.
Neither an unread suffix nor rows outside a native source limit are counted
or validated solely for a streaming preview. TOON is an exception: it decodes and
validates the entire document into memory before any preview or source limit.
Its schema and object-detection byte budgets measure compact JSON-equivalent
logical payload, not compressed input bytes; those budgets do not bound decoding
memory or I/O.

Exact local sorting, filtering, maximum-width calculation, full auto-range
profiling, and similar whole-result operations may need to index or materialize
the selected table. Numeric view filters require whole-active-result profile
evidence even when only a short preview is emitted. Saved filters on late
columns may need deferred earlier rows to be rechecked. Costs grow with the
required source result; a native bounded response has no refill query after
local filtering. Stdin and encodings that cannot safely use byte offsets use
materialized storage.
