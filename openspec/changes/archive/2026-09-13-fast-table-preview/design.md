## Context

See [the proposal](proposal.md) for the preview use case. `src/output.rs` currently asks for `complete_rows` and `stable_widths` for table output. A serializer-only limit would still pay the full ingestion, sort, and profiling cost. The existing incremental stores already distinguish exact and partial row counts, but source opening can still eagerly read smaller inputs or scan JSON schemas before output preparation.

## Goals and non-goals

Make the unsorted preview path stop reading once it has enough matching rows and bounded lookahead. Keep output without a limit compatible. This change does not promise a fixed latency for arbitrary queries, filters, or nested JSON paths.

## Decisions

### Resolve preview options before opening a source

Use invocation-only options. Explicit `--sorted` or `--top-lines` requires resolved direct table output. `--sorted` defaults to true but its implicit default must not invalidate other modes. Require a positive integer for the limit. Do not change automatic mode resolution based on either flag.

`--sorted false` suppresses only saved `view.sort`, including delayed application when columns appear. It does not remove a saved source sort or rewrite SQL/ES|QL. This preserves the meaning of a source query and avoids disabling useful formatting through `--no-view`.

### Select rows before profiling or formatting

Pass a complete-or-prefix preparation policy through source opening and the shared driver. Prefix selection applies source operations and the configured source limit, then view filters, then enabled view sorting, then the output limit. A local exact sort can require the full active result; use `--sorted false` to avoid that cost. Do not sort only the first N input rows and describe them as the top N of the sorted result.

File readers must use incremental preparation in preview mode regardless of file size. Stop after N matching rows and enough lookahead to detect another match. Read-ahead buffers may be bounded; they must not grow with the unconsumed file. Selective filters can still require a long scan to find N matches or prove EOF. Stdin closes after preview preparation instead of waiting for a finite producer's EOF when enough rows are available.

Native adapters may retrieve their bounded response eagerly. Reduce fetching only where the prefix remains equivalent after all filters and sorts. Do not push N into a query before local filters and silently return too few rows. Source limits still define the active result, and the summary does not count rows outside that result.

### Freeze preview layout from the selected rows

Automatic widths and auto-range colors use the emitted rows, with existing explicit widths and fixed color rules honored. Plain output skips color profiling. Default schema discovery uses the selected prefix and required bounded format detection rather than the normal 100 MiB scan. Lookahead must not widen columns or add fields to the displayed prefix. Resolve pending saved column settings for the preview schema before writing.

An explicit or saved `schema_scan: full` still requests complete schema discovery. Document `--schema-scan default` as the override for preview users. Whole-object parsing or validation must not force scanning the remainder of a selected JSON array or keyed object solely to complete a preview. Finding a nested pointer can still require reading earlier content.

### Report remaining rows without a counting pass

Use an exact count only for the effective filtered result, never an unfiltered source count. Otherwise, look for one extra matching row and emit `more rows...`. Emit no summary at EOF with N or fewer matches. This deliberately relaxes the requested numeric summary when a number would defeat fast output. Do not estimate counts from file size or physical newlines.

The summary is a plain, unstyled stdout line after the table. Headers and the summary do not consume the data-row allowance. Each logical record still occupies one physical table line, including quoted multiline CSV and cells with escaped newlines.

### Preserve error handling

Prepare the prefix and required lookahead before writing. Errors encountered there leave stdout empty. Unread trailing content is not validated, so preview success does not certify the full file. Keep broken-pipe success, other write failures, stderr diagnostics, and terminal-free batch behavior.

## Risks and trade-offs

- Saved sorting, restrictive filters, explicit full-schema scans, and native server queries can remain expensive. Document these limits with the preview command.
- Preview widths, inferred types, late fields, and automatic gradients can differ from a full export. Freeze them before the first output byte and test that later rows do not influence them.
- Early stdin termination can close the producer's pipe. Document it as normal preview behavior.
- An unknown count gives less information. Prefer honest `more rows...` to a full counting pass or a misleading estimate.

## Migration plan

Add the options without changing existing invocations. Update the output guide and CLI help when implemented. Keep this change active until implementation, regression checks, and review are complete; do not synchronize or archive the draft proposal now.
