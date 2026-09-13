## MODIFIED Requirements

### Requirement: Saved views in non-interactive output
When compiled with saved-view support, batch output SHALL perform the same saved-view selection and apply nested `source` configuration before opening and nested `view` configuration before emitting stdout. For direct table output, `--sorted false` SHALL suppress all saved `view.sort` application, including pending sort keys, while preserving filters, columns, formatting, and nested source configuration. Preview limits SHALL apply after the effective view operations. Neither option SHALL modify the saved file.

#### Scenario: Automatically selected view
- **WHEN** redirected output opens a filename matching a saved view
- **THEN** its source query and view transform control the bounded output

#### Scenario: Forced named view
- **WHEN** batch output uses `--view <name>`
- **THEN** that named view controls source and view configuration even when its filename patterns do not match

#### Scenario: Saved views disabled
- **WHEN** batch output uses `--no-view`
- **THEN** no saved source table or other saved configuration is applied

#### Scenario: Saved SQLite table selection
- **WHEN** a database has multiple selectable candidates and a matching saved view sets `source.table`
- **THEN** batch output opens that table without interactive selection

#### Scenario: Pending column configuration
- **WHEN** bounded result traversal discovers a column whose `view.columns` configuration was pending
- **THEN** the configuration is applied before final widths and rows are rendered

#### Scenario: View filter produces no rows
- **WHEN** `view.filters` excludes every row from the bounded source result
- **THEN** batch output follows configured header visibility and empty-result rules without refilling

#### Scenario: Interactive transformation starts from saved view
- **WHEN** `--interactive` and `--output <format>` are combined
- **THEN** the TUI starts from nested saved configuration and final output uses subsequent live changes

#### Scenario: Preview disables saved sort only
- **WHEN** direct table output uses `--sorted false -n 30` with a saved view that defines source ordering, view sorting, filters, and formatting
- **THEN** source ordering, filters, and formatting remain active, view sorting is skipped, and at most 30 matching rows are emitted

#### Scenario: Late sort key stays disabled
- **WHEN** `--sorted false` is active and schema discovery resolves a pending saved view sort column
- **THEN** Tview does not apply that sort or trigger a full scan for it
