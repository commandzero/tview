---
type: Guide
title: Release process
description: Reviewed tags, native packaging, checksums, and publication recovery.
generated: { by: codex/gpt-6, at: 2026-09-12T17:14:16Z }
---

# Release process

## Release proposal

Prepare a reviewed PR that updates Cargo.toml, Cargo.lock, a dated changelog
section, migration notes, and compatibility and support changes together. Use
the package version in Cargo.toml as the release version. Use `v<version>` tags,
including prerelease suffixes. Set the release date in the proposal. Do not
publish an Unreleased section.

After merge, create the accepted tag on that reviewed main-branch commit.
Dispatch [release.yml](../.github/workflows/release.yml) with that tag. The
workflow checks tag/version/changelog agreement, runs preflight and native
platform tests, builds with the lockfile and pinned compiler, packages and
smoke-tests every target, and creates a draft release from the curated
changelog. It never overwrites assets.

## Platform and artifact contract

| Target | Native build and test host | Support floor |
|---|---|---|
| aarch64-apple-darwin | macOS 14 arm64 | macOS 14 |
| x86_64-unknown-linux-gnu | Ubuntu 24.04 x86_64 | Ubuntu 24.04, glibc 2.39 |
| aarch64-unknown-linux-gnu | Ubuntu 24.04 arm64 | Ubuntu 24.04, glibc 2.39 |

Every target must pass its native build and test job before release. WSL uses
the Linux userspace contract. Native Windows, Intel macOS, and musl are not
advertised release targets. Expand the matrix only with tested demand.

Each archive is `tview-v<semver>-<rust-target-triple>.tar.gz`. Files appear
directly at its root: `tview`, `LICENSE.txt`, and `BUILD-INFO.txt`. The metadata
records tag, commit, compiler, target, host OS, support floor, and enabled
features. Default binary features are `saved-views,sqlite`. Optional
Elasticsearch and clipboard support remain source-build options with their own
preflight coverage.

Each `.sha256` sidecar contains the archive hash and basename. Packaging
extracts the archive into a temporary directory, verifies executable/version
agreement, and runs an offline stdin-to-table smoke test. Never infer broad
Linux ABI compatibility from the Rust target triple alone.

## Publish and recover

The workflow stops at a draft containing all three archives and their checksum
files. A maintainer checks every target and the release notes before
publication. Set the prerelease flag for versions with a prerelease suffix.
Checksums detect changed bytes but do not identify the signer.

An existing release causes the workflow to fail before upload. If draft creation
or upload fails partially, inspect the draft and compare existing checksums.
Upload only missing, verified files to that draft; never use `--clobber`. If
rebuilt bytes differ, retain the existing assets and prepare a new version
instead of replacing them. Do not move an accepted published tag.

Crates.io is the primary distribution channel; the user installation command is
`cargo install tview`. Publish the crate from the same reviewed tag as the
native archives. The release preflight runs `cargo publish --locked --dry-run`
to validate the package before either channel is published.

After all release checks pass, an authorized maintainer publishes to crates.io:

```bash
cargo publish --locked
```

For the first release, verify the published package with `cargo install tview --version 0.1.0 --locked --root /tmp/tview-release-check` and run the installed
binary's version and offline stdin smoke checks. Also verify the ordinary `cargo install tview` command. Then publish the prepared GitHub release. Registry
credentials belong in the maintainer's Cargo credential store or CI secrets,
never in this repository.

If a later channel fails, retain the published crate and repair the missing
GitHub assets using the same reviewed source. Never republish different source
under the same version. A future Homebrew updater must verify hashes and layout,
open a formula PR, and pass supported-host installation tests before merge.

## Local release checks

```bash
bash scripts/release-check.sh v0.1.0
bash scripts/release-package.sh aarch64-apple-darwin dist
```

The first command requires a dated release section and a tag pointing at HEAD.
The second builds and tests an archive locally without uploading it.
