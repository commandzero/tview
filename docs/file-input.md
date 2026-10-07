---
type: Guide
title: File input
description: Delimited text, JSON, NDJSON, TOON, format detection, and nested data.
generated: { by: openai-codex/gpt-6.1-sol, at: 2026-10-07T03:44:18Z }
---

# File input

Open a file or use `-` to read stdin:

```sh
tview data.csv
tview - < data.csv
tview records.ndjson --format ndjson
```

## Delimited text

Set the delimiter, encoding, quoting, start position, or column width when
detection is insufficient:

```sh
tview data.tsv --delimiter '\t' --quoting QUOTE_NONE
tview data.csv --encoding iso8859-1 +6:5
tview data.csv --width mode
```

`--start_pos 6,5` is equivalent to `+6:5`; `+6:` starts at row 6. Width can
also be `max` or a number such as `20`. To use Tview as the MySQL pager, add
`pager=tview -d '\t' --quoting QUOTE_NONE -` and `silent` to `~/.my.cnf`.

## JSON, NDJSON, and TOON

```sh
tview response.json --json-path /hits/hits
tview repositories.json --object-mode entries
tview settings.json --object-mode record
```

`--object-mode auto|record|entries` sets the row shape of a selected JSON or
TOON object. `record` makes one row from the object. `entries` makes one row per
direct member in source order, with the member name in a synthetic first text
column whose canonical identity is `@key`. Arrays, scalars, delimited files,
and NDJSON row streams do not support `entries`.

The default, `auto`, chooses `entries` when sampled members look like records;
otherwise it uses `record`. Set the mode explicitly for reproducible scripts
and saved views. See [large files](large-files.md) for the sample bounds.

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
tview examples/data/forecasts.toon --json-path /forecast
tview --format toon - < records.toon
```

TOON uses the [4.1 specification](https://github.com/toon-format/spec/blob/v4.1.1/SPEC.md)
in strict mode with two-space indentation. It accepts UTF-8, an initial BOM,
CRLF, full-line comments, keyed tabular objects, and nested header field groups.
Invalid UTF-8, indentation, duplicate keys or header fields, declared counts,
and row widths cause errors before output. The selected table must be an object
or array, not a root scalar.

TOON labels retain nested context: `temp{min,max}` displays as `temp.min` and
`temp.max`, with canonical keys `/temp/min` and `/temp/max`. If a later field
would reuse an existing label, it uses its canonical pointer instead; existing
labels stay fixed. JSON and NDJSON use compact labels.

Signed 64-bit integers remain exact. Larger integers and decimal numbers use
floating-point approximation; very small exponents can underflow to zero.
Nonfinite overflow is rejected. Quote numbers to preserve them as text.

TOON validates the complete document before previews or source limits and
decodes it into memory. See [large files](large-files.md) for schema discovery
and scan costs.

## Format detection

`--format auto|delimited|json|ndjson|toon` defaults to `auto`. Builds with the
`sqlite` or `elasticsearch` feature also accept the corresponding format. An
explicit format wins. For structured stdin, set `--format json`, `ndjson`, or
`toon`: under `auto`, batch stdin is delimited unless a structured option or
saved format selects structured input.

Under `auto`, a `.toon` extension selects TOON unless delimited-only options
override it. Tview does not guess TOON from extensionless text. Extensions take
precedence over a bounded probe; the SQLite file signature is recognized before
text decoding. Delimited-only options imply delimited input under `auto` except
for SQLite files, and conflict with SQLite or an explicitly structured format.
An unambiguous URL scheme can select a source. `file://` resolves to a local
path; `libsql://` is reserved and unsupported. HTTP and HTTPS require
`--format elasticsearch`; Tview does not fetch URLs to detect a format.

## Table previews

`-p` previews rows in source order. A count after `-t` applies saved-view
sorting before selecting rows:

```sh
tview -p 10 data.csv
tview -t 10 data.csv
```

Both produce colored tables even when redirected. Without a saved sort, both
retain source order. Source sorting and filters still apply. For a plain
preview without saved sorting, use:

```sh
tview --output table --sorted false --top-lines 10 data.csv
```

`--top-lines`, or `-n`, requires a positive count. `--sorted false` skips saved
`view.sort`, not filters, formatting, column settings, or source sorting. These
options apply only to direct table output, including automatic output on
redirected stdout.

`-p` requires a count and cannot combine with `-t`, `--output`, `--color`,
`--sorted`, `--top-lines`, or `--interactive`. Bare `-t` writes the full table
and can combine with `--top-lines`, `--sorted`, or `--interactive`; `-it` exports
a colored table on quit. `-t` cannot combine with explicit `--output` or
`--color`. Counted `-t` implies `--sorted true` and `--top-lines`, so it cannot
combine with either option or interactive mode.

Attached counts such as `-p10`, `-t10`, `-t=10`, and `--table-color=10` work. A
separate decimal token after `-t` counts rows, not a filename. Use `-t -- 10`
or `-t ./10` to open a file named `10`.

See [large files](large-files.md) for indexing and schema scan limits, [saved
views](saved-views.md) for reusable settings, and the [CLI
contract](cli-contract.md) for output formats and pipes.
