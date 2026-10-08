---
type: Guide
title: Saved views
description: Save source options, column formatting, filters, sorting, and colors.
generated: { by: openai-codex/gpt-6.1-sol, at: 2026-10-07T03:44:29Z }
---

# Saved views

Tview reads `.yml` and `.yaml` views recursively from
`$XDG_CONFIG_HOME/tview/views`, or `~/.config/tview/views` when
`XDG_CONFIG_HOME` is unset. This location also applies on macOS. Directory
symlinks are not followed. Put related views in subdirectories if useful.

The file stem is the view name, even in a subdirectory. Keep stems unique. If
they clash, `.yml` wins over `.yaml`; otherwise the first path in lexical order
wins. Tview warns in the interactive footer or on batch stderr.

Views match the input basename. A remote endpoint uses a name such as
`https_elastic.example_9200`, without credentials, query strings, or fragments.
`filenames` accepts exact names, globs containing `*`, `?`, or `[`, and regexes
that start with `^` or end with `$`. Exact matches take precedence over globs,
then regexes. Force a file stem with `--view <name>` or disable view loading and
saving with `--no-view`:

```sh
tview data.csv --view my-view
tview data.csv --no-view
```

Tview selects one view for the invocation. A missing forced view fails before
opening the source. Start another invocation to pick up changes to the YAML.

### Elastic CLI context matching

Context sources match their full canonical reference, not an input basename or
the generated YAML filename. `.production.es://logs-*` and
`.production.elasticsearch://logs-*` both use
`.production.elasticsearch://logs-*`. The rightmost segment selects the
service; preceding segments keep the exact context name, so
`.production.us-west.es://` selects `production.us-west`. The literal suffix
is part of the matching identity. Matching does not resolve the endpoint or
run credential resolvers.

Generated filenames use a filesystem-safe form of that identity. Keep the
full canonical reference in `filenames`, rather than copying the safe filename:

```yaml
name: production-logs
filenames: [".production.elasticsearch://logs-*"]
source:
  format: elasticsearch
  table: "logs-*"
  limit: 1000
view: {}
```

This uses existing view fields. YAML contains no resolved endpoint,
authentication, or resolver values. A current-context reference is saved as
`.elasticsearch://`, not the concrete context name. A fresh invocation follows
the then-current context; reload in the original invocation stays on its
resolved connection. Named references remain named.

## Column settings

Set only the properties you want to override. This view gives a column a type,
formats another, and applies a local sort and filter:

```yaml
name: cat-shards
filenames: [cat_shards.txt]
source: {}
view:
  columns:
    shard:
      type: integer
      width: header
    "*count":
      type: integer
      format: locale
  sort:
    - column: shard
      direction: asc
      kind: numeric
  filters:
    - column: "*count"
      action: in
      kind: numeric
      condition: ">0"
```

Delimited headers match case-insensitively. Exact column keys beat wildcard
keys; wildcard ties use the most literal characters, then lexical order.
Column types and null placement apply before saved sorts and filters, including
when structured columns arrive later. Each saved filter is installed only once.
Local edits supersede pending saved sorts and filters. `--sorted false`
suppresses saved view sorts, including late sorts, but not source order,
filters, or formatting.

`view.nulls` and per-column `nulls: first|last` set sort placement regardless
of direction. The column setting wins; `last` is the default. `view.locale`
overrides the system POSIX locale for `format: locale`, whose fallback is
`en_US`. See the [view schema](../schemas/view.schema.json) for types, formats,
widths, and number masks.

Headers show sort state before filter state: `▲` means ascending, `▼`
descending, `+` filter-in, `-` filter-out, and `±` multiple filters. Markers
are added before header truncation.

## Source settings

Tview applies `source` settings before opening a table; `view` settings
format, filter, and sort the resulting rows. Explicit CLI source options
override saved options, which override defaults. For example,
`--schema-scan default` overrides saved `source.schema_scan: full`.
`source.object_mode: record|entries` pins how an object becomes rows.

