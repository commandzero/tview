# Conditional-color cutover evidence

Captured 2026-10-05T04:44:47+00:00, macOS arm64. Fixed baseline: `523002c8b3a61d9abe6a75404780474538306e7c`, rebuilt in an isolated worktree. Post: integrated ownership cutover, before final mechanical lint/API-shape cleanup. Both libraries and CLI binaries use Homebrew rustc 1.99.0, default features (`clipboard`, `saved-views`, `sqlite`); libraries are debug builds and standalone harnesses use `rustc -O --edition=2021`. This comparison replaces the earlier prebuilt-binary baseline, whose source revision was not established. Pinned repository/MSRV checks are separate verification, not these timing builds.

## Reproducible workload

Use an isolated XDG configuration containing the existing complete `cmdzro.yml` theme with mode `auto` and palette alias `"alpha;β:gamma,(x)": "#25A39AFF"`. Configure three columns: `id` identifiers using that alias, green and `palette(124)`; numeric `val` auto gradient with nine steps using alias/green/yellow; `marker` match with `warn`→alias and `ok`→green. Configured YAML color strings remain unchanged. The saved view has source `{}`, no filters or sorts, and filename pattern `*.csv`.

Generate two CSVs headed `id,val,marker`, 5,000 records `j=0..4999`. Repeated IDs: `identifier-${j%16}`; unique IDs: `identifier-${j padded to five digits}`. Values: `j%500`; marker: `warn` for even `j`, `ok` otherwise. For each Ansi16/Ansi256/TrueColor mode, classify the resident rows with viewport origin `(0,0)`, height 24, width 100 and apply the same configured columns.

Cold measures classify/bind plus prepared conditional-color construction (previously eager during binding; now explicitly prepared in that window). Warm measures ten sweeps × 5,000 rows × three columns: **150,000 rendered-cell/evaluator operations**. Baseline obtains the encoded color then resolves its style; post obtains the typed foreground from the corresponding prepared source-cell context. Both black-box rendered text and color/style. Viewport measures 100 × 24 × three = **7,200 prepared cell operations**, not Ratatui frames or FPS.

Full colored and subsequent plain windows include output preparation and table serialization to a sink. The new immutable projection requires a source-backed view; opening and binding that view are outside these windows. The old mutable-view writer and new projection are not identical micro-operations. Fresh CLI timings cover actual complete preparation and output in both versions. No timing assertion is committed.

## Median harness measurements

Three trials per IDs/mode, milliseconds; each pair is fixed baseline → post.

| IDs | Mode | Cold | Warm 150,000 cells | Full colored | Full plain after color |
| --- | --- | ---: | ---: | ---: | ---: |
| repeated | Ansi16 | 36.166 → 36.974 | 547.677 → 84.712 | 161.282 → 48.840 | 43.145 → 32.018 |
| repeated | Ansi256 | 35.268 → 37.074 | 473.941 → 85.119 | 148.112 → 50.248 | 42.450 → 35.043 |
| repeated | TrueColor | 35.712 → 37.088 | 463.874 → 82.105 | 147.172 → 51.111 | 43.215 → 32.279 |
| unique | Ansi16 | 44.593 → 39.138 | 562.488 → 95.067 | 174.221 → 55.523 | 45.097 → 34.733 |
| unique | Ansi256 | 43.913 → 38.749 | 483.761 → 94.587 | 157.851 → 55.880 | 44.984 → 35.439 |
| unique | TrueColor | 43.444 → 39.942 | 474.643 → 97.193 | 159.600 → 60.911 | 45.450 → 34.899 |

Viewport medians, two trials each, milliseconds:

| IDs | Mode | Baseline | Post |
| --- | --- | ---: | ---: |
| repeated | Ansi16 | 25.445 | 3.825 |
| repeated | Ansi256 | 22.106 | 3.850 |
| repeated | TrueColor | 21.457 | 3.775 |
| unique | Ansi16 | 25.697 | 4.369 |
| unique | Ansi256 | 22.151 | 4.214 |
| unique | TrueColor | 21.611 | 4.050 |

## Fresh CLI and byte equivalence

Three trials each. Invoke `--view color-bench --output table --color always|never`; ANSI16 uses `TERM=xterm`, ANSI256 uses `TERM=xterm-256color`, truecolor additionally uses `COLORTERM=truecolor`. Clear `NO_COLOR`, `FORCE_COLOR`, and unrelated color overrides. All **36 post invocations** exit zero with empty stderr and exactly match the corresponding fixed-baseline SHA-256 and byte count, including ANSI escapes. Plain hashes are identical across terminal modes for a given fixture.

| IDs | Mode | Colored ms before → after | Plain ms before → after | Colored bytes | Matching SHA-256 |
| --- | --- | ---: | ---: | ---: | --- |
| repeated | ansi16 | 198.134 → 73.919 | 71.168 → 58.165 | 305332 | `088173f5e8dd4abe7a616adb050107e72481968f0ecd843d196b0e39c4d674aa` |
| repeated | ansi256 | 183.944 → 73.730 | 70.847 → 55.833 | 331745 | `cd1c6fc10d685653f392ffe91cd7c9ee8f2a42004dbe480320f82db30fdc762c` |
| repeated | truecolor | 184.638 → 75.518 | 71.310 → 55.780 | 427111 | `2cb2bbab9774393c0ffe10d9235eb3837887542e1d761d2dc0001914a184c238` |
| unique | ansi16 | 213.285 → 78.209 | 73.559 → 57.879 | 323663 | `231593eb39009a1649aac458bf0d7c94635ac33dd2493f9bf912544c92f11a52` |
| unique | ansi256 | 200.862 → 79.287 | 73.660 → 58.173 | 347268 | `860536c0bebb2cab60bdd4de1e9f1f05151318e4c5adbba817ea42e3a2bb2ac0` |
| unique | truecolor | 197.053 → 82.825 | 74.054 → 58.256 | 441490 | `1858279d816bf5f8029fa1c08e3e1b2b7812045e674361a23b8f6e8d51b69a2e` |

## Interpretation and limits

The measured prepared-cell workloads are substantially faster on these fixtures; cold startup is not uniformly faster. This is a local debug-build observation, not a universal speedup or latency promise. Compiler/library/harness/fixture/color modes are matched. Scheduling, caches, and concurrent repository preflight compilation can affect results; no statistical confidence or allocation-count measurement was performed. Complete writer architecture differs, so its harness timings are corroborative; byte-identical fresh CLI results are the output-equivalence evidence.

Production computed-color transport and decoding were removed. Ordered compiled rules return `Option<Color>`; theme parsing and saved YAML retain configured strings. Exact source profiles are cached separately from compiled theme programs; scroll/repaint do not repeatedly reduce the same exact source domain. Prefix profiles use emitted rows; complete locally transformed projections use the selected resident domain and preserve the existing numeric parser and fallback semantics.

Harnesses, copied private-seam instrumentation, generated CSVs and raw JSON live only in temporary directories and are removed after evidence capture. No benchmark-only production API or fixture is committed. Measurements do not assert native-source latency, TUI FPS, unread-suffix validation, or a fixed preview scan bound.
