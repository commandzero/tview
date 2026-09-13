## 1. CLI and saved views

- [x] 1.1 Add explicit boolean sorting and positive preview-count options; verify defaults, aliases, invalid values, automatic table mode, and incompatible modes in CLI tests.
- [x] 1.2 Suppress saved view sort application before any expensive work, including pending columns; verify source ordering, filters, formatting, and saved files remain intact.

## 2. Prefix preparation

- [x] 2.1 Pass preview preparation through source opening and output orchestration; use read-count tests above and below the lazy threshold to verify early stopping.
- [x] 2.2 Support incremental CSV, NDJSON, JSON arrays, keyed objects, and nested pointer selection for previews; verify logical multiline records, prefix schema, bounded read-ahead, and malformed unread suffixes.
- [x] 2.3 Implement filter-before-limit and exact enabled-sort semantics; verify selective filters, sorted rows near EOF, and source limits without refilling.
- [x] 2.4 Finish stdin previews once the prefix and lookahead are ready; verify with a producer that keeps its pipe open after supplying enough rows.
- [x] 2.5 Integrate SQLite and Elasticsearch prefixes without unsafe limit pushdown or extra count queries; verify filtered results and native source limits with adapter tests.

## 3. Table rendering

- [x] 3.1 Freeze preview schema, widths, formatting, and color profiles from selected rows; verify late fields, wide omitted cells, full-schema overrides, explicit widths, gradients, and plain-output profiling.
- [x] 3.2 Emit exact or unknown remaining-row summaries; verify filtered counts, EOF boundaries, headerless and empty output, newline framing, and ANSI reset behavior.
- [x] 3.3 Preserve pre-output errors, broken pipes, and write failures; verify complete table output, JSON, JSONL, and interactive export regressions remain green.

## 4. Documentation and validation

- [x] 4.1 Document the preview command, row-count semantics, unknown summary, stdin behavior, and costs of sorting, filtering, and full-schema scans; validate documentation links and the OKF bundle.
- [x] 4.2 Record cold-process preview measurements on large CSV, NDJSON, and JSON files; compare read counts and elapsed time against complete table output and verify increasing the unread suffix does not increase unsorted prefix work.
- [x] 4.3 Run repository preflight and strict OpenSpec validation; record results and resolve failures before review.

## Validation

`bash scripts/preflight.sh` passed on 2026-09-13, including documentation and
script checks, strict main-spec validation, Clippy, and default, minimal, and
all-feature Rust tests. The all-feature run passed 420 unit tests, 6 interactive
tests, and 37 non-interactive tests. Six live Elasticsearch tests require an
external service and stayed ignored; mocked Elasticsearch preview tests passed.

`OPENSPEC_TELEMETRY=0 openspec validate fast-table-preview --strict` passed.
See [preview measurements](performance.md) for timings and bounded-reader checks.
