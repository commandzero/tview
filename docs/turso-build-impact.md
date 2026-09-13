---
type: Guide
title: Turso build and runtime impact
description: Recorded SQLite dependency, build, and runtime tradeoffs.
generated: { by: codex/gpt-6, at: 2026-09-12T17:14:16Z }
---

# Turso build and runtime impact

## Dependencies

The default `sqlite` feature enables Turso 0.7.1 and its mimalloc allocator.
Turso's other default features, including FTS, are disabled. SQLite and file
sources run background queries on the shared Tokio multi-thread runtime.
Disabling `sqlite` removes Turso and mimalloc, but retains Tokio.

## Recorded measurements

The release measurement below was taken from an incremental build state on macOS
in July 2026 with `cargo build --release --all-features`. The compiler, target,
linker, debug symbols, and build cache affect these measurements. They do not
predict the size or build time of a current release.

| Measurement | Value |
| --- | --- |
| Platform | macOS 26.5.2, Apple Silicon, `aarch64-apple-darwin` |
| Rust | 1.90.0, LLVM 20.1.8 |
| Turso | 0.7.1, defaults disabled; `mimalloc` enabled explicitly |
| Tokio features | `macros`, `rt-multi-thread`, `sync` |
| Allocator | mimalloc through Turso's explicit `mimalloc` feature |
| FTS | Disabled; Tantivy absent from the normal graph and lockfile |
| Release binary | 19,968,208 bytes, reported by `ls` as 19 MiB |
| Incremental release rebuild after runtime change | 7.15 seconds wall clock |

## Read-only access

See [SQLite sources](sqlite.md#read-only-access-and-supported-tables) for
supported tables. Tview disables Turso FTS because it does not use that
functionality.

The test suite uses a bundled reference SQLite build only as a development
dependency to create FTS5, RTree, shadow-table, and unavailable-module fixtures.
The pinned Turso build reports `no such module: fts5` for SQLite's `fts5` module
because Turso's optional `fts` feature is disabled. The reference fixture does
not ship in the release binary.

A private wrapper opens the database through Turso core with
`OpenFlags::ReadOnly` before creating a connection, verifies `PRAGMA query_only`
as defense in depth, and exposes only typed schema discovery, prepared query,
and row-fetch operations. Tview does not expose arbitrary SQL execution. This
storage-level boundary prevents rollback-to-WAL conversion, sidecar creation,
and writes to existing database or sidecar bytes.

## Historical test results

All-feature and no-default-feature builds and tests passed on the recorded macOS
platform. The no-default-feature dependency graph retains Tokio as the
application runtime while excluding Turso and mimalloc. Before the standard
runtime refactor, the all-feature test suite and all-target, all-feature Clippy
with warnings denied also passed on Fedora 44 x86_64 with Rust 1.95.0. Fedora's
optional `util-linux-script` package was represented by an isolated PTY shim for
the six integration tests that require the `script` command; no system packages
were installed. The host was unreachable during the runtime change, so that
Linux result was not refreshed. These are historical measurements. The [release
process](releases.md) defines the current supported targets and required checks.
