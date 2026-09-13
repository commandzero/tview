---
type: Guide
title: Large files and schema discovery
description: Indexing, schema scan limits, and operations that read the full input.
generated: { by: codex/gpt-6, at: 2026-09-12T17:14:16Z }
---

# Large files and schema discovery

Tview indexes large seekable inputs as you navigate. CSV offsets come from the
CSV parser, so quoted multiline records remain one row. Navigation requests
additional bounded ranges; drawing the table, opening a cell popup, and copying
a cell do not clone the full table.

JSON schema discovery examines up to 100 MiB of selected logical-row payload by
default and finishes the row crossing that boundary. If discovery stops at that
limit, newly encountered canonical paths append on the right, earlier rows
receive nulls, and existing labels and order remain fixed. Use `--schema-scan full` when every column and inferred source type must be known before the table
appears.

Exact sorting, filtering, maximum-width calculation, full auto-range profiling,
and similar whole-dataset operations may need to index or materialize the
selected table. Their cost grows with the complete source even when initial
opening was bounded. Stdin and encodings that cannot safely use byte offsets use
materialized storage.
