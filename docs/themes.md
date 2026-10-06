---
type: Guide
title: Color themes
description: Theme files, palettes, and terminal color modes.
generated: { by: openai-codex/gpt-6.1-sol, at: 2026-10-05T04:25:43Z }
---

# Color themes

Tview loads theme settings from `$XDG_CONFIG_HOME/tview/config.yml`, or
`~/.config/tview/config.yml` when `XDG_CONFIG_HOME` is unset:

```yaml
theme: cmdzro
```

Theme files live in `tview/themes/*.yml` or `tview/themes/*.yaml` under the same
config directory. If both `name.yml` and `name.yaml` exist, `.yml` wins. The
built-in `cmdzro` theme uses gray text, blue UI backgrounds, yellow search
highlights, and red errors. Tview uses it when no theme is configured.

Theme colors accept 16-color names, 256-color palette values, and 32-bit hex:

```yaml
name: ops-dark
mode: auto # auto, ansi16, ansi256, hex32, or truecolor

palette:
  text: "#AFAFAFFF"
  gray: gray
  muted: palette(240)
  ui_blue: palette(19)
  blue: blue
  dark_blue: palette(19)
  cyan: cyan
  dark_cyan: dark-cyan
  green: dark-green
  magenta: magenta
  yellow: yellow
  error: dark-red
  teal: "#25A39AFF"

identifiers:
  colors: [bright-green, magenta, cyan, white]

styles:
  table:
    location:
      fg: gray
      bg: black
    current_cell:
      fg: cyan
      bg: dark_blue
    divider:
      fg: gray
    header:
      fg: dark_cyan
      modifiers: [bold]
    header_selected:
      fg: cyan
      modifiers: [bold]
    header_glyph:
      fg: muted
    cell:
      fg: text
    selected:
      fg: text
      bg: dark_blue
    hidden_marker:
      fg: muted
  popup:
    background:
      fg: text
      bg: dark_blue
    border:
      fg: cyan
      bg: dark_blue
    title:
      fg: gray
      bg: dark_blue
    body:
      fg: text
      bg: dark_blue
    disabled:
      fg: muted
      bg: dark_blue
    active:
      fg: gray
      bg: dark_blue
    action:
      fg: cyan
      bg: dark_blue
    option_selected:
      fg: cyan
      bg: dark_blue
  search:
    highlight:
      fg: yellow
      modifiers: [underline]
  message:
    footer:
      fg: yellow
      bg: ui_blue
```

Named 16-color values use tview's built-in cmdzro base palette; in truecolor
mode they resolve to those RGB values, while `mode: ansi16` emits ANSI colors
for the terminal palette.

Conditional colors in a saved view select a foreground using ordered match,
range, fixed-gradient, automatic-gradient, or identifier rules. The TUI and
colored table output resolve those rules against the loaded theme in the same
terminal color mode. A palette alias takes precedence over a literal color
name, even when both have the same spelling; configured alias names retain
punctuation and Unicode. ANSI-16, ANSI-256, and truecolor modes use their
supported deterministic fallbacks. Hex alpha does not blend with the terminal
background. Saved YAML retains its configured color strings.

Complete rendering profiles the applicable complete result, including rows
outside the current viewport; a table preview profiles emitted rows only.
Omitted and lookahead rows cannot change the preview's automatic gradient or
identifier foregrounds. Selecting a TUI cell keeps its background and
modifiers; only a matching search substring takes search foreground and
modifiers. Plain table, JSON, and JSONL output do not profile colors solely
for conditional rules.

See the [complete sample theme](../examples/data/config/themes/cmdzro.yml) and [theme
schema](../schemas/theme.schema.json). For per-column rules, see [conditional
colors](saved-views.md#conditional-colors).
