## Why

Tview's table output prepares every row and profiles the full result before writing. Large files therefore make Yazi previews slow even when only a few rows fit on screen.

## What changes

- Add `--sorted true|false`, defaulting to `true`. For direct table output, `false` skips saved `view.sort` while retaining other view settings and source-query ordering.
- Add `-n <count>` and `--top-lines <count>` to limit direct table output to that many data rows after filtering and the selected sorting policy. The header and summary are extra lines. Omitting the option preserves complete output.
- Prepare preview rows before rendering, without completing the file just for widths, schema discovery, conditional colors, or row counting. Unsorted previews stop after the requested matching rows and bounded lookahead.
- Append `x more rows...` when the exact number of omitted result rows is known. Append `more rows...` when additional rows are confirmed but their count is unknown. Do not scan the rest of a file just to count it.
- Keep these options scoped to direct table output, including automatically selected table output. Reject explicit use with the TUI, interactive export, JSON, or JSONL.

Example for a file-manager preview:

```sh
tview --output table --sorted false --top-lines 30 data.csv
```

## Capabilities

### New capabilities

None.

### Modified capabilities

- `cli-compatibility`: Add sorting and preview-limit arguments, defaults, and validation.
- `non-interactive-output`: Add prefix preparation, preview widths and schema, and a remaining-row summary while retaining complete-output defaults.
- `saved-views`: Allow an invocation to skip saved view sorting without disabling the rest of the saved view.

## Impact

Changes will affect CLI parsing, saved-view application, source preparation, the shared output driver, and the fixed-width table adapter. File adapters must support early stopping even below the existing lazy-file threshold. SQLite and Elasticsearch keep their source-query limits and ordering. Tests must measure rows read, not just lines written. No new dependency is planned.

## Non-goals

Changing default output limits, truncating JSON exports, changing TUI navigation, editing saved-view files, removing native SQL or ES|QL sorting, or adding Yazi plugin code.
