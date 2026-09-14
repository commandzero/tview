# Preview measurements

Measured 2026-09-13T19:20:45.051541+00:00 on macOS-26.6.2-arm64-arm-64bit-Mach-O.

Release build from the fast-table-preview implementation. Each run starts a new process; OS file caches were not cleared. Preview times are medians of three runs, full-output times are single runs capped at 15 seconds. Both write to /dev/null with an empty config directory. Files contain an integer and a 480-character payload per row.

| Format | Bytes | Rows | Preview, 30 rows | Full output |
| --- | ---: | ---: | ---: | ---: |
| csv | 8370884 | 17331 | 0.008 s | 0.079 s |
| csv | 133940258 | 277309 | 0.007 s | 1.064 s |
| ndjson | 8388419 | 16710 | 0.008 s | 0.095 s |
| ndjson | 134217229 | 267365 | 0.008 s | >15 s, stopped |
| json | 8388421 | 16710 | 0.008 s | 0.096 s |
| json | 134217231 | 267365 | 0.009 s | >15 s, stopped |

Commands use `--output table --sorted false`, adding `-n 30` for previews. Instrumented reader tests supply a prefix followed by a 200 MiB unread suffix. CSV, JSON, and NDJSON consume at most 8 KiB for two data rows and one lookahead row, with lazy thresholds set both below and above the input size. These read-count checks establish bounded work independently of elapsed-time noise.
