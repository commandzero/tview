---
type: Guide
title: Release process
description: Release checks, supported targets, publication, and recovery.
generated: { by: openai-codex/gpt-6.1-sol, at: 2026-10-07T03:44:02Z }
---

# Release process

## Release proposal

Prepare a reviewed PR with matching Cargo.toml and Cargo.lock versions, a dated
changelog section, and any migration, compatibility, or support changes.
Use the package version and a `v<version>` tag, including prerelease suffixes.
Do not publish from `Unreleased`.

After merge, tag the reviewed main-branch commit and dispatch
[release.yml](../.github/workflows/release.yml). It checks tag, version, and
changelog agreement; runs preflight and native tests; builds with the pinned
compiler and lockfile; smoke-tests packages; and creates a draft from the changelog.

## Platform and artifact contract

| Target | Native build and test host | Support floor |
| --- | --- | --- |
| aarch64-apple-darwin | macOS 14 arm64 | macOS 14 |
| x86_64-unknown-linux-gnu | Ubuntu 24.04 x86_64 | Ubuntu 24.04, glibc 2.39 |
| aarch64-unknown-linux-gnu | Ubuntu 24.04 arm64 | Ubuntu 24.04, glibc 2.39 |

Every target must pass native build and test checks. WSL follows the Linux
requirements. Windows, Intel macOS, and musl are not release targets. Add targets
only with tested demand; a Rust triple alone does not establish Linux ABI support.

Archives are named `tview-v<semver>-<rust-target-triple>.tar.gz` and contain
`tview`, `LICENSE.txt`, and `BUILD-INFO.txt` at the root. Build metadata records
tag, commit, compiler, target, host OS, support floor, and features.
Default features are `saved-views,sqlite,clipboard`; Elasticsearch is source-build
opt-in with separate preflight coverage. Each `.sha256` file contains the archive
hash and basename. Packaging extracts each archive and checks the executable,
version, and offline stdin-to-table output. Checksums do not identify a signer.

## Publish and recover

The workflow creates a draft with all three archives and checksums. It refuses
existing releases and never overwrites assets. A maintainer reviews every target
and the notes before publication. Mark versions with prerelease suffixes as prereleases.

Crates.io is the primary channel. Publish from the same reviewed tag as the
archives, after all checks pass. Preflight runs `cargo publish --locked --dry-run`.
An authorized maintainer publishes with:

```bash
cargo publish --locked
```

For the first release, check the published installation:

```bash
cargo install tview --version 0.1.0 --locked --root /tmp/tview-release-check
```

Run its version and offline stdin smoke checks. Also verify `cargo install tview`,
then publish the GitHub draft. Keep credentials in Cargo's credential store or
CI secrets, never in the repository.

If draft creation or upload partly fails, compare existing checksums and upload
only missing verified files. Never use `--clobber`. If rebuilt bytes differ,
retain existing assets and release a new version. Never move a published tag or
republish different source under the same version. A GitHub failure does not
justify removing a published crate; repair missing assets from the reviewed source.
Any future Homebrew updater must verify hashes and layout, open a formula PR,
and pass supported-host installation tests before merge.

## Local release checks

```bash
bash scripts/release-check.sh v0.1.0
bash scripts/release-package.sh aarch64-apple-darwin dist
```

The first requires a dated changelog section and a tag at HEAD. The second builds
and tests an archive without uploading. Neither replaces the native target checks.
