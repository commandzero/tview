---
type: Guide
title: Contributor guide
description: Setup, required checks, and implementation boundaries.
generated: { by: openai-codex/gpt-6.1-sol, at: 2026-10-07T03:44:02Z }
---

# Contributor guide

CommandZero maintains this independently released Rust rewrite of Tabview.
Preserve the upstream [MIT license and attribution](../LICENSE.txt). The library
organizes the binary's code and makes no separate API stability promise. Keep
one package unless another consumer or measurements justify a split.

## Standards adoption

Follow the applicable Rust CLI and TUI repository standards. The local bundle
starts at `~/.agents/memory/repo-man/index.md`, unless the nearest parent
AGENTS.md points elsewhere. Obtain a usable copy before changing policy.
Draft standards do not change product commitments; this guide records adoption.

## Compilers and features

Use rustup and the compiler pinned in [rust-toolchain.toml](../rust-toolchain.toml).
Preflight explicitly selects it even when another Cargo appears first on PATH.
Build and test with the committed lockfile and `--locked`.

The minimum supported Rust version is 1.90.0. CI tests it separately. Raising
it requires a minor release and changelog entry. Before release, check every
supported target and selected feature, including dependencies.

Default releases include `saved-views`, `sqlite`, and `clipboard`.
`elasticsearch` is opt-in. All features coexist. Preflight tests default,
minimal, and all-feature builds, including doctests, and compiles each source
feature alone. No fixed coverage percentage is required.

## Local preflight and CI

Install the pinned [tool versions](../scripts/tools-versions.sh):

```bash
rustup toolchain install 1.97.1 --profile minimal --component rustfmt,clippy
rustup toolchain install 1.90.0 --profile minimal
cargo install okf --version 0.2.7 --locked
npm install --global @fission-ai/openspec@1.11.0
bash scripts/preflight.sh
```

Also install ShellCheck, ripgrep, and Actionlint 1.7.7. Scripts target Bash 3.2.
CI uses the same preflight entry point and installs Actionlint through Go.
Pass `docs`, `scripts`, `specs`, `rust`, or `msrv` to run one check group.
Documentation-only PRs skip compilation; workflow and check-script changes run
full preflight. CI retains failures, cancels superseded runs, and caches Cargo.

Use Conventional Commits for PR titles and squash commits. Prefer squash merge
and preserve historical commits. The required `Compliance` check covers docs,
scripts, OpenSpec associations, and applicable code checks. Configure branch
protection from the versioned ruleset once workflows exist remotely. Require
passing checks and resolved threads, with no minimum reviewer count.
Maintainers review compatibility and policy changes.

## OpenSpec completion

Every PR body declares `OpenSpec changes: none` or a comma-separated list of
change IDs, including implementation associations without changed spec files.
The gate also selects changed active and archive paths, including renames and
deletions. Review the declaration and run:

```bash
bash scripts/openspec-gate.sh origin/main HEAD add-elasticsearch-source
```

The gate compares committed HEAD to its merge base. Before merging, archive each
associated change with completed tasks using
`OPENSPEC_TELEMETRY=0 openspec archive <change-id> --yes`.
The gate uses native `validate --archived` and `validate --specs --strict`.
It never syncs or archives files, and unrelated active work does not block a PR.

Review main requirements against the deltas, especially after `--skip-specs`.
Archive validation alone does not prove synchronization. Changes without deltas
need a reviewed `no-spec-deltas.md`. Preserve historical archives when later
changes update the same requirement.

## Source, binding, and output boundaries

- Commit source configuration only after loading and rebuilding succeed. Saves
  and reloads use that configuration, not pending requests. Reload supersedes
  pending work and preserves compatible state by source identity without
  rediscovering YAML. Latest activation failure must block final export.
- Use one validated saved-view snapshot per invocation. Apply CLI source
  precedence before opening, then bind column metadata before sorts and filters,
  including late columns. Distinguish missing or ambiguous references from
  unavailable numeric profiles. Deliver each warning once, never on stdout.
- Prepare output selection, late binding, accepted field presence, and remainder
  evidence before freezing a complete or prefix projection. Writers only
  serialize it; they must not fetch rows or mutate viewer settings, stores, or
  screen state. Sorting, numeric filters, and full schema scans can require full
  traversal. Local filters must not refill native source limits.
- Retain shared-store schema deltas after preparation failures and replay them
  on normal viewer progress. Do not publish partial output or alter frozen presentation.
- Resolve conditional-color strings consistently for the TUI and colored tables.
  Preserve aliases and rule order. Keep complete-result and preview profiles
  separate; plain table, JSON, and JSONL must not profile solely for color.
  Record color-work measurements in the active change. Never claim unmeasured speedups.

See [saved views](saved-views.md), [CLI output](cli-contract.md),
[large files](large-files.md), and [themes](themes.md) for user-facing behavior.

## Unsafe code boundary

Rust denies unsafe code except for:

1. Windows terminal handles in [terminal.rs](../src/ui/terminal.rs). Close owned
   duplicates once, keep borrowed process handles open, and restore handles on failure.
2. Unix PTY setup and signals in [interactive_output.rs](../tests/interactive_output.rs),
   confined to tests.

Changes in either scope require ownership and safety review plus native tests.
Reject unsafe code elsewhere. Windows release support requires a separate decision.

## Documentation bundle

`docs/` is a flat OKF 0.2 bundle with an index. Keep OpenSpec files in `openspec/`
and tools, schemas, and temporary reports outside `docs/`.
Run `bash scripts/preflight.sh docs` to validate the complete bundle, local links,
and index coverage. No additional frontmatter schema is required.
Preserve imported text and attribution, link source artifacts, and record each
edit's author and time. Record only checks that ran; do not autofix in CI.
