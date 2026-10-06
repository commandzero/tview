## Purpose

Define configurable color themes for Ratatui rendering, terminal color-mode fallback, and theme-driven identifier color families.

## Requirements

### Requirement: Theme discovery
The system SHALL discover color theme YAML files from `$XDG_CONFIG_HOME/tview/themes`, or `~/.config/tview/themes` when `XDG_CONFIG_HOME` is unset, and SHALL provide a built-in default theme when no user theme is selected.

#### Scenario: Built-in default theme
- **WHEN** a user opens an input without selecting a theme and no theme configuration is present
- **THEN** the system applies the built-in `cmdzro` theme

#### Scenario: Discover user themes
- **WHEN** `solarized-dark.yml` exists under `~/.config/tview/themes`
- **THEN** the system makes a theme named `solarized-dark` available for selection

#### Scenario: Missing theme directory
- **WHEN** the theme directory does not exist
- **THEN** the system opens the input using the built-in default theme without reporting an error

### Requirement: Theme selection
The system SHALL allow selecting a theme by name through configuration and SHALL fail clearly when a user explicitly selects a missing theme.

#### Scenario: Select configured theme
- **WHEN** tview configuration selects `theme: ops-dark` and `ops-dark.yml` exists in the theme directory
- **THEN** the system applies the `ops-dark` theme to the TUI session

#### Scenario: Missing selected theme
- **WHEN** tview configuration selects `theme: missing` and no discovered or built-in theme has that name
- **THEN** the system reports a clear configuration error and does not start the viewer

#### Scenario: Invalid unselected theme
- **WHEN** an unselected theme file is malformed
- **THEN** the system logs the failure, records a TUI warning, and continues opening the input with the selected or default theme

### Requirement: Theme YAML schema
The system SHALL ship and document a YAML theme schema covering theme metadata, color mode, palette aliases, identifier color families, and named UI style tokens.

#### Scenario: Valid theme file
- **WHEN** a theme YAML file defines `name`, `mode`, palette aliases, identifier color families, and required style tokens
- **THEN** the theme validates and can be applied

#### Scenario: Unknown style token
- **WHEN** a theme YAML file defines an unsupported style token
- **THEN** validation reports the unsupported token with the theme filename and token path

#### Scenario: Missing required token
- **WHEN** a selected theme omits a required style token
- **THEN** the system reports a clear configuration error and does not start the viewer

### Requirement: Theme identifier families
The system SHALL allow themes to define identifier color families used by saved-view `identifiers` conditional colors.

#### Scenario: Theme identifier colors
- **WHEN** a theme defines `identifiers.colors: ["bright-green", "magenta", "cyan", "white"]`
- **THEN** `identifiers` conditional colors using `colors: auto` are generated from those families

#### Scenario: Identifier family shades
- **WHEN** an identifier family color is configured
- **THEN** the system generates 16 dark-to-light shades for that family before repeating, with the darkest shade no darker than the ANSI dark/dim foreground equivalent

#### Scenario: Built-in identifier families
- **WHEN** no user theme is selected
- **THEN** the built-in `cmdzro` theme provides green, magenta, cyan, and white identifier families

### Requirement: Color value modes
The system SHALL support 16-color names, 256-color palette indexes, and 32-bit hex colors in theme files and conditional color rules.

#### Scenario: Sixteen-color name
- **WHEN** a theme color is `green`, `bright-white`, or another supported 16-color name
- **THEN** the system maps it to the corresponding built-in cmdzro base-16 color in truecolor/256-color modes, or emits the ANSI color in `ansi16` mode

#### Scenario: Two hundred fifty six color index
- **WHEN** a theme color is `palette(124)` or an equivalent 256-color integer notation
- **THEN** the system maps it to the corresponding 256-color terminal palette entry

#### Scenario: Thirty two bit hex color
- **WHEN** a theme color is `#25a39aFF`
- **THEN** the system parses the red, green, blue, and alpha channels and uses the RGB value for terminal rendering

#### Scenario: Lower color terminal fallback
- **WHEN** the active terminal cannot render the configured color mode
- **THEN** the system resolves each color to the nearest supported configured fallback without changing the loaded theme file

### Requirement: Cmdzro default theme
The built-in default theme SHALL use `~/.config/nvim/colors/cmdzro.vim` as its baseline while adapting the palette for table viewing constraints.

#### Scenario: Text color avoids blue
- **WHEN** default theme text, headers, ordinary cell values, and popups are rendered
- **THEN** the rendered foreground colors do not use blue-family text colors

#### Scenario: Yellow reserved for emphasis
- **WHEN** the default theme renders search highlights or emphasized UI state
- **THEN** yellow-family colors are permitted only for those search or emphasis tokens

#### Scenario: Red reserved for unhealthy state
- **WHEN** the default theme renders ordinary values, headers, or navigation UI
- **THEN** red-family colors are not used unless the token represents an error, failed validation, unhealthy status, or user-defined conditional rule

#### Scenario: Blue reserved for UI elements
- **WHEN** the default theme uses blue-family colors
- **THEN** they appear only in UI backgrounds, borders, status areas, selections, or other non-text emphasis surfaces

### Requirement: Themed Ratatui rendering
The system SHALL render Ratatui table and popup styles from the active theme tokens rather than hard-coded colors.

#### Scenario: Table chrome uses theme
- **WHEN** the table renders location, divider, header, selected cell, hidden-column marker, and footer message line
- **THEN** each element uses the corresponding active theme style token

#### Scenario: Popups use theme
- **WHEN** cell, info, help, search, filter, column info, or saved view popups are rendered
- **THEN** popup background, border, title, disabled text, active item, and action labels use the active theme tokens

#### Scenario: Search highlight uses theme
- **WHEN** search highlights are visible in the table
- **THEN** only the matching substring uses the active theme search foreground and modifiers
- **AND** the cell background is not changed by search highlighting

### Requirement: Theme style modifiers
The system SHALL support style modifiers for theme tokens, including bold, italic, underline, reversed, and dim where supported by Ratatui and the active terminal backend.

#### Scenario: Bold header token
- **WHEN** a theme token sets `modifiers = ["bold"]` for headers
- **THEN** table headers render with Ratatui bold styling

#### Scenario: Unsupported terminal modifier
- **WHEN** the terminal backend cannot visibly render a configured modifier
- **THEN** tview continues rendering with the configured colors and does not fail the session

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
