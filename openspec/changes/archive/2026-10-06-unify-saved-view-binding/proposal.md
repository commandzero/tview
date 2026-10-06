# Proposal

## Why

Startup currently discovers and selects saved views twice, while initial and late-schema binding separately translate the same presentation, sort, and filter configuration. Recursive view bundles make the repeated discovery concrete work; retaining one validated snapshot and one schema-aware binding responsibility prevents mixed source/presentation snapshots and makes completion warnings consistent.

## What Changes

- Select and retain one invocation-local validated saved view, its canonical filename identity, authoring target, and discovery/selection/validation warnings before source opening; use that same snapshot for post-open presentation without rereading or rediscovering YAML.
- Unify initial and delayed binding of canonical column settings, ordered view sorts, view filters, type metadata, null inheritance, and provisional-schema pending state.
- Correct ambiguous structured operation references so they never fall through to permissive header matching, and report missing or invalid binding items consistently and once through existing interactive and batch diagnostic routes.
- Preserve recursive bundles, deterministic duplicate-name and filename-match precedence, platform case rules, explicit CLI source precedence, forced-missing errors, disabled authoring/discovery, and saved-sort-only suppression by `--sorted false`.
- Remove the second discovery and duplicate startup/late operation-translation loops. Keep reload restoration of active runtime settings separate from a fresh invocation; do not silently reload YAML or introduce global caching.

## Capabilities

### New Capabilities

- None.

### Modified Capabilities

- `saved-views`: Require invocation-consistent saved-view selection, unified schema-aware presentation/operation binding, and once-only completion/application diagnostics while preserving existing discovery, override, source/view layering, and disabled-mode contracts.

## Impact

- Affected implementation areas: `src/lib.rs` startup selection/application and diagnostic delivery, `src/saved_views/mod.rs` owned selection and schema-aware binding, and `src/view/mod.rs` schema-delta/pending binding integration and settings restoration.
- Behavior coverage: existing saved-view parser/discovery tests, view/schema behavior tests, and `tests/non_interactive.rs`, plus actual-binary preview and TUI reload smoke scenarios during implementation.
- Future implementation documentation: applicable saved-view/output guides and changelog; YAML schema, inherited profiles, feature names, package layout, and dependencies remain unchanged.
- Source-result activation, committed source configuration, reload source selection, and source persistence truth belong to `own-source-result-lifecycle`; preview selection/freezing and conditional-color computation belong to their separate changes. This change passes selected source overrides to opening and bound view settings to those consumers, without taking over their responsibilities.
