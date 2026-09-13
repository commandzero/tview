---
okf_version: "0.2"
---

# Tview documentation

Start with the [README](../README.md) to install Tview and open your first
table.

## Using Tview

- [Installation](installation.md) - Cargo installation, optional features, and local builds.
- [File input](file-input.md) - Delimited text, JSON, NDJSON, format detection, and nested data.
- [SQLite sources](sqlite.md) - Read-only database browsing, table selection, and source queries.
- [Elasticsearch sources](elasticsearch.md) - ES|QL queries, index selection, authentication, and source limits.
- [Keybindings](keybindings.md) - Navigation, search, sorting, filters, and column controls.
- [Saved views](saved-views.md) - Save source options, column formatting, filters, sorting, and colors.
- [Color themes](themes.md) - Theme files, palettes, and terminal color modes.
- [Large files and schema discovery](large-files.md) - Indexing, schema scan limits, and operations that read the full input.
- [CLI output and compatibility](cli-contract.md) - Output schemas, exit codes, stream behavior, and compatibility rules.
- [Migration from Tabview](migration.md) - Upgrade steps for the Rust binary and renamed configuration.

## Project and development

- [Contributor guide](contributing.md) - Ownership, compiler support, checks, and documentation boundaries.
- [Release process](releases.md) - Reviewed tags, native packaging, checksums, and publication recovery.
- [Modal style](modal-style.md) - Layout and keyboard interaction rules for terminal modal dialogs.
- [Turso build and runtime impact](turso-build-impact.md) - Recorded SQLite dependency, build, and runtime tradeoffs.
- [Credits](credits.md) - Tabview origins, contributors, and license.
