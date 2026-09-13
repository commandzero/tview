---
type: Guide
title: Migration from Tabview
description: Upgrade steps for the Rust binary and renamed configuration.
generated: { by: codex/gpt-6, at: 2026-09-12T17:14:16Z }
---

# Migration from Tabview

Tview is an independent Rust rewrite of Tabview. The first planned Tview release
is `0.1.0`, with its own version sequence. Historical Tabview tags and MIT
attribution remain intact. Crates.io is the primary distribution channel. GitHub
archives provide native binaries alongside the crate.

1. Install the published crate using `cargo install tview`.
2. Replace calls to the Python `tabview` command with `tview`.
3. Keep Python integrations on upstream Tabview or rewrite them around the CLI.
   Tview does not provide the upstream Python import API.
4. Copy saved configuration from `$XDG_CONFIG_HOME/tabview` to
   `$XDG_CONFIG_HOME/tview`, or from `~/.config/tabview` to `~/.config/tview`.
   Keep the old directory until the new command works with your saved views.
5. Replace `TABVIEW_` environment-variable prefixes with `TVIEW_` in scripts that
   used the earlier Rust rewrite. Elasticsearch credentials retain their existing
   `ELASTIC_` names. No automatic fallback to the old config directory occurs.
6. Review pipelines. Redirected stdout defaults to table text; use explicit JSON
   or JSONL for a documented machine format. Never redirect over the source file.

See the [release process](releases.md) for archive names, supported platforms,
and publication checks. Historical Tabview tags and published files stay
unchanged.
