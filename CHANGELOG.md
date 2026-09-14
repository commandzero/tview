# Changelog

## [Unreleased]

### Removed

- Breaking: removed support for the upstream Python import API. Keep Python
  integrations on upstream Tabview or follow the [migration steps](docs/migration.md).
- Removed Python packaging and runtime support from the Rust rewrite.
- Removed the legacy Travis CI configuration.

### Changed

- Start Tview at `0.1.0` with an independent release sequence; retain upstream
  Tabview history and attribution. During 0.x, incompatible changes use minor releases.
- Shortened the README and moved detailed usage into focused user guides.
- Adopted shared repository checks, documentation validation, compiler pins, and
  native release packaging. Minimum Rust is 1.90.0.
- Breaking: renamed the Rust rewrite to `tview`, including its crate, executable,
  configuration directory, and environment variables. Follow the
  [migration steps](docs/migration.md) for existing configuration and scripts.
- Rewrote the upstream Python viewer as a Rust CLI distributed as a single
  `tview` binary.
- Preserved the existing command-line interface, including stdin mode, explicit
  encodings, delimiters, quoting options, and `+y:x` start-position syntax.
- Rebuilt the spreadsheet-like terminal interface with Ratatui and crossterm
  while preserving the existing layout, navigation, search, sort, reload,
  column sizing, header, popup, and skip-to-change workflows.
- Switched installation to `cargo install tview` from crates.io.
- Made clipboard support an optional Cargo feature backed by Rust clipboard
  integration.
- Made large seekable inputs open through incremental stores with partial row
  counts, bounded initial rendering, and controlled full-table operations.
- Unified sorting and filtering across sources. Preserved typed values, stable
  row identity, null placement, and the previous result when a query fails.

### Added

- Added `--sorted true|false` to control saved-view sorting in direct table output.
- Added `-n` and `--top-lines` for fast table previews with a remaining-row summary.
- Added explicit JSON and JSONL export of displayed cells and `--version`.
- Added Rust test coverage for CLI compatibility, data ingestion, table
  operations, rendering snapshots, and accepted behavior changes.
- Added JSON and NDJSON table inputs with automatic or explicit format
  selection, RFC 6901 starting paths, typed cells, and streaming schema
  discovery.
- Added saved-view source options, canonical JSON column matching, display-label
  overrides, and view/per-column null-placement policy.

## [1.4.4] - 2020-01-09

### Added

- Added a note about Visidata and minimal maintenance.
- Added file URI scheme support.
- Added sample text with long and wide characters.

### Changed

- Removed Python 2.x support.
- Removed Python 3.3 support.
- Updated Travis CI for newer Python versions and fixed the flake8 command.

### Fixed

- Fixed flake8 errors.

## [1.4.3] - 2017-11-13

### Added

- Added an additional parse step for space-delimited files:
  1. Replace multiple spaces, such as those used to align columns, with a single
     space.
  2. If and only if the top line begins with a standard comment character (`#`
     or `%`), remove it.
- Added numeric sort.
- Added the ability to specify `quotechar`.

### Changed

- Removed `0` for beginning-of-line and changed numeric sort to `#` and `@`.

## [1.4.2] - 2016-01-17

### Added

- Added support for running unit tests with `python setup.py test`.

### Fixed

- Fixed packaging issues.

## [1.4.1] - 2015-04-04

### Added

- Added a file and data information popup.
- Added support for different quoting schemes.

## [1.4.0] - 2015-02-21

### Added

- Added incremental find-as-you-type search.
- Added support for reloading changed files while preserving display
  parameters.
- Added variable width columns with `mode`, `max`, and fixed-width settings.
- Added support for reading from stdin.
- Added commands to resize columns individually or as a whole.
- Added commands to skip to the next changed value by row or column.
- Added support for passing a `y,x` start position on the command line or to
  `view()`.

## [1.3.0] - 2015-01-17

### Added

- Added basic unit and integration tests.
- Added Travis CI integration.

### Fixed

- Fixed bugs and improved speed.

## [1.2.0] - 2015-01-08

### Added

- Added dual Python 2.7+ and Python 3+ support with improved Unicode handling.
- Added natural sort capability for better numeric sorting.
- Added dynamic column width and gap adjustment.
- Added a jump-to-column command.
- Added terminal resizing support.

### Fixed

- Fixed multiple crashes.

## [1.1.0] - 2014-10-29

### Added

- Added in-place file reload support. Fixes #2.
- Added yank-to-clipboard support. Fixes #13.
- Added additional encoding types to try before failing.

### Changed

- Read the entire file before deciding the encoding.

### Fixed

- Fixed extra highlighting when at the bottom-right cell. Fixes #7.
- Fixed header row toggling cleanup. Fixes #18.
- Fixed a crash and display of cells with newlines. Fixes #16.

## [1.0.1] - 2014-08-16

### Added

- Added the `0` key for beginning-of-line navigation.

### Changed

- Updated modifier key handling.

[Unreleased]: https://github.com/commandzero/tview/compare/aad067df576e13a16a0b74559ecb59b6b4d1ec4a...main
[1.4.4]: https://github.com/Tabviewer/tabview/compare/1.4.3...1.4.4
[1.4.3]: https://github.com/Tabviewer/tabview/compare/1.4.2...1.4.3
[1.4.2]: https://github.com/Tabviewer/tabview/compare/1.4.1...1.4.2
[1.4.1]: https://github.com/Tabviewer/tabview/compare/1.4.0...1.4.1
[1.4.0]: https://github.com/Tabviewer/tabview/compare/1.3.0...1.4.0
[1.3.0]: https://github.com/Tabviewer/tabview/compare/1.2.0...1.3.0
[1.2.0]: https://github.com/Tabviewer/tabview/compare/1.1.0...1.2.0
[1.1.0]: https://github.com/Tabviewer/tabview/compare/1.0.1...1.1.0
[1.0.1]: https://github.com/Tabviewer/tabview/compare/1.0...1.0.1
