# Spec Delta

## ADDED Requirements

### Requirement: Cohesive conditional-color evaluation
The system SHALL evaluate configured conditional colors consistently for the TUI and colored table output using the same ordered rule semantics and terminal fallback. Equivalent cells, presentation, theme, profile scope, and resolved terminal mode SHALL produce the same conditional foreground without changing raw or rendered values.

#### Scenario: First match across both consumers
- **WHEN** a cell satisfies both an earlier range rule and a later automatic gradient rule
- **THEN** the TUI and colored table output use the earlier rule's foreground
- **AND** reordering those rules changes precedence consistently in both consumers

#### Scenario: No matching conditional color
- **WHEN** no configured rule matches a cell
- **THEN** both consumers retain the ordinary cell foreground from the active theme

#### Scenario: Selection and search remain separate
- **WHEN** a conditionally colored TUI cell is selected and contains a search match
- **THEN** selection preserves its configured background and modifiers while preserving the conditional foreground outside the match
- **AND** only the matching substring receives the search foreground and modifiers without changing the cell background
- **AND** colored table output contains the conditional foreground without transient selection or search styling

### Requirement: Configured color reference fidelity
The system SHALL preserve supported YAML color strings and theme aliases at the configuration and persistence edge while resolving conditional foregrounds consistently for ANSI-16, ANSI-256, and truecolor terminals. Palette alias lookup SHALL preserve the exact configured alias name and take precedence over interpreting that name as a literal color.

#### Scenario: Punctuation and Unicode aliases
- **WHEN** a theme defines aliases containing commas, semicolons, colons, parentheses, or Unicode and a match, range, fixed gradient, automatic gradient, or identifier rule refers to them
- **THEN** both consumers resolve each complete alias name without splitting, truncating, or normalizing it

#### Scenario: Alias shadows literal color spelling
- **WHEN** the active theme defines a palette alias named `green` with a different target color and a conditional rule selects `green`
- **THEN** both consumers use that alias target rather than the built-in literal green

#### Scenario: Terminal capability parity
- **WHEN** the same configured rules use a named 16-color value, `palette(124)`, and `#25A39AFF` under ANSI-16, ANSI-256, and truecolor resolved modes
- **THEN** both consumers produce the same supported foreground for each mode using the existing deterministic fallback rules
- **AND** alpha does not cause terminal blending or alter the persisted color string

#### Scenario: Saving retains configured colors
- **WHEN** a saved view with explicit identifier families, aliases, or gradient colors is displayed or saved after rendering
- **THEN** its YAML contains those configured color strings rather than runtime identifier indexes, bucket results, or resolved terminal foregrounds

### Requirement: Scoped conditional-color profiles
The system SHALL derive profile-dependent conditional colors from the applicable presentation scope: only emitted rows for a frozen preview projection, and the complete configured active result for complete-result rendering. Omitted, rejected, and lookahead rows SHALL NOT affect preview colors. Identifier indexing SHALL use deterministic ordering of unique nonempty rendered values within that scope.

#### Scenario: Preview gradient ignores omitted extreme
- **WHEN** a two-row preview emits numeric values `1` and `2` and an omitted or lookahead row changes from `3` to `99999999`
- **THEN** the two emitted automatic-gradient foregrounds remain unchanged
- **AND** neither gradient profiling nor style evaluation pulls additional source rows beyond preview preparation's selected rows and existing lookahead

#### Scenario: Complete result includes offscreen values
- **WHEN** the complete configured active result contains `1`, `2`, and `99999999` but the viewport shows only the first two rows
- **THEN** its automatic gradient uses the complete result's extrema rather than viewport-only extrema
- **AND** complete colored table output uses the same foregrounds for those cells under that same scope and resolved terminal mode

#### Scenario: Stable identifiers within a scope
- **WHEN** a scope contains rendered identifiers `beta`, `alpha`, and `beta` and its row order changes without changing its values
- **THEN** both occurrences of `beta` keep the same foreground and deterministic index assignment is independent of row order
- **AND** empty rendered identifiers receive no identifier color

#### Scenario: Preview identifier ignores lookahead identifier
- **WHEN** the preview emits `beta` and `gamma` and an omitted or lookahead value changes to the lexicographically earlier `alpha`
- **THEN** the emitted identifier foregrounds remain unchanged

