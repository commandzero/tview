## 1. CLI and saved views

- [ ] 1.1 Add explicit boolean sorting and positive preview-count options; verify defaults, aliases, invalid values, automatic table mode, and incompatible modes in CLI tests.
- [ ] 1.2 Suppress saved view sort application before any expensive work, including pending columns; verify source ordering, filters, formatting, and saved files remain intact.

## 2. Prefix preparation

- [ ] 2.1 Pass preview preparation through source opening and output orchestration; use read-count tests above and below the lazy threshold to verify early stopping.
- [ ] 2.2 Support incremental CSV, NDJSON, JSON arrays, keyed objects, and nested pointer selection for previews; verify logical multiline records, prefix schema, bounded read-ahead, and malformed unread suffixes.
- [ ] 2.3 Implement filter-before-limit and exact enabled-sort semantics; verify selective filters, sorted rows near EOF, and source limits without refilling.
- [ ] 2.4 Finish stdin previews once the prefix and lookahead are ready; verify with a producer that keeps its pipe open after supplying enough rows.
- [ ] 2.5 Integrate SQLite and Elasticsearch prefixes without unsafe limit pushdown or extra count queries; verify filtered results and native source limits with adapter tests.

## 3. Table rendering

- [ ] 3.1 Freeze preview schema, widths, formatting, and color profiles from selected rows; verify late fields, wide omitted cells, full-schema overrides, explicit widths, gradients, and plain-output profiling.
- [ ] 3.2 Emit exact or unknown remaining-row summaries; verify filtered counts, EOF boundaries, headerless and empty output, newline framing, and ANSI reset behavior.
- [ ] 3.3 Preserve pre-output errors, broken pipes, and write failures; verify complete table output, JSON, JSONL, and interactive export regressions remain green.

## 4. Documentation and validation

- [ ] 4.1 Document the preview command, row-count semantics, unknown summary, stdin behavior, and costs of sorting, filtering, and full-schema scans; validate documentation links and the OKF bundle.
- [ ] 4.2 Record cold-process preview measurements on large CSV, NDJSON, and JSON files; compare read counts and elapsed time against complete table output and verify increasing the unread suffix does not increase unsorted prefix work.
- [ ] 4.3 Run repository preflight and strict OpenSpec validation; record results and resolve failures before review.
