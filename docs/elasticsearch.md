---
type: Guide
title: Elasticsearch sources
description: ES|QL queries, index selection, authentication, and source limits.
generated: { by: openai-codex/gpt-6.1-sol, at: 2026-10-07T03:56:21Z }
---

# Elasticsearch sources

Enable the optional `elasticsearch` feature and provide an HTTP or HTTPS
cluster endpoint:

```sh
tview --format elasticsearch https://elastic.example:9200
tview --format elasticsearch https://elastic.example:9200 --table 'logs-*'
tview --format elasticsearch https://elastic.example:9200 \
  --query 'FROM logs-* | KEEP @timestamp, message | SORT @timestamp DESC'
```

Select data with `--table` or `--query`, or a saved view's `source.table` or
`source.query`. Without a selection, interactive mode offers a picker for
visible, open non-dot indices and data streams. It omits aliases; use `--table`
to select one. Batch output requires a configured table or query.

## Queries and limits

`--table` generates an ES|QL `FROM` query with `_index` and `_id` metadata.
`--query` supplies a complete ES|QL base query; Tview does not validate its
`FROM` targets. Source Configuration sets filters, sorts, and a hard limit of
1,000 rows by default. It quotes filters safely and requests one extra row to
detect truncation. The Query popup shows a reusable query with the configured
limit.

For a selected index or data stream, Tview reads mappings and field
capabilities to offer nested fields, multifields, runtime fields, and
cross-index conflicts. Result columns come from the ES|QL response, since
`STATS`, `EVAL`, and `KEEP` can change them. Starting with `--query` skips
mapping discovery.

Failed source changes leave the previous rows and successful query available
for saving and reload. Reload reopens that selection and keeps compatible
local column settings. See [saved views](saved-views.md#source-settings) for
source and view settings.

## Authentication

Set credentials and custom CA certificates through environment variables:

```sh
ELASTIC_API_KEY=... tview --format elasticsearch https://elastic.example:9200 --table 'logs-*'
ELASTIC_USERNAME=elastic ELASTIC_PASSWORD=... \
  tview --format elasticsearch https://elastic.example:9200 --table 'logs-*'
ELASTIC_CA_CERT=/path/to/ca.pem \
  tview --format elasticsearch https://elastic.example:9200 --table 'logs-*'
```

API-key and username/password modes cannot be combined. Credentials in the
endpoint URL are rejected. URL query strings, fragments, and userinfo never
appear in diagnostics or saved-view identities. The account needs permission
to resolve index metadata and read mappings and field capabilities for picker
and table mode, and to run ES|QL and read the target data.

Partial ES|QL results are labeled in the viewer and warned on stderr; stdout
contains only table data.

## Client and limitations

Tview uses native TLS. Server-side async-query progress and connection
profiles are unsupported; aliases can be selected with `--table` but do not
appear in the picker.
