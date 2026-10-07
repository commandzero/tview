---
type: Guide
title: Modal style
description: Layout and keyboard interaction rules for terminal modal dialogs.
generated: { by: openai-codex/gpt-6.1-sol, at: 2026-10-07T03:44:02Z }
---

# Modal style

Use these rules for terminal modal dialogs.

## Layout

- Leave one blank character between the border and content.
- Left-align titles with one space after the border. Use title case for titles
  and section headers.
- Use two columns for several short sections.
- Skip section headers during keyboard navigation.

## Actions

- Put modal actions in the lower-right border as `[ Name ]` buttons.
- Use `Tab` and `Shift+Tab` to move forward and backward between groups.
- Use arrow keys to move between all options in the active group.
- Use `Space` to activate or deactivate the selected item.
- Display disabled options in dim terminal text, using dark white or bright black.

## Example

```text
 ┌─ Column Info ───────────────────────────┐
 │                                        │
 │ > Visible          Align               │
 │   (*) visible      (*) auto            │
 │   ( ) hidden       ( ) left            │
 │                    ( ) right           │
 │                                        │
 └───────────────────[ Save ] [ Cancel ]───┘
```
