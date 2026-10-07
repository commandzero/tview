---
type: Guide
title: SQLite sources
description: Read-only database browsing, table selection, and source queries.
generated: { by: openai-codex/gpt-6.1-sol, at: 2026-10-07T03:44:18Z }
---

# SQLite sources

```sh
tview examples/data/us-counties.sqlite3
tview examples/data/us-counties.sqlite3 --format sqlite --table counties
```

Tview opens local SQLite files read-only through Turso. It selects the only
ordinary table or compatible ordinary view automatically. With several
relations, the viewer opens a picker; batch output needs `--table <name>`,
a saved `source.table`, or `--query`.

SQLite support is enabled by default. Stdin, Turso Cloud, `libsql://`, and
other remote SQLite URLs are unsupported; `libsql://` is reserved. See
[installation](installation.md) for feature selection.

## Source queries and local view

Source queries return at most 1,000 rows by default. Source filters and SQLite
sorts run before the limit. Local view filters, sorts, search, formatting, and
hiding act on returned rows without fetching replacements.

Press `u` to change the source limit, filters, and sort; press `V` for local
view settings or `i` for a column. The `f`/`F` and sort keys change only the
local view. Failed source changes leave the previous rows and query in place.
Saving keeps the last successful source query. Reload (`r`) reopens that query
and keeps compatible local settings, but a reload error ends the session.
See [saved views](saved-views.md#source-settings) for saving and reload rules.

## Custom SQL

`--query` accepts one read-only, row-producing statement. SELECT and CTE
queries are wrapped so source filters, sorts, and the hard limit still apply.
Multiple statements, writes, state-changing pragmas, unbound parameters, and
non-tabular statements are rejected. The database also uses storage-level
read-only flags.

Press `p` to inspect and copy the effective SQLite `SELECT` with parameter
values. The popup shows typed parameters and lists local view operations
separately. Its query omits the extra row requested to detect truncation.

## Read-only access and supported tables

Browsing does not switch a rollback-journal database to WAL, create sidecar
files, or change database or sidecar bytes. Source filters use bound parameters.
Tview lists ordinary tables and compatible ordinary views, but not virtual
tables such as FTS5 and RTree, shadow tables, or SQLite-internal tables.

Declared column types provide initial hints; runtime values can widen them.
Tview can keep cursor and marks across queries for tables with rowids or
declared primary keys. It resets them for views and tables without stable keys.

Try the [sample database](../examples/data/us-counties.sqlite3); its [source
and column selection](../examples/data/README.md) is documented with the data.
