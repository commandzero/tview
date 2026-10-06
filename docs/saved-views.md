---
type: Guide
title: Saved views
description: Save source options, column formatting, filters, sorting, and colors.
generated: { by: openai-codex/gpt-6.1-sol, at: 2026-10-05T04:25:43Z }
---

# Saved views

Tview loads YAML views from `$XDG_CONFIG_HOME/tview/views`, or
`~/.config/tview/views` when `XDG_CONFIG_HOME` is unset. Tview uses this path on
every platform, including macOS. Subdirectories are scanned recursively, so
bundles can live in folders such as `views/elasticsearch/`. Directory symlinks
are not followed. Files ending in `.yml` and `.yaml` are accepted.

View names remain file stems, regardless of their subdirectory. Keep stems unique
across bundles. For duplicates, `.yml` wins over `.yaml`; otherwise the first
path in lexical order wins. A footer warning is shown in interactive mode;
batch mode writes the warning to stderr.

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

Tview selects and validates a view once per invocation. Its source settings
and column presentation come from that same snapshot, even if the YAML changes
while the source opens. A new invocation discovers current files. `--no-view`
skips discovery and saved-view authoring; a missing forced `--view` fails before
opening the source.

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

Column keys match headers case-insensitively for delimited input. Exact keys
win over wildcard keys; wildcard ties use the most literal characters, then
lexical order. Metadata for a column, including its type and null placement,
binds before saved sorts and filters. The same rule applies when structured
columns are discovered later, so a late high-priority sort retains its order
and a saved filter does not get installed twice. Supported
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
operations belong under `view`. Explicit CLI source options override the saved
view, which overrides defaults; the selected view still supplies presentation
against the resulting schema. Supplying `--schema-scan default` therefore
overrides a saved `source.schema_scan: full` for one invocation. For object
tables, Tview saves `source.object_mode` as `record` or `entries` so later
detection changes do not change the rows. Non-object tables omit it.

Saving uses the complete source configuration of the last successfully
activated result. It retains applicable opening choices, the selected relation
or configured native base query, structured source operations, and its effective
finite limit. Draft, pending, failed, or superseded requests do not change that
source section; currently applied local settings still belong under `view`.
Native sources persist either `source.table` or `source.query`, never both.
Generated SQL or ES|QL and the extra-row limit probe are not saved as a
user-supplied base query. For a file source opened without a finite limit, the
limit remains omitted. Local filters do not fetch replacement source rows.

Reload reopens the committed source configuration and supersedes pending work.
It keeps compatible live view settings rather than rereading YAML or resetting
the view to its selected snapshot. Column settings follow unambiguous compatible
source identities, not column positions. Stable adapter-proven row identities
can preserve cursor and marks across a compatible replacement; otherwise
row-bound state resets. Reloading stdin remains a no-op; a reload error ends
the interactive session.

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
regardless of whether its display label is `name` or `_key`. Unambiguous source
labels remain a fallback; an ambiguous structured label never selects an
arbitrary column or operation. Missing canonical references remain pending while
the schema is provisional. When schema discovery completes, missing or ambiguous
references and invalid operations produce non-fatal, once-only warnings in the
TUI and on stderr, not in batch stdout. A present numeric filter can still await
the required numeric profile; definitive unavailability is reported as an
unavailable operation, not as a missing column. Local edits supersede pending
saved sort/filter intent; `--sorted false` suppresses saved view sorts, including
late sorts, but not source order, view filters, or formatting. A column can set
`label` without changing its canonical identity or raw data. View-level and
per-column `nulls: first|last` control direction-independent sort placement,
with the column policy winning over the view policy and `last` as the built-in
default.

## Conditional colors

Column color rules run in order; the first matching rule selects the foreground.
Match and range rules, fixed and automatic gradients, and identifier colors use
the same configured rules in the TUI and colored table output. No match keeps
the theme's ordinary cell foreground. YAML retains configured color strings,
not computed gradients, identifier indexes, or terminal colors. Colors do not
change values, sorting, filtering, search, copying, or popups.

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

Automatic gradients and identifiers use the applicable complete-result profile
when rendering the complete result, not only visible screen rows. In a table
preview, their profile uses emitted rows alone: rejected, lookahead, and omitted
rows do not change emitted foregrounds. The complete profile may use resident
rendered identifiers or exact store-backed raw identifiers, depending on the
source path; it does not broaden the source-result limit. Plain table, JSON,
and JSONL output do not request color-only profiling.

## Saving a view

Press `v` to inspect the generated YAML. In that modal, press `s` to save it to
the loaded view file, or to a placeholder file named from the current input with
only the last extension replaced by `.yml`. Existing files ask for `y`/`n`
confirmation. Saves are atomic and create the views directory as needed.

While a source replacement is pending or after one fails, saving keeps the
last successfully activated source configuration and the current applied local
view. It does not save the rejected query or mix source settings across results.

Use the [view schema](../schemas/view.schema.json) for editor validation. See
the [conditional-colors example](../examples/data/config/views/conditional-colors.yml)
for a complete view.
