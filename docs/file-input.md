---
type: Guide
title: File input
description: Delimited text, JSON, NDJSON, TOON, format detection, and nested data.
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

## JSON, NDJSON, and TOON

```sh
tview response.json --json-path /hits/hits
tview repositories.json --object-mode entries
tview settings.json --object-mode record
tview response.data --format json --schema-scan full
```

`--object-mode auto|record|entries` controls how a selected JSON or TOON object
becomes rows. `record` opens the object as one row. `entries` opens each direct member
as a row, preserving source order; the member name is a synthetic first text
column with canonical identity `@key`. It is not valid for arrays, scalars,
delimited input, or NDJSON row streams.

`auto`, the default, detects entries only when the bounded sample has at least
three members, every sampled value is an object, and at least 75 percent share a
direct child field with the same value kind. Detection examines at most 64
entries or 1 MiB of logical entry payload, finishing the entry that crosses the byte bound. Use explicit
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
text values remain distinct. Structured `null` differs from an empty string.

### TOON 4.1

```sh
tview records.toon
tview response.toon --json-path /items
tview examples/data/forecasts.toon --json-path /forecast
tview --format toon - < records.toon
```

TOON reading follows the [4.1 specification](https://github.com/toon-format/spec/blob/v4.1.1/SPEC.md)
in strict mode with two-space indentation. It accepts UTF-8, an initial BOM,
CRLF, full-line comments, keyed tabular objects, and nested header field groups.
Invalid UTF-8, indentation, duplicate keys or header fields, declared counts,
and row widths are errors before output. The selected table must be an object
or array; root scalars are not tabular sources.

TOON column labels retain their complete nested context relative to the selected
table: `temp{min,max}` displays as `temp.min` and `temp.max`, including fields
discovered after the initial schema scan. Canonical column keys remain
`/temp/min` and `/temp/max`. JSON and NDJSON keep their compact label policy.
If a later field would reuse an existing label, such as the synthetic entry-key
column's `name`, that field uses its canonical pointer label (`/name`) instead.
Previously assigned labels stay fixed.

Signed 64-bit integers remain exact. Larger integers and decimal numbers use
floating-point approximation; very small exponents can underflow to zero.
Nonfinite numeric overflow is rejected. Quote numbers to preserve them as text.

TOON reads and validates the complete document before source limits or previews
are applied; it has no incremental or lazy reader. Schema discovery and automatic
object detection measure compact JSON-equivalent logical payload bytes, not the
compressed TOON file size. The schema scan limit is not an input-memory limit.

Preview columns still follow the shared structured-table rules: a bounded preview
uses fields present in its accepted prefix. With `--schema-scan full`, fields from
later accepted rows are included only within the source limit; fields found only
in rejected rows are not displayed. Full-document validation does not expose
those rejected or out-of-limit fields.

## Format detection

`--format auto|delimited|json|ndjson|toon` is always available. Builds with the
`sqlite` Cargo feature also accept `--format sqlite`; builds with the
`elasticsearch` feature also accept `--format elasticsearch`. The default is
`auto`.
Batch stdin is treated as delimited under `auto` unless an explicit or saved
structured format, or a structured option such as `--json-path`, selects
structured input. Use `--format json`, `--format ndjson`, or `--format toon` for
structured stdin. Under `auto`, a `.toon` extension selects TOON unless
delimited-only options override it; ambiguous extensionless text is not probed
as TOON.
An unambiguous URL scheme can select a source format: `libsql://` is recognized
as SQLite but remains unsupported and reserved for future use; `file://`
resolves to a local path. HTTP and HTTPS URLs require `--format elasticsearch`;
Tview does not fetch remote content to guess its type.
Tview checks filename extensions before probing a bounded sample; SQLite's
`SQLite format 3` signature is recognized before any text decoding. An explicit
format always wins. Delimited-only options imply delimited input under `auto`
unless the input has a SQLite signature, and are rejected for SQLite and
explicitly selected structured formats.

See [large files](large-files.md) for indexing and schema scan limits, [saved
views](saved-views.md) for reusable settings, and the [CLI
contract](cli-contract.md) for output formats and pipes.
