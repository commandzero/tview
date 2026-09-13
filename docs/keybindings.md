---
type: Guide
title: Keybindings
description: Navigation, search, sorting, filters, and column controls.
generated: { by: codex/gpt-6, at: 2026-09-12T17:14:16Z }
---

# Keybindings

The top-left corner shows the selected cell position and contents. Press `?` or
`F1` for help inside Tview. Numeric prefixes repeat a movement or select a
position.

| Key | Action |
| --- | --- |
| `F1`, `?` | Show keybindings. |
| Cursor keys, `h`, `j`, `k`, `l` | Move the highlighted cell, scrolling if required. |
| `q`, `Q` | Quit. |
| `Home`, `^`, `Ctrl-a` | Move to the start of this row. |
| `End`, `$`, `Ctrl-e` | Move to the end of this row. |
| <code>[num]&#124;</code> | Go to column `num`, or the first column when `num` is omitted. |
| `PgUp`, `PgDn`, `J`, `K` | Move a page up or down. |
| `H`, `L` | Move a page left or right. |
| `g` | Go to the top of the current column. |
| `[num]G` | Go to row `num`, or the bottom of the current column when `num` is omitted. |
| `Ctrl-g` | Show file and data information. |
| `Insert`, `m` | Mark the current cell. |
| `Delete`, `'` | Return to the marked cell, if any. |
| `Enter` | View full cell contents in a popup. |
| `/` | Search. |
| `i` | Edit the current column view configuration, sort state, and filter action. |
| `u` | Edit staged source filters, native sort, and source limit. |
| `V` | Show source-independent view configuration. |
| `p` | Show and copy the active source query when available. |
| `f`, `F` | Filter in or filter out rows by the current column. `Tab` cycles text, regex, and numeric modes; submitting an empty condition clears filters for the current column. |
| `n` | Go to the next search result. |
| `N` | Go to the previous search result. |
| `t` | Toggle fixed header row. |
| `<`, `>` | Decrease or increase all column widths. |
| `,`, `.` | Decrease or increase the current column width. |
| `-`, `+` | Decrease or increase the column gap. |
| `s`, `S` | Sort the current column lexically, ascending or descending. |
| `a`, `A` | Sort the current column naturally, ascending or descending. |
| `#`, `@` | Sort the current column numerically, ascending or descending. |
| `r` | Reload file or input data and reset sort order. |
| `y` | Yank the rendered current cell to the clipboard when clipboard support is enabled. |
| `Y` | Yank the raw current cell to the clipboard when clipboard support is enabled. |
| `v` | Show the saved view modal when saved views are enabled. |
| `[num]z` | Toggle variable column width mode between `mode` and `max`, or set all columns to width `num`. |
| `[num]Z` | Maximize the current column, or set the current column to width `num`. |
| `[num]chh`, `[num]chl` | Hide visible columns to the left or right of the current column. |
| `chj`, `chk` | Hide the current column. |
| `[num]cHh`, `[num]cHl` | Show adjacent hidden columns to the left or right. |
| `csk`, `csj`, `csx` | Sort the current column ascending, sort descending, or clear its sort key. |
| `[num][` | Skip to the previous row value change. |
| `[num]]` | Skip to the next row value change. |
| `[num]{` | Skip to the previous column value change. |
| `[num]}` | Skip to the next column value change. |
