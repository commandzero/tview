---
type: Guide
title: Color themes
description: Theme files, palettes, and terminal color modes.
generated: { by: openai-codex/gpt-6.1-sol, at: 2026-10-07T03:48:23Z }
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

Start from the [complete sample theme](../examples/data/config/themes/cmdzro.yml).
Copy it into your themes directory, rename it, and set `theme` to that filename's
stem. Theme files must include the required style tokens; partial themes fail
to load.

For example, edit the palette and identifier colors in the copied file:

```yaml
palette:
  text: "#AFAFAFFF"
  accent: palette(19)
identifiers:
  colors: [bright-green, magenta, cyan]
```

This is an excerpt, not a complete theme. Use palette names in the file's
style definitions to apply them.

`mode` accepts `auto`, `ansi16`, `ansi256`, `hex32`, or `truecolor`.
Colors accept 16-color names, 256-color palette values, and hex values.
Named 16-color values use the built-in cmdzro palette in truecolor mode;
`ansi16` uses the terminal palette. A palette alias overrides a literal color
name with the same spelling. Hex alpha does not blend with the background.
The theme's terminal color mode also applies to colored table output.

Saved-view color rules choose cell foregrounds; see [conditional
colors](saved-views.md#conditional-colors). The [complete sample
theme](../examples/data/config/themes/cmdzro.yml) shows more styles, and
the [theme schema](../schemas/theme.schema.json) describes accepted values.