### Requirement: Conditional numeric and family semantics
The system SHALL preserve existing numeric scalar parsing and raw-value losslessness, fixed-gradient interval semantics, automatic-gradient bucket semantics, and identifier family generation when evaluating conditional colors. Color evaluation SHALL NOT change source types, raw values, display formatting, or saved numeric/string match configuration.

#### Scenario: Fixed stop intervals are not interpolation
- **WHEN** a fixed gradient defines stops `0: green`, `50: yellow`, and `100: red`
- **THEN** values below `0` are unmatched, values in `[0, 50)` use green, values in `[50, 100)` use yellow, and values at or above `100` use red
- **AND** both consumers preserve those half-open intervals rather than interpolating between stops

#### Scenario: Automatic bucket boundaries
- **WHEN** an automatic gradient with eight steps profiles numeric values from `0` to `80`
- **THEN** values below `10` occupy the first bucket, `10` begins the second bucket, and `80` occupies the final bucket
- **AND** the first and final buckets resolve to the configured endpoint colors with the existing RGB interpolation and terminal fallback

#### Scenario: Constant and nonnumeric profile
- **WHEN** all parseable values in an automatic-gradient scope are equal
- **THEN** those values use the first configured color
- **AND** nonnumeric values do not affect extrema and remain unmatched by that gradient so later matching rules can apply

#### Scenario: Numeric parsing preserves cell representation
- **WHEN** numeric conditional rules evaluate percentages, supported unit suffixes, or values interpreted using a column's time/scientific numeric profile
- **THEN** they use the existing scalar parser and column profile rather than parsing a rounded formatted display string
- **AND** original integer, decimal, and string cell values remain lossless for output, copy, and configuration persistence

#### Scenario: Theme and saved-view identifier families
- **WHEN** identifier rules use `colors: auto` or an explicit saved-view family list
- **THEN** automatic families come from the active theme and explicit families override them
- **AND** indexes cycle through families before advancing through their 16 dark-to-light shades, retain the existing dim-foreground contrast floor, and repeat only after all family/shade combinations

### Requirement: Conditional-color state freshness
The system SHALL discard or refresh result-dependent conditional-color facts when the active result, schema, rendered presentation, rule configuration, numeric profile, or profile scope changes. Scrolling or repainting an unchanged result SHALL NOT change its conditional foregrounds.

#### Scenario: Formatting changes identifier grouping
- **WHEN** presentation changes values `alpha` and `ALPHA` to the same uppercase rendered identifier
- **THEN** subsequent TUI and colored output cells share that identifier's foreground rather than using stale pre-formatting indexes

#### Scenario: Active result or schema replacement
- **WHEN** a successful replacement changes column identity, available values, numeric interpretation, or configured rules
- **THEN** subsequent rendering uses color facts from the newly active result and presentation rather than the previous result or an unrelated column index
- **AND** a failed or superseded replacement that leaves the previous result active preserves its correct colors

#### Scenario: Preview followed by complete output
- **WHEN** the same configuration is used first with an emitted-prefix scope and later with a complete-result scope containing additional extrema or identifiers
- **THEN** complete output recomputes the applicable profile facts rather than reusing prefix-dependent indexes or extrema

### Requirement: Demand-scoped conditional-color work
The system SHALL avoid color-specific profiling for plain table, JSON, and JSONL output, and SHALL NOT eagerly compute conditional styles for every cell of a complete dataset. Repeated colored rendering with unchanged configuration and profiles SHALL reuse resolvable rule colors, family shades, and gradient buckets instead of repeating color-list decoding or alias traversal per cell.

#### Scenario: Plain output does not request color profiles
- **WHEN** an invocation applies saved conditional rules but emits plain table output with `--color never`, JSON, or JSONL
- **THEN** no source traversal or profile reduction is requested solely to prepare conditional colors
- **AND** required source/view operations and value formatting still execute normally

#### Scenario: Repaint reuses resolved colors
- **WHEN** the same profiled viewport or colored output rows are rendered repeatedly without a relevant state change
- **THEN** configured color lists and alias chains are not decoded or traversed again for each cell
- **AND** reusable color state depends on configured rules, gradient steps, family/shade combinations, and unique scoped identifiers rather than a row-by-column matrix of computed styles
