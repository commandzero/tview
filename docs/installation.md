---
type: Guide
title: Installation
description: Cargo installation, optional features, and local builds.
generated: { by: openai-codex/gpt-6-astra, at: 2026-09-20T01:21:51Z }
---

# Installation

Install from crates.io with Rust 1.90.0 or later:

```sh
cargo install tview
```

See [release platforms](releases.md#platform-and-artifact-contract) for native
archive targets and support floors. If you used Tabview, follow the [migration
guide](migration.md).

## Optional features

Saved views, SQLite, and clipboard support are enabled by default.
Elasticsearch support is opt-in:

```sh
cargo install tview --features elasticsearch
```

Enable every feature, including Elasticsearch, with `all`:

```sh
cargo install tview --features all
```

The `all` feature also works with `--no-default-features`.

Omit the default features for a smaller build, or select them individually:

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
