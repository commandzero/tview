---
type: Guide
title: Saved views
description: Save source options, column formatting, filters, sorting, and colors.
generated: { by: codex/gpt-6, at: 2026-09-12T17:14:16Z }
---

# Saved views

Tview loads YAML views from `$XDG_CONFIG_HOME/tview/views`, or
`~/.config/tview/views` when `XDG_CONFIG_HOME` is unset. Tview uses this path on
every platform, including macOS. Files ending in `.yml` and `.yaml` are
accepted. If both `name.yml` and `name.yaml` exist, `.yml` wins and a footer
warning is shown in interactive mode; batch mode writes the warning to stderr.

Views match the input basename. Remote endpoints use a name such as
`https_elastic.example_9200` to distinguish hosts without storing credentials,
query strings, or fragments. Filename patterns can be exact strings, globs
containing `*`, `?`, or `[`, or regexes that start with `^` or end with `$`.
Exact matches win before globs, then regexes. Use `--view <name>` to force a
view by file stem, or `--no-view` to disable loading and saving for that run.

```sh
tview data.csv --view my-view
tview data.csv --no-view
```

## Column settings

Set only the column properties you want to override:

```yaml
name: cat-shards
filenames:
  - cat_shards.txt
source: {}
view:
  nulls: last
  columns:
    shard:
      type: integer
      width: header
      align: left
      nulls: first
    "*count":
      type: integer
      format: locale
      width: content
    segment:
      type: text
      visible: false
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

A keyed-object view can pin its row shape and address the synthetic key column
independently of its display label:

```yaml
name: repositories
filenames: [repositories.json]
source:
  format: json
  object_mode: entries
view:
  columns:
    "@key":
      label: Repository
```

Column keys match headers case-insensitively. Exact keys win over wildcard keys;
wildcard ties use the most literal characters, then lexical order. Supported
type aliases are `string`, `text`, `date`, `ip`, `number`, `float`, `integer`,
`semver`, `boolean`, `char`, `bit`, and `word`. Formats include `plain`,
`locale`, `mask`, `uppercase`, `lowercase`, `char`, `bit`, and `word`. Number
masks support `0`, `0.00`, `#,##0`, and `#,##0.00` forms. `locale` uses the
system POSIX locale with `en_US` fallback, or `view.locale`. Headers are
prefixed first with sort state, then filter state: `▲` for ascending sort, `▼`
for descending sort, `+` for filter-in, `-` for filter-out, and `±` for multiple
filters. Truncation applies after those prefix markers.

## Source settings

Tview applies `source` settings before opening the table. Formatting and local
operations belong under `view`. Explicit CLI options override the saved view,
which overrides defaults. Supplying `--schema-scan default` therefore overrides
a saved `source.schema_scan: full` for one invocation. For object tables, Tview
saves `source.object_mode` as `record` or `entries` so later detection changes
do not change the rows. Non-object tables omit it. Native sources may persist
either `source.table` or `source.query`, never both. For Elasticsearch,
`source.query` stores only the configured ES|QL base text; Tview stores filters
and sorting separately and omits the extra-row limit probe.

For delimited, JSON, and NDJSON sources, saved source filters stream decoded
logical records before the source limit. Use `column: "*"` for a grep-style
whole-record filter. Quoted multiline CSV fields remain part of one logical
record. File sources do not offer source sorting because it would require
loading the full input. Use `view.sort` to sort the bounded result.

## Column identity

SQLite column keys use the source column name when it is unique. Duplicate names
use occurrence keys such as `name#1` and `name#2`. Tview rejects an unsuffixed
duplicate because it cannot identify one column.

Structured column configuration should use exact, case-sensitive canonical JSON
Pointers such as `/_source/user/email`; keyed-object member names use `@key`,
regardless of whether its display label is `name` or `_key`. An unambiguous
compact display label is accepted as a fallback. A column can set `label`
without changing its canonical identity or raw data. View-level and per-column
`nulls: first|last` control direction-independent sort placement, with the
column policy winning over the view policy and `last` as the built-in default.

## Conditional colors

Column color rules run in order. The first match sets the cell style. Colors do
not change values, sorting, filtering, search, copying, or popups.

```yaml
view:
  columns:
    active:
      type: boolean
      colors:
        - match:
            true: green
            false: muted
    prirep:
      type: string
      colors:
        - match:
            p: darkgreen
            r: blue
    used_percent:
      type: number
      colors:
        - range:
            "<10": red
            ">=90": red
        - gradient:
            mode: auto
            steps: 8
            colors: [green, yellow]
    latency_ms:
      type: number
      colors:
        - gradient:
            mode: fixed
            stops:
              0: green
              100: yellow
              500: red
    ip_address:
      type: ip
      colors:
        - identifiers:
            colors: auto
    host:
      type: string
      colors:
        - identifiers:
            colors: [cyan, "palette(198)", "#25A39AFF"]
```

The `identifiers` rule is for string-like discrete values. It assigns each
unique rendered value in the column, such as an IP address or host name, to a
stable generated color. `colors: auto` uses the active theme's
`[identifiers].colors` families; a view can override those families with a color
array. Each family generates 16 dark-to-light shades, and identifiers cycle
across families before advancing shades. The darkest shade matches the family's
ANSI dark/dim foreground color or a brighter value, so it is never darker than
that color.

## Saving a view

Press `v` to inspect the generated YAML. In that modal, press `s` to save it to
the loaded view file, or to a placeholder file named from the current input with
only the last extension replaced by `.yml`. Existing files ask for `y`/`n`
confirmation. Saves are atomic and create the views directory as needed.

Use the [view schema](../schemas/view.schema.json) for editor validation. See
the [conditional-colors example](../sample/config/views/conditional-colors.yml)
for a complete view.
