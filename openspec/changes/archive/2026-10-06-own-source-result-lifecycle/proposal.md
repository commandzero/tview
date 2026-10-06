# Proposal

## Why

A reproduced SQLite failure retains the last successful table but saves the rejected native query, so reopening the saved view fails. Source Configuration, replacement activation, reload, persistence, and final export currently reconcile requested and active state independently; one committed source configuration must advance only with a successfully activated source result.

## What Changes

- Deepen source-result lifecycle ownership around the existing typed `TableDefinition`/`TableStore` and revisioned query coordinator: drafts, pending requests, and committed source configuration become distinct states.
- Publish committed source configuration together with adapter-derived active query identities, schema, rows, extent, provenance, compatible view state, and cursor/mark reconciliation only after result loading and view reconstruction succeed.
- Save the last successful source configuration during pending or failed replacement; reload that same configuration, superseding pending work so it cannot later publish.
- Preserve opening settings, selected relation or native base query, source operations, finite native limits, and existing unbounded file-source omission; do not introduce native unbounded-limit serialization.
- Prevent unsafe positional migration of identity-backed column state across changed schemas; preserve adapter-proven keyed rows and reset row-bound state when identity is unavailable.
- Make latest-revision execution, loading, and reconstruction failures equally authoritative for final export while preserving blocking-worker ownership and asynchronous supersession/shutdown.
- Remove eager `App.open_options` mutation and caller-side reconstruction of save/reload authority instead of retaining forwarding compatibility paths. Existing CLI, YAML structure, terminal commands, and source/view semantics remain unchanged apart from the demonstrated correctness fixes; reload errors continue to end the interactive session rather than gaining a new non-fatal route.

## Capabilities

### New Capabilities

- None.

### Modified Capabilities

- `table-source-model`: Extend atomic asynchronous source replacement to committed source configuration and successful result activation; specify reload authority, safe identity reconciliation, revision outcomes, and worker lifetime.
- `saved-views`: Serialize source configuration from the last successfully activated source result, preserving opening settings, generated/native selection, base-query separation, and effective limits.
- `non-interactive-output`: Add a precise source-neutral final-export requirement covering failures after query execution succeeds but result loading or view reconstruction fails.

## Impact

The implementation affects source orchestration and modal/save/reload call sites in `src/lib.rs`, activation and restoration in `src/view/mod.rs`, the coordinator and result contract in `src/table`, and source adapter integration as needed to retain effective opening configuration. Existing adapters and output adapters remain real seams within one Cargo package; no universal query AST, plugin framework, new YAML format, UI editor feature, or crate split is introduced.

This change precedes saved-view binding, conditional-color deepening, and frozen-preview preparation in dependency-safe application order; the architecture priority ranking remains lifecycle, binding, preview, color. It owns committed source configuration and source-result activation, not saved-view discovery/binding, preview selection, or conditional-color computation. Future implementation must add consumer-visible regressions and runtime smoke evidence, then update applicable user documentation and the changelog; this proposal does not implement or run those checks.

## References

1. [Source request, Saved View, and reload call sites](../../../../src/lib.rs): `handle_source_config_key`, `handle_saved_view_key`, and `App::reload`; [replacement activation and serialization](../../../../src/view/mod.rs): `request_source_query`, `poll_source_query`, and `to_saved_view_yaml`. The reproduced query/save/replay sequence is preserved in the [saved-view delta](specs/saved-views/spec.md).
2. [Canonical domain vocabulary](../../../../CONTEXT.md).
3. [Current source-model requirements](../../../specs/table-source-model/spec.md), [saved-view requirements](../../../specs/saved-views/spec.md), and [output requirements](../../../specs/non-interactive-output/spec.md).
4. [Elasticsearch/native-query design](../2026-09-06-add-elasticsearch-source/design.md), [SQLite design](../2026-07-26-add-turso-sqlite-support/design.md), and [active interface-updates proposal](../../interface-updates/proposal.md).
