---
type: Guide
title: Modal style
description: Layout and keyboard interaction rules for terminal modal dialogs.
generated: { by: codex/gpt-6, at: 2026-09-12T17:14:16Z }
---

# Modal style

Use these rules for terminal modal dialogs.

## Layout

- Keep one blank character between the border and content.
- Left-align the title in title case.
- Put one space between the left border and the title.
- Use title case for section headers.
- Use two-column section layouts when the modal has several short sections.
- Skip section headers in keyboard navigation.

## Actions

- Put modal actions in the lower-right border as `[ Name ]` buttons.
- Use `Tab` and `Shift+Tab` to move forward and backward between groups.
- Use arrow keys to navigate all options in the active group.
- Use `Space` to activate or deactivate the selected item.
- Use dim terminal text, dark white or bright black, for disabled options.

## Example

```text
 ┌─ Column Info ─────────────────────────────────────────────────────────────────────────────────────────┐
 │                                                                                                       │
 │ > Visible                                             Align                                           │
 │   (*)  visible                                        (*)  auto                                       │
 │   ( )  hidden                                         ( )  left                                       │
 │                                                       ( )  right                                      │
 │   Type                                                                                                │
 │   ( )  text                                           Format                                          │
 │   ( )  date                                           (*)  plain                                      │
 │   ( )  ip                                             ( )  locale                                     │
 │   (*)  float                                          ( )  uppercase                                  │
 │   ( )  integer                                        ( )  lowercase                                  │
 │   ( )  semver                                         ( )  char                                       │
 │   ( )  boolean                                        ( )  bit                                        │
 │                                                       ( )  word                                       │
 │   Sort                                                                                                │
 │   (*)  none                                           Filters                                         │
 │   ( )  ▲ ascending                                    None                                            │
 │   ( )  ▼ descending                                                                                   │
 │                                                                                                       │
 │                                                                                                       │
 └───────────────────────────────────────────────────────────────────────────────────[ Save ] [ Cancel ]─┘
```
