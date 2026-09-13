---
type: Guide
title: File input
description: Delimited text, JSON, NDJSON, format detection, and nested data.
generated: { by: codex/gpt-6, at: 2026-09-12T17:14:16Z }
---

# File input

Open a file or use `-` to read stdin:

```sh
tview data.csv
tview - < data.csv
tview records.ndjson --format ndjson
```

## Delimited text

Set the delimiter, encoding, or quoting rules when detection is insufficient:

```sh
tview data.tsv --delimiter '\t' --quoting QUOTE_NONE
tview data.csv --encoding iso8859-1
tview data.csv --start_pos 6,5
tview data.csv +6:5
tview data.csv --encoding iso8859-1 +6:
tview data.csv --width mode
tview data.csv --width max
tview data.csv --width 20
```

Use Tview as the MySQL pager by adding these options to `~/.my.cnf`:

```ini
pager=tview -d '\t' --quoting QUOTE_NONE -
silent
```

## JSON and NDJSON

```sh
tview response.json --json-path /hits/hits
tview repositories.json --object-mode entries
tview settings.json --object-mode record
tview response.data --format json --schema-scan full
```

`--object-mode auto|record|entries` controls how a selected JSON object becomes
rows. `record` opens the object as one row. `entries` opens each direct member
as a row, preserving source order; the member name is a synthetic first text
column with canonical identity `@key`. It is not valid for arrays, scalars,
delimited input, or NDJSON row streams.

`auto`, the default, detects entries only when the bounded sample has at least
three members, every sampled value is an object, and at least 75 percent share a
direct child field with the same value kind. Detection examines at most 64
entries or 1 MiB, finishing the entry that crosses the byte bound. Use explicit
`record` or `entries` for reproducible scripts and saved views. An explicit mode
overrides automatic detection.

### Selecting nested data

`--json-path` uses RFC 6901 JSON Pointer, not JSONPath, and selection happens
before object-mode resolution. For example, `--json-path /hits/hits` selects
Elasticsearch search hits while ignoring response metadata. Selected arrays
remain rows. For NDJSON the pointer is resolved in each complete document and
the selected object or array remains that document's single row.

Nested objects are flattened to canonical row-relative pointers. Nested arrays
remain atomic JSON cells. Native null, boolean, integer, floating-point, and
text values remain distinct. JSON `null` differs from an empty string.

## Format detection

`--format auto|delimited|json|ndjson|sqlite|elasticsearch` defaults to `auto`.
An unambiguous URL scheme can select a source format: `libsql://` resolves to
SQLite and `file://` resolves to a local path. HTTP and HTTPS URLs require
`--format elasticsearch`; Tview does not fetch remote content to guess its type.
Tview checks filename extensions before probing a bounded sample; SQLite's
`SQLite format 3` signature is recognized before any text decoding. An explicit
format always wins. Delimited-only options imply delimited input under `auto`
unless the input has a SQLite signature, and are rejected for SQLite and
explicitly selected structured formats.

See [large files](large-files.md) for indexing and schema scan limits, [saved
views](saved-views.md) for reusable settings, and the [CLI
contract](cli-contract.md) for output formats and pipes.
