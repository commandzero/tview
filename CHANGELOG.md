# Changelog

## [Unreleased]

## [0.1.0] - 2026-09-19

### Removed

- Breaking: removed the Python API. Python integrations must stay on upstream Tabview (#1).

### Changed

- Rewrote the upstream Python viewer as a Rust CLI distributed as a single `tview` binary (#1).
- Breaking: renamed the command and configuration to `tview`. See the
  [migration guide](https://github.com/commandzero/tview/blob/v0.1.0/docs/migration.md).
- Large files can open without waiting for the entire input to load (#3).
- Sorting and filtering work consistently across data sources (#7).

### Added

- JSON and NDJSON input, including nested data (#3).
- Read-only SQLite browsing with custom SQL queries (#6).
- Optional Elasticsearch browsing through ES|QL (#7).
- Saved views for column layout, formatting, filters, sorting, and colors (#1, #2).
- Customizable color themes (#2).
- Table, JSON, and JSONL export, including the final view after interactive browsing (#4, #8).
- Fast table previews with `-n` / `--top-lines` (#10).
- Install with `--features all` to enable every feature (#11).

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

[Unreleased]: https://github.com/commandzero/tview/compare/v0.1.0...main
[0.1.0]: https://github.com/commandzero/tview/releases/tag/v0.1.0
[1.4.4]: https://github.com/Tabviewer/tabview/compare/1.4.3...1.4.4
[1.4.3]: https://github.com/Tabviewer/tabview/compare/1.4.2...1.4.3
[1.4.2]: https://github.com/Tabviewer/tabview/compare/1.4.1...1.4.2
[1.4.1]: https://github.com/Tabviewer/tabview/compare/1.4.0...1.4.1
[1.4.0]: https://github.com/Tabviewer/tabview/compare/1.3.0...1.4.0
[1.3.0]: https://github.com/Tabviewer/tabview/compare/1.2.0...1.3.0
[1.2.0]: https://github.com/Tabviewer/tabview/compare/1.1.0...1.2.0
[1.1.0]: https://github.com/Tabviewer/tabview/compare/1.0.1...1.1.0
[1.0.1]: https://github.com/Tabviewer/tabview/compare/1.0...1.0.1
