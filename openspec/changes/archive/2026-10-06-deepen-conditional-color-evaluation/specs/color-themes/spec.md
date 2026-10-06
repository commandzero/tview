# Spec Delta

## ADDED Requirements

### Requirement: Cohesive conditional-color evaluation
The system SHALL evaluate configured conditional colors consistently for the TUI and colored table output using the existing ordered-rule, numeric, gradient, family, and raw-value preservation requirements in the saved-views capability and the existing terminal fallback rules. Equivalent cells, presentation, theme, profile scope and interpretation, and resolved terminal mode SHALL produce the same conditional foreground without changing raw or rendered values.

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
The system SHALL derive profile-dependent conditional colors from their supplied presentation scope: emitted rows for previews, and the applicable complete-result profiling domain for complete rendering. Omitted and lookahead rows SHALL NOT affect emitted-preview colors. Rejected-row exclusion SHALL follow the preparation owner's supplied emitted-row domain rather than color evaluation performing additional selection. Identifier indexing SHALL preserve deterministic ordering of unique nonempty profile keys and the provider's declared interpretation: rendered keys for resident/emitted profiling, raw display keys without rendered-key fallback for exact store reductions, and typed store numeric extrema with resident scalar-parsed fallback when exact numeric extrema are absent.

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

### Requirement: Conditional-color state freshness
The system SHALL discard or refresh result-dependent conditional-color facts when the active result, schema, rendered presentation, rule configuration, numeric profile, or profile scope changes. Scrolling or repainting an unchanged result SHALL NOT change its conditional foregrounds.

#### Scenario: Formatting preserves each existing profile interpretation
- **WHEN** presentation changes `alpha` and `ALPHA` to the same uppercase rendered identifier
- **THEN** resident/emitted profiling groups those rendered identifiers together, while exact store profiling retains its existing raw keys and a rendered lookup absent from those keys remains uncolored

#### Scenario: Active result or schema replacement
- **WHEN** a successful replacement changes column identity, available values, numeric interpretation, or configured rules
- **THEN** subsequent rendering uses color facts from the newly active result and presentation rather than the previous result or an unrelated column index
- **AND** a failed or superseded replacement that leaves the previous result active preserves its correct colors

#### Scenario: Preview followed by complete output
- **WHEN** the same configuration is used first with an emitted-prefix scope and later with a complete-result scope containing additional extrema or identifiers
- **THEN** complete output recomputes the applicable profile facts rather than reusing prefix-dependent indexes or extrema

### Requirement: Demand-scoped conditional-color work
The system SHALL avoid color-specific profiling for plain table, JSON, and JSONL output. Repeated colored rendering with unchanged configuration and applicable profiles SHALL preserve foregrounds without requesting additional source traversal or profile reductions solely for conditional colors.

#### Scenario: Plain output does not request color profiles
- **WHEN** an invocation applies saved conditional rules but emits plain table output with `--color never`, JSON, or JSONL
- **THEN** no source traversal or profile reduction is requested solely to prepare conditional colors
- **AND** required source/view operations and value formatting still execute normally

#### Scenario: Repaint preserves colors without re-profiling
- **WHEN** the same profiled viewport or colored output rows are rendered repeatedly without a relevant state change
- **THEN** conditional foregrounds stay unchanged and no additional source traversal or profile reduction is requested solely to recompute conditional colors