Saving uses the last successfully activated source configuration, including
its effective finite limit, along with the current local view settings.
Pending or failed requests do not replace saved source settings. Native
sources save either `source.table` or `source.query`, not generated SQL or
ES|QL. A file source opened without a finite limit leaves the limit unset.
Reload reopens that committed source configuration while keeping compatible
live view settings; it does not reread the saved YAML. Reloading stdin does
nothing, and a reload error ends the interactive session.

For context sources, a non-empty positional suffix acts as the initial
`--table` selection and overrides a saved table or query. It conflicts with
explicit `--table` or `--query`. An empty suffix preserves normal CLI-over-saved
selection and interactive discovery. These are startup rules: a later
successfully committed source-query replacement becomes the configuration
used for saving and reload, without reapplying the initial suffix. Discovery,
replacement queries, and reload all reuse the context connection resolved for
the invocation. Restart Tview to refresh the endpoint or credentials.

Pending and failed replacements still leave the last successful source
settings intact. The [latest-failure export rule](cli-contract.md#interactive-mode-and-output)
also remains unchanged.

For files, source filters run on decoded records before the source limit.
Delimited, JSON, and NDJSON readers stream records; TOON validates the whole
document. Use `column: "*"` for a whole-record filter. File sources do not
support source sorting; use `view.sort` to sort the bounded result. Local
filters do not fetch replacement source rows. See [file input](file-input.md)
and [large files](large-files.md) for input and scan limits.

## Column identity

SQLite uses the source column name when unique. Duplicates need occurrence
keys such as `name#1` and `name#2`; an unsuffixed duplicate is rejected.

Structured columns use exact, case-sensitive JSON Pointers such as
`/_source/user/email`. Keyed-object members use `@key`, regardless of their
display label. An unambiguous source label is also accepted, but an ambiguous
label never selects a column or operation. Changing a column's `label` does
not change its identity or raw value.

Missing references can wait until schema discovery finishes. Tview then
reports missing or ambiguous references and invalid operations once, without
stopping the session. Warnings appear in the TUI or on stderr, not batch
stdout. A numeric filter can wait for a column's numeric profile; if that
profile is unavailable, Tview warns about the operation rather than a missing
column.

## Conditional colors

Column color rules run in order. The first match sets the foreground; no
match leaves the theme's cell color. Rules support matches, ranges, fixed or
automatic gradients, and identifier colors:

```yaml
view:
  columns:
    active:
      type: boolean
      colors:
        - match:
            true: green
            false: muted
    used_percent:
      type: number
      colors:
        - range:
            ">=90": red
        - gradient:
            mode: auto
            steps: 8
            colors: [green, yellow]
```

Identifiers color distinct rendered values in a column. `colors: auto`
uses the active theme's `[identifiers].colors` families, or a view can supply
its own array. Each family produces 16 dark-to-light shades; values cycle
across families before moving to lighter shades. See [color
themes](themes.md) and the [complete conditional-colors
example](../examples/data/config/views/conditional-colors.yml) for other rules.

The TUI and colored table output use the same rules. Automatic gradients and
identifiers use the full result, or only emitted rows in a table preview.
They do not extend the source limit. Colors do not alter values, sorting,
filtering, search, copying, or popups. Plain table, JSON, and JSONL output do
not profile rows solely for colors.

## Saving a view

Press `v` to inspect generated YAML, then `s` in the modal to save it. Tview
saves to the loaded view file or a file named from the input with only its last
extension replaced by `.yml`. Context sources instead use a filesystem-safe
canonical reference name. It asks for `y`/`n` before overwriting a file.
Saves are atomic and create the views directory if needed.

Use the [view schema](../schemas/view.schema.json) for editor validation and
the [conditional-colors example](../examples/data/config/views/conditional-colors.yml)
for a complete view.
