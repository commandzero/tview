---
type: Guide
title: Installation
description: Cargo installation, optional features, and local builds.
generated: { by: codex/gpt-6, at: 2026-09-12T17:14:16Z }
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

Saved views and SQLite are enabled by default. Clipboard and Elasticsearch
support are opt-in:

```sh
cargo install tview --features clipboard
cargo install tview --features elasticsearch
cargo install tview --features clipboard,elasticsearch
```

Omit the default features for a smaller build, or select them individually:

```sh
cargo install tview --no-default-features
cargo install tview --no-default-features --features saved-views
cargo install tview --no-default-features --features sqlite
```

## From a checkout

Install with the repository's pinned toolchain and lockfile:

```sh
cargo install --path . --locked
```

See [contributing](contributing.md) for tool setup and preflight checks.
