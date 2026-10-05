# Proposal

## Why

Preview preparation currently depends on a temporary `TableView` mode, whole-view clone/restore checkpoints, and removing its stores before serialization [1]. Give configured source result → frozen projection preparation one owner so bounded selection, accepted schema, delayed filters, lookahead, and presentation cannot escape into startup or output callers; this addresses observed structural coupling without claiming a reproduced preview defect or measured performance regression.

## What Changes

- Establish one preparation interface for complete output and direct table previews, consuming an active source result plus configured local view facts and returning an immutable prepared projection with remainder evidence.
- Own streaming versus complete traversal, delayed operation resolution, accepted-row schema/presence, required matching lookahead, and selected-row width/color profile inputs inside preparation. Preserve exact whole-result sorting and numeric-filter profiling when required.
- Remove `preview_preparing`, defer-before-settings orchestration, broad `TableView` clone/restore checkpoints, and successful store detachment as an output-preparation precondition. Preparing output does not mutate the live view's configuration, stores, viewport, or screen state.
- Retain existing sequential JSON/delimited `TableStore` adapters, `PreviewSourceStore`, typed `TableDefinition`, source-native bounds, and table/JSON/JSONL output adapters; output writers receive frozen facts and cannot fetch/profile source rows.
- Preserve current CLI/YAML behavior, filtered full-schema semantics, header/empty-result behavior, exact/unknown summaries, widths/colors/escaping, errors-before-stdout, unread-suffix behavior, and early stdin closure. No fixed-latency claim is added.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `non-interactive-output`: Require actual projection preparation ownership and immutable complete/preview outputs rather than a mutable viewer lifecycle; make delayed-schema, filtered-presence, profiling, and traversal obligations explicit while preserving existing scenarios.

## Impact

- Expected code areas: `src/lib.rs`, `src/view/mod.rs`, `src/output.rs`, and a cohesive preparation module within the existing package. Existing table/ingest seams remain real dependencies, not parser replacement targets.
- Consumers: direct table output and complete table/JSON/JSONL output, including final interactive export. Existing runtime tests in `tests/non_interactive.rs` and `tests/interactive_output.rs` will exercise the same external behavior.
- Integration: source-result lifecycle supplies an activated result after required latest-revision waiting; saved-view binding supplies one selected saved-view snapshot and schema-aware configuration; conditional-color evaluation supplies rule resolution and styles using this change's selected-row profile inputs. This change owns none of those sibling responsibilities.
- No package, dependency, CLI flag, YAML format, universal query AST, global cache, or plugin framework is introduced. Applicable output/developer documentation and changelog are updated during implementation after behavioral proof, not during this planning change.

## References

1. [Current preview preparation](../../../src/view/mod.rs), `prepare_preview` and `preview_preparing` guards.
2. [Archived fast-preview design](../archive/2026-09-13-fast-table-preview/design.md).
3. [Current non-interactive output contract](../../specs/non-interactive-output/spec.md).
4. [Domain vocabulary](../../../CONTEXT.md).
5. [Contributor standards](../../../docs/contributing.md).
