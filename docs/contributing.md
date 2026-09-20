---
type: Guide
title: Contributor guide
description: Ownership, compiler support, checks, and documentation boundaries.
generated: { by: openai-codex/gpt-6-astra, at: 2026-09-20T01:16:36Z }
---

# Contributor guide

CommandZero maintains this independently released Rust rewrite of Tabview. The
upstream MIT license and attribution remain in [LICENSE.txt](../LICENSE.txt).
The Rust library organizes the binary's code and has no separate API stability
promise. Keep one package unless another consumer needs it or measurements
justify a dependency split.

## Standards adoption

This repository adopts the applicable repository-management standards for its
Rust CLI and TUI. The local standards bundle starts at
`~/.agents/memory/repo-man/index.md`, relative to the contributor's home
directory. Follow the nearest parent AGENTS.md pointer if the location differs.
Obtain a usable local copy before changing policy. Draft standards do not change
product commitments. This guide records the standards adopted here.

## Compilers and features

The minimum supported Rust version is 1.90.0. The development and release
compiler is pinned in [rust-toolchain.toml](../rust-toolchain.toml). Use rustup
so the pin also governs developer commands. The shared check script explicitly
selects it, including when a package-manager Cargo installation appears first on
PATH.

Application builds and tests use the committed lockfile with `--locked`. CI
tests the minimum supported compiler separately. An MSRV increase requires a
minor version and a changelog entry. Before release, check all supported targets
and the selected features, including their dependencies.

Default releases include `saved-views`, `sqlite`, and `clipboard`.
`elasticsearch` is an opt-in source-build feature. All features coexist.
Preflight runs default, minimal, and all-feature tests plus compilation of each
source feature alone. This includes library doctests; no fixed coverage
percentage is required.

## Local preflight and CI

Install the versions in [tools-versions.sh](../scripts/tools-versions.sh):

```bash
rustup toolchain install 1.97.1 --profile minimal --component rustfmt,clippy
rustup toolchain install 1.90.0 --profile minimal
cargo install okf --version 0.2.7 --locked
npm install --global @fission-ai/openspec@1.11.0
bash scripts/preflight.sh
```

Also install ShellCheck, ripgrep, and Actionlint 1.7.7. CI installs Actionlint
through the Go toolchain. Bash 3.2 is the script compatibility target. CI calls
the same entry point. `docs`, `scripts`, `specs`, `rust`, and `msrv` select
individual check groups. Documentation-only PRs skip Rust compilation, while
workflow and check-script changes run full preflight. Failure output is
retained. CI cancels superseded PR runs and caches Cargo dependencies.

Use Conventional Commits for PR titles and squash commits. Prefer squash merge.
Keep historical commits unchanged. The required `Compliance` check covers
documentation, script checks, OpenSpec association, and applicable code checks.
Configure main-branch protection with the versioned ruleset after the workflows
exist remotely. The rules require passing checks and resolved review threads,
with no minimum reviewer count. Maintainers review compatibility and policy
changes.

## OpenSpec completion

Every PR body declares `OpenSpec changes: none` or a comma-separated list of
change IDs. Declare implementation associations even if no spec artifact
changed. Changed active paths and archive paths are also selected automatically,
including both sides of renames and deletions. Review the declaration as part of
PR review.

```bash
bash scripts/openspec-gate.sh origin/main HEAD add-elasticsearch-source
```

The gate compares committed HEAD to its merge base. Associated changes must have
one archive with completed tasks. Use `OPENSPEC_TELEMETRY=0 openspec archive <change-id> --yes` to validate and apply deltas before merging. The gate runs
OpenSpec's native `validate --archived` for the selected archives and `validate --specs --strict` for the final specifications. It does not reimplement
OpenSpec's Markdown parser.

Reviewers must check that the affected main requirements and scenarios reflect
the associated deltas, especially if archiving used `--skip-specs`. Native
archive task validation does not prove synchronization. A change without spec
deltas needs a reviewed `no-spec-deltas.md` explanation. Historical archives
stay as history when a later change updates the same requirement. Unrelated
active work does not block a PR. The gate never syncs or archives the
contributor's files.

## Unsafe code boundary

Rust denies unsafe code by default. Two scoped exceptions exist because the
standard library does not provide the required OS operations:

1. Windows terminal handle attachment, duplication, and restoration in
   [terminal.rs](../src/ui/terminal.rs). Owned duplicates must close once, borrowed
   process handles must remain open, and failed attachment must restore handles.
2. Unix PTY setup and signal delivery in
   [interactive_output.rs](../tests/interactive_output.rs), confined to tests.

New unsafe code outside these scopes is rejected. Changes inside either scope
need ownership/safety review and tests on that platform. Native Windows is not a
release target; adding it requires its own validation and support decision.

## Documentation bundle

The complete `docs/` directory is a flat OKF 0.2 bundle of authored guides and
the reserved index. OpenSpec workflow files live in `openspec/`, outside docs
validation and export. Keep build tools, schemas, and temporary reports outside
`docs/`.

Run `bash scripts/preflight.sh docs` for the complete bundle. It uses pinned
OKF, checks local authored links, and checks that every concept appears in the
index. There is no additional local frontmatter schema. Do not use automatic
fixes in CI. Preserve imported source bodies and attribution. Link to source
artifacts and record the author and time of each edit. Record verification only
for checks that ran.
