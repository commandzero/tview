# Proposal

## Why

Conditional colors currently cross the view/theme seam as private `identifier(...)` and `gradient(...)` strings that both the TUI and colored table output decode and resolve. One cohesive evaluation responsibility can preserve visual behavior while removing this runtime transport, duplicated gradient selection, and reusable work from per-cell rendering; any allocation or throughput improvement remains to be measured, not assumed.

## What Changes

- Compile configured column color rules against the loaded theme once per relevant configuration and terminal mode, preserving YAML color strings only as configuration and persistence data.
- Evaluate ordered match/range rules, fixed and automatic gradients, identifier families, aliases, and terminal fallback through one typed foreground/style interface used by both rendering consumers.
- Make profile scope explicit: emitted preview rows versus the existing complete-result profiling domains. Preserve numeric parsing, raw-value losslessness, each path's existing identifier/extrema interpretation, selection readability, and substring-only search styling. Do not silently correct store-backed raw-profile versus rendered-lookup discrepancies as part of the ownership refactor.
- Replace computed-string caches with reusable resolved rule colors, gradient buckets, identifier family shades, and scoped identifier indexes. Invalidate result-dependent facts on active result/schema, presentation, or profile changes; do not eagerly style every dataset cell or perform color profiling for plain output.
- Remove the private computed-color encoders/decoders, redundant selection paths, and their format-pinning tests rather than retaining forwarding wrappers. Retain valid YAML parser and color conversion tests and add consumer-visible parity and invalidation coverage.

## Capabilities

### New Capabilities

- None.

### Modified Capabilities

- `color-themes`: Add cohesive conditional-color evaluation requirements covering consumer parity, configured aliases, profile scope, and reusable evaluation without changing the supported theme or saved-view YAML format.

The existing `saved-views` conditional-rule requirements already specify rule semantics, ordering, and preservation of values. They remain unchanged; discovery and initial/delayed binding are outside this change.

## Impact

- Affected runtime areas: `src/theme.rs`, conditional color/profile ownership in `src/view/mod.rs`, `src/ui/mod.rs`, `src/output.rs`, and the existing saved-view color configuration/persistence seam where needed to preserve configured strings.
- Tests: theme/view/render/output behavior and non-interactive profile scope; actual-binary TUI and colored/plain output smoke coverage. Future implementation records focused before/after allocation or throughput evidence without timing assertions or unsupported speedup guarantees.
- Documentation: update existing theme and applicable saved-view guides plus the changelog after implementation proof. No new package, dependency framework, YAML schema, global cache, live theme reload, source-query semantics, or editor work.
- Integration order: source-result lifecycle → saved-view snapshot/binding → conditional colors → frozen preview preparation. Color evaluation first consumes existing complete/emitted profile facts; frozen preparation then consumes the typed evaluator. This change does not own publication, discovery/binding, or preview selection.

## References

1. [View conditional-color selection/cache](../../../src/view/mod.rs), [theme decoding/resolution](../../../src/theme.rs), [UI consumer](../../../src/ui/mod.rs), and [output consumer](../../../src/output.rs) — inspected sources establish the private string round trip; performance impact has not been measured.
2. [Current color-theme requirements](../../specs/color-themes/spec.md) and [saved-view requirements](../../specs/saved-views/spec.md) — compatibility contracts.
3. [Archived color-theme design](../archive/2026-07-05-add-color-themes/design.md) — data-specific rules remain with columns; theme styles remain a projection; no live reload.
