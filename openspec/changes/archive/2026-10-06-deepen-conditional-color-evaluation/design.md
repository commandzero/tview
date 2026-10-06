# Design

## Context

See [proposal.md](proposal.md#why) for motivation and the [color-themes delta](specs/color-themes/spec.md) for acceptance behavior. The glossary in [CONTEXT.md](../../../../CONTEXT.md) distinguishes the active source result, view transform, selected saved-view snapshot, frozen preview projection, and conditional color.

Today `TableView` stores `ConditionalColorRule` lists and `ColumnColorMetadata`. The latter contains extrema, rendered-identifier maps whose values are encoded strings, and vectors of encoded gradient buckets. `conditional_color_for_source_cell` selects a rule, including its own automatic-gradient arithmetic; `ConditionalColorRule::color_ref_for` repeats that arithmetic. UI rendering reads `VisibleCellStyleContext.conditional_color`; output calls `output_conditional_color`, which also renders a value already rendered by the output path. Both then call `ResolvedTheme::conditional_style`, which decodes the private transport, allocates color lists, walks aliases, interpolates, and converts terminal colors.

The existing useful responsibilities remain: saved views parse ordered domain-specific rules; `ResolvedTheme` validates theme configuration and supplies tokens/fallback; view/store preparation determines rows and presentation; UI rendering projects state; output adapters serialize prepared cells. Exact store reductions and resident-row profiles already supply reusable column facts, while previews rebuild profiles after freezing emitted rows. No measured performance regression has been established.

## Goals / Non-Goals

**Goals:**

- One conditional-color evaluation implementation for rule precedence, numeric/range/fixed-stop selection, bucket selection, identifier assignment, alias resolution, interpolation, and fallback.
- A small preparation-and-evaluation interface with borrowed cell inputs and copyable computed foreground results, shared by the two real consumers.
- Reusable compiled color data and scope-dependent facts, with explicit invalidation and color demand; no per-cell transport allocations or repeated alias walks after preparation.
- Tests at the same evaluation/rendering seam used by callers, not tests that pin a replacement private representation.

**Non-Goals:**

- No source adapter or query redesign, universal expression engine, plugin interface, new Cargo package, global cache, or generic configuration framework.
- No live theme reload, new theme/view YAML syntax, alpha blending, new number parser, identifier hashing, or different gradient mathematics.
- No ownership of saved-view discovery/binding, result activation, preview row selection/lookahead, or selection/search rendering policies.
- No whole-dataset cell-style materialization or promised universal speedup.

## Decisions

### 1. Keep configured rules separate from compiled runtime color data

Retain configured strings and ordered `ConditionalColorRule` values for YAML diagnostics and persistence. Add a compact compiled form inside the existing theme/conditional-color responsibility, using a private focused submodule only if it materially improves locality. Do not create another independently public color engine or facade over the old paths.

Compile each rule's configured color references against the session's immutable `ResolvedTheme`. Resolve exact palette names before literal parsing, preserving punctuation and Unicode. Reuse the existing configured/resolved color distinctions: direct ANSI-16, ANSI-256, and RGB values have different terminal conversion semantics, so converting everything to RGB first would change behavior. Alpha remains configuration data and is ignored for terminal output. For interpolation, resolve stops/families to RGB and reuse the current channel rounding and contrast-floor functions; then convert generated RGB to terminal colors.

Compiled rule payloads contain ordered match predicates and resolved foregrounds, ordered range predicates and foregrounds, sorted fixed stops and foregrounds, automatic-gradient bucket foregrounds, or identifier family/shade foregrounds. Fixed gradients remain stop-owned intervals, not interpolation. Automatic gradients with nonconstant extrema use the existing `floor(ratio * steps)` bucket selection clamped to the final bucket and interpolate over `bucket / (steps - 1)`. Preserve the separate constant-profile result: the first configured color resolved directly, which can differ from an RGB-generated color in a limited terminal. A one-step gradient preserves its existing generated-color path when the profile is nonconstant.

Use a copyable foreground-only runtime result, for example `Option<Color>` backed by Ratatui's existing color type. `None` means no conditional overlay; no cell-specific identifier/gradient serialization, parsing, or owned color strings cross the runtime seam. A foreground-only result cannot accidentally replace selection background or modifiers. Consumers perform only ordinary foreground overlay, not rule or color computation.

**Alternative rejected:** a typed `Identifier(index, colors)` / `Gradient(bucket, colors)` transport that leaves theme resolution in both render consumers. It removes string formatting but retains repeated alias/fallback work and split evaluation ownership. Also reject storing only RGB for configured literal colors, because it loses ANSI palette semantics.

### 2. Compile once; bind profiles to an explicit scope; evaluate cells cheaply

The evaluation responsibility has two operations, conceptually:

- Prepare the column rules with the loaded theme and applicable profile facts, returning reusable evaluation state and any existing configuration diagnostics.
- Evaluate one column's cell from borrowed raw and already-rendered text, returning a copyable conditional foreground.

These are conceptual operations, not a requirement for new public Rust names or traits. Put all rule selection and generated-color handling behind this interface. The view and frozen preview projection own the state for their respective result/presentation lifetimes; `ResolvedTheme` supplies theme/color facts but does not fetch rows, own view state, or render. UI/output never inspect compiled rule variants, indexes, or buckets.

The evaluator requests only profile facts required by configured rules. Match/range/fixed rules need the existing `NumericColumnProfile` where relevant but no identifier/extrema reduction. Automatic gradients need extrema; identifier rules need deterministic indexes of unique nonempty keys in the existing provider's domain. Resident/emitted rows derive identifiers from rendered values and extrema through their existing scalar/profile interpretation. Exact store reductions derive identifier keys from `CellValue::display()` with no rendered-key fallback; their typed integer/float extrema retain the current fallback to resident scalar-parsed extrema when the exact reduction has no numeric extrema. Cell identifier lookup still uses already-rendered text, so formatting can produce a store-backed lookup miss. Preserve that result and the existing numeric fallback without adding a formatting-aware reduction or extra rendered-key traversal. Providers supply existing facts to one evaluator; consumers do not choose rule semantics. Correcting the identifier discrepancy requires a separately scoped behavior change.

Two profile scopes are explicit:

| Scope | Owner supplies | Color evaluation must not do |
| --- | --- | --- |
| Complete configured active result | Active generation/schema, effective local view transform, applicable column presentation/numeric profile, complete-result reduction inputs | Re-query the source, expand its bound, or restrict profiling to the viewport |
| Emitted frozen preview projection | Frozen emitted rows/schema, applied presentation/numeric profile, preview-local profile inputs | Read lookahead/omitted/rejected values, reopen/complete a store, or alter remaining-row evidence |

Complete-result scope preserves the existing profiling domain: the selected transformed query store where that path is active, otherwise the source/resident row domain used by current source-column profiling. It is never all possible source rows or merely the viewport. Do not change resident/local-only filter membership or the provider's key/extrema interpretation as an incidental refactor; preparation passes the existing domain and interpretation explicitly. Only a genuinely changed scope/domain invalidates profile facts; scrolling and reordering within an unchanged domain do not renumber identifiers. Indexes use the current sorted unique profile keys, not encounter order or hashing. A changed key set may change indexes, so stability is promised within the same scope/key domain only.

The evaluator lazily parses a raw numeric scalar at most once per cell using the existing parser/profile if a reached rule requires it. Preserve boolean and string match semantics, typed numeric comparisons, ordered entries, all existing supported suffixes/placeholders, and lossless raw/persisted data. Do not parse rounded rendered numbers, alter the source `CellValue`, or add a stricter losslessness check/new scalar policy. Preserve existing nonfinite handling rather than inventing a new fallback.

Preparation occurs as part of normal presentation preparation/publication for a color-demanding consumer, not an extra renderer-side algorithm. Thread the immutable theme and color demand through those existing preparation seams; binding saved rules stores configuration without immediately scanning for color profiles. Reuse the already-rendered cell text in output, eliminating its redundant render and owned color-reference return. The same prepared evaluation path works for initial binding, late-schema binding, interactive presentation edits, complete output, and a frozen preview.

**Alternative rejected:** eagerly style every row/column at profile preparation. It adds row-count × column-count storage and ties the cache to transient selection/search state. Also reject requiring render consumers to separately calculate bucket indexes or perform theme resolution.

### 3. Separate reusable compilation from result-dependent facts

Keep cached data local to the active view or frozen projection, not process-global:

- Theme session data: resolved aliases and identifier family targets; no live reload key is needed.
- Compiled per-column rule data: literal terminal foregrounds, auto-gradient bucket colors, and the finite family-count × 16 identifier shade cycle. Reuse resolved references within preparation rather than walking the same aliases for each cell.
- Scoped per-column profile data: extrema, effective numeric parsing profile, and one provider-domain identifier-key → index map per column. Multiple identifier rules share indexes, with their own family/shade cycles. Do not store a separate encoded string per value per rule or a full style per dataset cell.

Profile-dependent data is associated with active result identity/generation, schema identity, effective presentation revision, and explicit profile domain/scope. Prefer the existing publication and metadata-change paths as invalidation owners over a new general cache-key framework. Invalidate before publishing or exposing changed data; refresh demanded color facts before the next consumer observes that presentation. Frozen projections own immutable profile/evaluation state and cannot reuse another projection's prefix facts by column index alone.

| Change | Required handling |
| --- | --- |
| Successful source result/reload replacement | Discard previous result-dependent facts; reconcile stable columns using the lifecycle's published schema; rebind demanded profiles before rendering |
| Failed/superseded request | Keep prior active result/color facts; do not color the prior result using requested-but-uncommitted data |
| Appended/discovered schema or late saved-column metadata | Bind new columns and refresh affected numeric/presentation/profile facts using stable source identity |
| Rule/family configuration change | Recompile affected rules and refresh profile facts required by their new kinds |
| Type, format, mask, or locale change | Refresh affected parsing/rendered-identifier facts; reuse unaffected compiled literal colors |
| Effective result membership/extrema/identifier set or profile scope change | Refresh scoped facts, including emitted-prefix → complete-result transitions |
| Scroll, cursor, selection, search, or repaint only | Reuse conditional evaluation state; overlay transient styling in the renderer |
| Plain table/JSON/JSONL output | Do not prepare color-only profiles or compiled programs; retain configured rules for later color demand/persistence |

No configured rules or only profile-independent rules must not trigger exact color-profile reductions. Other required indexing, sorting, filtering, or width work can still read rows; tests distinguish those needs from color-specific reads.

**Alternative rejected:** a new global memoization layer keyed by file paths or column indexes. It cannot safely express source generations, formatted identifiers, and prefix-versus-complete scope and is unnecessary for two session-local consumers.

### 4. Preserve failure behavior and UI/output separation

Keep structural/semantic YAML validation and existing nonfatal rule-warning behavior at their existing edge; theme selection failures still occur before entering the TUI or emitting stdout. The refactor must not add repeated render-time diagnostics, change warning/fatality policy, or turn an unresolvable color into an unrelated later rule's foreground. Internally distinguish a predicate that did not match from a matched rule/entry whose configured foreground is unavailable: preserve the current no-style outcome for the latter rather than skipping it during compilation and changing precedence. Preserve partial resolution as well: an unavailable family or gradient endpoint makes only the affected family shades or buckets unavailable, not every otherwise-resolvable color in the rule. Existing selected-theme alias cycle/missing-target failures remain validated once.

The UI composes normal cell token → conditional foreground → selected background/modifiers, preserving conditional foreground where currently supported; then search overrides only matching substring foreground/modifiers. Colored table output composes normal cell token → conditional foreground only. ANSI/control normalization, resets, output adapters, error-before-stdout behavior, plain output, and JSON/JSONL values remain unchanged.

**Alternative rejected:** moving selection/search style composition into a generic conditional-style engine. Those are consumer-specific transient concerns and would widen the interface without increasing conditional-color leverage.

### 5. Replace the old paths and test the real seam

Delete private `identifier_color_ref`, `gradient_color_ref`, length-prefixed color-list encoding/decoding, identifier/gradient reference parsers and transport structs, the string-valued view cache members, duplicated rule/bucket selection, and old string-returning view/theme helpers after migrating every caller. Preserve configured YAML color parsing, schema validation, persistence, interpolation, and terminal conversion functions that still earn their keep. Do not leave deprecated aliases, legacy private transport parsing, or forwarding wrappers.

Migrate tests asserting `identifier(...)`, `gradient(...)`, borrowing variants, or private list encodings to foreground/rendered-cell assertions. Keep valid parser/composer tests for user configuration and durable numeric/color semantics. Establish the behavior matrix below, using evaluation inputs plus actual UI buffers and prepared/ANSI output rather than compiled-variant snapshots:

| Area | Required assertions |
| --- | --- |
| Two consumers × three resolved terminal modes | Same conditional foreground for equivalent cells/rules/profile inputs under ANSI-16, ANSI-256, and truecolor; named/indexed/RGB direct colors and generated colors retain correct conversion |
| Precedence and failure | First matching rule and entry; no match; earlier range versus auto-gradient; matched unresolvable color does not accidentally fall through; invalid YAML warning behavior remains nonfatal |
| Aliases | Exact names with commas, semicolons, colons, parentheses, and multibyte Unicode; chains; literal-spelling collisions; existing cycle/missing-target validation |
| Gradients and numerics | Fixed stops below/at/between/above bounds; automatic default/custom/one-step buckets, endpoints, intermediate rounding, constant/all-nonnumeric scopes; percentages/unit/time profiles and raw-value preservation |
| Identifiers | Repetition, sorted unique ordering independent of row order, empty values, resident/emitted formatting-induced collisions, retained store-backed raw-key/rendered-lookup misses, automatic/explicit families, shade/family cycling and contrast floor |
| Presentation | Selected background/modifiers and conditional foreground; substring-only search foreground/modifiers; no transient TUI styles in batch output; no ANSI leakage into later plain cells/output |
| Invalidation and scopes | Changed schema/result/rules/type/format/locale; restoration/reload; failed replacement retains colors; preview omitted/rejected/lookahead extrema and lexicographically earlier identifiers do not affect emitted colors; complete scope includes offscreen facts |
| Demand and bounded work | Plain table/JSON/JSONL does not request color reductions; profile-independent rules do not request exact reductions; preview styling does not pull the source; no row-by-column eager style store |

**Deletion test:** deleting this evaluation responsibility would force rule order, scalar parsing use, scoped bucket/index selection, family shades, and alias/fallback resolution back into both consumers and the view. It earns depth. Deleting the private transport eliminates complexity outright; it must not merely relocate encoders/decoders. `ResolvedTheme`, saved-view configuration, the typed store, source/output adapter seams, and renderer-specific overlays remain useful and are not replaced.

### 6. Measure focused work without promising a speedup

Before implementation, capture a reproducible baseline for repeated styles in an already-prepared viewport and full colored output using identifier rules, automatic gradients, and aliases. Repeat after cutover with the same compiler/features, terminal modes, fixture, row/column counts, profile scope, and iterations. Separate cold preparation/profile cost from warm evaluation/rendering, and report allocations or throughput plus the measurement method and actual results. Plain output is a control and must not gain color-only profiling work. Include both repeated and high-cardinality identifier data to expose memory trade-offs.

Use an available allocation profiler or a focused local benchmark without new permanent telemetry or brittle elapsed-time test thresholds. Keep performance evidence with the change outside the docs bundle; retain no throwaway instrumentation. Structural removal of string encoding/decoding and per-cell alias work is required even if measurements show no throughput benefit. Report observed regressions honestly and address avoidable work; do not infer a universal frame-rate/export gain from this design.

## Risks / Trade-offs

- Direct literal fallback differs from generated RGB fallback → retain configured/resolved color distinctions and test ANSI-16/indexed/truecolor parity, including constant-gradient handling.
- Store-backed and resident profiles have different interpretation domains → preserve each provider's existing keys/extrema and numeric fallback, test retained rendered-lookup misses, and do not introduce presentation-sensitive exact-store reductions.
- Cache freshness spans source, late schema, and presentation changes → attach invalidation to their actual publication/binding paths and test observable foregrounds after each transition.
- Identifier maps grow with unique rendered values and gradient tables grow with configured steps → reuse one index map per column and finite family/shade cycles; avoid row-by-column styles and duplicated per-rule value strings. This change does not impose a new step limit or hashing policy.
- Profile acquisition can fail or require completing a bounded result → retain the existing preparation error contracts and errors-before-stdout; never silently broaden preview scope or invent approximate profiles.
- Reworking unsupported color references could alter precedence → preserve matched-but-unavailable outcomes, existing diagnostics, and configured rule order rather than filtering predicates during compilation.
- Performance benefit is unmeasured → record before/after evidence, not a timing guarantee; compare cold and warm paths separately.

## Migration Plan

1. Record baseline behavior and focused measurement inputs; introduce compiled rules and scoped evaluation inside the existing package.
2. Migrate initial/late column presentation and complete/frozen preparation to store configured rules separately from demand-prepared color state, with explicit invalidation.
3. Migrate both UI and output to the same copyable foreground result using already-rendered text; replace private-format tests with seam and consumer tests.
4. Remove obsolete transport/helpers/caches and all callers together. No user YAML migration is needed, and no private-format compatibility shim is retained.
5. Run behavior tests, the default/minimal/all-feature preflight and source-feature checks, and actual-binary smoke cases: a custom theme plus range/gradient/identifier saved view under controlled ANSI-16/ANSI-256/truecolor environments; selected/search TUI behavior and export; prefix output with omitted extreme/earlier identifier; complete colored output; plain/JSON/JSONL output with the same configured rules; reload/schema/presentation changes. Capture exit codes, stdout/stderr or PTY evidence and exact commands. Record post-cutover measurement results and update the existing theme/applicable saved-view guides and changelog only after proof.

Rollback, if needed before release, is a normal revert of the cohesive implementation commit(s), not a retained dual-runtime path. Planning artifacts do not authorize implementation, tests, main-spec synchronization, or archive.

## Dependencies and Overlap

Integration order is lifecycle → saved-view binding → conditional colors → frozen preview. Color evaluation first accepts existing complete/emitted profile facts; frozen preparation then consumes that typed evaluator. No new change directory is edited by this plan.

- Source lifecycle owns latest-only atomic result activation and committed source configuration. Colors react to successful activation; they neither commit requested source configuration nor own reload/persistence source truth.
- Saved binding owns the selected saved-view snapshot and initial/delayed column binding. It supplies configured rules and presentation revisions; colors own their compilation/evaluation. Existing saved-view requirements need no delta here.
- Preview owns row selection, accepted schema, lookahead, remainder evidence, and the frozen preview projection. It supplies emitted-only profile inputs and color demand; colors must not complete or detach its source. Both designs preserve no color-only profiling for plain output.
- `interface-updates` is an unimplemented editor proposal. Existing presentation mutation seams must remain sufficient; this change does not implement that editor or depend on speculative edit actions.

There are no unresolved behavior or ownership decisions. The specific local allocation/throughput measurement tool can be chosen during application without changing the contract; the evidence task is mandatory regardless of tool choice.

## References

1. [Color-themes main spec](../../../specs/color-themes/spec.md) and [saved-view main spec](../../../specs/saved-views/spec.md) — existing configuration, rule, family, and selection contracts.
2. [Archived theme design](../2026-07-05-add-color-themes/design.md) — foreground-only rules, numeric/family semantics, terminal fallback, no live reload.
3. [View color selection and cache](../../../../src/view/mod.rs), [theme resolution](../../../../src/theme.rs), [UI consumer](../../../../src/ui/mod.rs), and [output consumer](../../../../src/output.rs) — inspected runtime ownership and private transport.
4. [Shared numeric scalar parser](../../../../src/ops/sort.rs) and [typed column reductions](../../../../src/table/mod.rs) — reusable parsing/profile seams, not replacements.
5. [Non-interactive regression coverage](../../../../tests/non_interactive.rs) — emitted-preview profile isolation is covered by existing tests; those tests were not run during planning.
6. [Contributor guide](../../../../docs/contributing.md), [theme guide](../../../../docs/themes.md), and [saved-view guide](../../../../docs/saved-views.md) — feature validation and existing documentation targets.
7. [Interface-updates proposal](../../interface-updates/proposal.md) — editor work is separate and unimplemented.
