---
type: Guide
title: Installation
description: Homebrew and Cargo installation, optional features, and local builds.
generated: { by: openai-codex/gpt-6.1-sol, at: 2026-10-07T04:09:31Z }
---

# Installation

## Homebrew

Install from the [CommandZero tap](https://github.com/commandzero/homebrew-tools):

```sh
brew tap commandzero/tools
brew install tview
```

The formula installs prebuilt releases for macOS 14 or later on Apple Silicon
and Ubuntu 24.04 or later on Linux amd64 or arm64 with glibc 2.39 or later.
Intel macOS and a source-build fallback are not supported by the formula.
Use Cargo below for Elasticsearch support or custom feature selection.

## Cargo

Install from crates.io with Rust 1.90.0 or later:

```sh
cargo install tview
```

See [release platforms](releases.md#platform-and-artifact-contract) for native
archive targets and support floors. For an existing Tabview installation,
follow the [migration guide](migration.md).

## Optional features

Saved views, SQLite, and clipboard support are enabled by default.
Elasticsearch support, including Elastic CLI `.elasticrc` context sources
such as `.production.es://logs-*`, requires the optional `elasticsearch`
feature. It is not enabled in default, minimal, or Homebrew builds:

```sh
cargo install tview --features elasticsearch
```

Enable every feature, including Elasticsearch, with `all`:

```sh
cargo install tview --features all
```

The `all` feature also works with `--no-default-features`.

To omit default features or select them individually, use:

```sh
cargo install tview --no-default-features
cargo install tview --no-default-features --features saved-views
cargo install tview --no-default-features --features sqlite
cargo install tview --no-default-features --features clipboard
```

## From a checkout

Install with the repository's pinned toolchain and lockfile:

```sh
cargo install --path . --locked
```

See [contributing](contributing.md) for tool setup and preflight checks.
