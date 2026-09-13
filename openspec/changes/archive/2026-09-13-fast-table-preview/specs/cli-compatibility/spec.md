## ADDED Requirements

### Requirement: Table output sorting option
The CLI SHALL accept `--sorted true|false` with an explicit value and SHALL default to `true`. Explicit use SHALL be valid only for resolved direct table output. The implicit default SHALL NOT change or invalidate other modes. `false` SHALL suppress saved view sorting for the invocation without altering source ordering or other saved configuration.

#### Scenario: Default retains saved sorting
- **WHEN** direct table output omits `--sorted` or supplies `--sorted true`
- **THEN** configured saved view sorting is applied

#### Scenario: Disable saved sorting
- **WHEN** direct table output supplies `--sorted false`
- **THEN** saved view sort keys are not applied and rows retain the source result order after filtering

#### Scenario: Invalid sorting value
- **WHEN** `--sorted` lacks a value or its value is not `true` or `false`
- **THEN** argument parsing fails with exit code 2 before reading input or writing stdout

### Requirement: Table preview row limit option
The CLI SHALL accept equivalent `-n <count>` and `--top-lines <count>` options with a positive integer representable by the implementation's row-count type. Omitting the option SHALL impose no additional output limit. The count SHALL limit logical data rows, excluding the header and remaining-row summary. Explicit use of either preview option SHALL require direct table output after normal mode resolution. Incompatible modes SHALL fail with exit code 1 before source consumption or stdout output; invalid argument values SHALL fail with exit code 2.

#### Scenario: Short and long forms agree
- **WHEN** otherwise identical direct table invocations use `-n 30` and `--top-lines 30`
- **THEN** both emit the same first 30 result rows or all result rows if fewer exist

#### Scenario: Automatic table mode accepts preview options
- **WHEN** stdout is redirected, no output option is supplied, and `--sorted false -n 30` is supplied
- **THEN** normal mode resolution selects table output and applies the preview options

#### Scenario: Invalid row limit
- **WHEN** the count is missing, zero, negative, fractional, nonnumeric, or too large for the row-count type
- **THEN** argument parsing fails with exit code 2 and stdout remains empty

#### Scenario: Incompatible mode
- **WHEN** either option is explicitly supplied with automatic or explicit TUI mode, interactive export, JSON output, or JSONL output
- **THEN** Tview rejects the combination before consuming input and explains that the option requires direct table output
