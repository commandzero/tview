---
type: Guide
title: SQLite sources
description: Read-only database browsing, table selection, and source queries.
generated: { by: codex/gpt-6, at: 2026-09-12T17:14:16Z }
---

# SQLite sources

```sh
tview sample/us-counties.sqlite3
tview sample/us-counties.sqlite3 --format sqlite --table counties
```

Tview opens local SQLite files through Turso. It selects the table or compatible
ordinary view automatically when only one is available. With several choices,
the viewer opens a table picker. Batch output requires `--table <name>` or a
saved `source.table` unless a query selects the data.

SQLite input requires a local file. Stdin, Turso Cloud, `libsql://`, and other
remote SQLite URLs are unsupported. The `libsql://` scheme is reserved for
future use. SQLite support is enabled by default. See
[installation](installation.md) for feature selection.

## Source queries and local view

SQLite source queries return at most 1,000 rows by default. Source filters and
SQLite sorting run before that limit. View filters, local sorts, search,
formatting, and hiding operate on the returned rows without fetching
replacements.

Press `u` for Source Configuration to stage a query's limit, filters, and sort.
Press `V` for View Configuration or `i` for the current column's settings. The
`f`/`F` and sort keys affect the local view only.

## Custom SQL

`--query` accepts one read-only, row-producing SQLite statement. SELECT and CTE
queries are composed as a derived table so source filters, source sorting, and
the hard limit remain enforced. Multiple statements, writes, state-changing
pragmas, unbound parameters, and non-tabular statements are rejected while the
database remains opened with storage-level read-only flags.

Press `p` to open the Query modal. It shows the parameterized SQLite `SELECT`,
typed parameters, and a copyable statement with parameter values filled in. It
lists local view operations separately because they do not run in SQL. The
displayed query omits the extra row Tview requests to detect truncation.

## Read-only access and supported tables

Tview opens the database read-only. Browsing does not switch a rollback-journal
database to WAL, create sidecar files, or change existing database or sidecar
bytes. Source filters use bound parameters.

Tview lists ordinary tables and compatible ordinary views. It reports virtual
tables, including FTS5 and RTree, as unsupported and hides shadow and
SQLite-internal tables. See [Turso implementation
details](turso-build-impact.md) for the connection safeguards and dependency
choices.

Declared column types appear as source metadata and provide initial type hints.
SQLite values remain dynamically typed at runtime, and observed integer, real,
text, blob, and null values can widen those hints. For tables with rowids or
declared primary keys, Tview can track the cursor and marks across queries. It
resets that state for views and tables without a stable key.

The [sample database](../sample/us-counties.sqlite3) contains 1,000 county
records. Its [sample guide](../sample/README.md) records the Census Bureau
source and column selection.
