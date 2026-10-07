---
type: Guide
title: CLI output and compatibility
description: Output formats, stream behavior, exit codes, and compatibility guarantees.
generated: { by: openai-codex/gpt-6.1-sol, at: 2026-10-07T03:40:18Z }
---

# CLI output and compatibility

This reference defines what scripts can rely on when running Tview. See
[file input](file-input.md#table-previews) for preview commands and
[large files](large-files.md) for scanning and memory costs.

## Interactive mode and output

By default, Tview opens the viewer when stdout is a terminal and writes a text
table when stdout is redirected or piped.

| Options | Behavior |
| --- | --- |
| `--interactive`, `-i` | Open the viewer without exporting. |
| `--output table`, `-o table` | Write a fixed-width text table. |
| `--table-color [TOP_LINES]`, `-t [TOP_LINES]` | Write a colored table, optionally limited after saved-view sorting. |
| `--preview <TOP_LINES>`, `-p <TOP_LINES>` | Write a colored preview without saved-view sorting. |
| `--output json` or `--output jsonl` | Write structured output. |
| `--interactive --output <format>` | Export the final view after a normal quit. |

Exports include formatting, filters, sorting, and visible columns. Interactive
stdin sessions use the controlling terminal for keyboard input and display.
Complete exports wait for EOF and late columns. Interactive exports also wait
for pending source replacement. If the latest activation fails, Tview reports
the error on stderr, leaves stdout empty, and exits with code 1 rather than
exporting the earlier result. A newer successful activation replaces older failures.

## Text tables

Tables do not limit their total width to the terminal. `--width` and saved fixed
widths can clip cells. Batch output never enters raw mode or the alternate screen.
`--color auto` and `--color never` write plain text; `--color always` uses ANSI
styling from the active theme. Table output does not preserve CSV or JSON syntax.

Preview limits count data rows after filters and enabled saved-view sorting.
Headers and remainder summaries are extra lines. Tview reports an exact
remaining count only when it knows the filtered total. Otherwise, an additional
matching row confirms `more rows...`. The summary excludes rows outside native
source limits, and local filters do not fetch replacement rows.

Streaming previews may stop before EOF and leave trailing content unvalidated.
The producer may receive a broken pipe. TOON always validates the entire input.
Sorting, numeric filters, and full schema scans can require reading the full
result even for a short preview. See [large files](large-files.md).

## JSON and JSONL

`--output json` writes one UTF-8 document followed by a newline:

```json
{"columns":["Name","Count"],"rows":[["alpha","2"],["beta","10"]]}
```

`--output jsonl` writes one object per row, each followed by a newline:

```json
{"columns":["Name","Count"],"values":["alpha","2"]}
{"columns":["Name","Count"],"values":["beta","10"]}
```

Cells are displayed strings, not native source values. Saved-view settings affect
the result; width clipping, padding, control-character replacement, and ANSI
styling do not. JSON escapes embedded newlines and control characters.

Ordered arrays preserve duplicate column names. Hidden or absent headers produce
an empty `columns` array while rows retain positional values. Empty JSON results
have an empty `rows` array; empty JSONL results contain no records.

See the [JSON schema](../schemas/output.schema.json) and
[JSONL record schema](../schemas/output-record.schema.json). Consumers should
ignore unknown fields added in future versions. Tview does not offer TOON output.

## Streams and failures

Data goes to stdout and diagnostics go to stderr. Structured batch output never
prompts for source selection. JSON and JSONL reject `--color always`.
Redirected help is plain by default; `NO_COLOR=1` also disables terminal help colors.
Help styling is independent of the table-output `--color` option.

| Exit code | Meaning |
| --- | --- |
| 0 | Success, help, version, normal interactive quit, or a consumer closing its pipe. |
| 1 | Source, configuration, runtime, output, or incompatible runtime-option error. |
| 2 | Command-line syntax or value rejected by the argument parser. |

Preparation failures leave stdout empty. Cancelling an interactive operation
never exports an unfinished result. Broken pipes during writing or flushing count
as success. Other write failures return 1 and may leave partial output. Writes
are not atomic; termination during serialization can leave truncated JSON or
JSONL. OS signals retain OS-defined exit status.

## Writes and compatibility

Tview does not modify source data. Saving a view writes local configuration and
prompts before overwriting it. Shell redirection opens its destination before
Tview runs, so never redirect output to the input path.

During 0.x, incompatible CLI or configuration changes require a minor release
and migration notes. Compatible fixes use patch releases. Version 1.0.0 will
establish the supported public contract; later incompatible changes require a
major release. Existing defaults and output field meanings remain unchanged
unless a release documents an incompatible change. Native-type export would
require a separate format. See [migration](migration.md).
