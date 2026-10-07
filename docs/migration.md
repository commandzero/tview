---
type: Guide
title: Migration from Tabview
description: Upgrade steps for the Rust binary and renamed configuration.
generated: { by: openai-codex/gpt-6.1-sol, at: 2026-10-07T03:35:24Z }
---

# Migration from Tabview

Tview is an independent Rust rewrite of Tabview. Its first planned release is
`0.1.0`, with a separate version sequence. Historical Tabview tags and MIT
attribution remain intact. Crates.io is the primary distribution channel;
GitHub archives also provide native binaries.

1. Install the published crate using `cargo install tview`.
2. Replace calls to the Python `tabview` command with `tview`.
3. Keep Python integrations on upstream Tabview or rewrite them around the CLI.
   Tview does not provide the upstream Python import API.
4. Copy saved configuration from `$XDG_CONFIG_HOME/tabview` to
   `$XDG_CONFIG_HOME/tview`, or from `~/.config/tabview` to `~/.config/tview`.
   Keep the old directory until the new command works with your saved views.
5. Replace `TABVIEW_` environment-variable prefixes with `TVIEW_` in scripts
   that used the earlier Rust rewrite. Elasticsearch credentials keep their
   `ELASTIC_` names. Tview does not fall back to the old config directory.
6. Review pipelines. Redirected stdout defaults to table text. Use explicit
   JSON or JSONL for a documented machine format. Never redirect over the source
   file.

See the [release process](releases.md) for archive names, supported platforms,
and publication checks. Historical Tabview tags and published files stay
unchanged.
