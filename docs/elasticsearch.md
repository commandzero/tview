---
type: Guide
title: Elasticsearch sources
description: ES|QL queries, index selection, authentication, and source limits.
generated: { by: codex/gpt-6, at: 2026-09-12T17:14:16Z }
---

# Elasticsearch sources

Build with `--features elasticsearch`, then provide an HTTP or HTTPS cluster
endpoint:

```sh
tview --format elasticsearch https://elastic.example:9200
tview --format elasticsearch https://elastic.example:9200 --table 'logs-*'
tview --format elasticsearch https://elastic.example:9200 \
  --query 'FROM logs-* | KEEP @timestamp, message | SORT @timestamp DESC'
```

Without `--table` or `--query`, interactive mode discovers visible, open non-dot
indices and data streams and displays them in separate picker sections. The
picker omits aliases. Supply an alias through `--table` to use it. Direct
non-interactive output requires `--table` or `--query`, because it cannot ask
the user to choose a target.

## Queries and limits

`--table` generates an ES|QL `FROM` query with `_index` and `_id` metadata.
`--query` supplies the complete ES|QL base query; Tview does not parse or
validate its `FROM` targets. Source Configuration can add safely quoted filters
and sorts and change the hard limit, which defaults to 1,000 rows. Tview
privately requests one extra row to distinguish a complete result from a limited
one; the reusable query shown by the Query popup retains the configured limit.

For a selected index or data stream, Tview reads mappings and field capabilities
to build a catalog including nested fields, multifields, runtime fields, and
cross-index conflicts. The displayed columns come from the ES|QL response, since
`STATS`, `EVAL`, and `KEEP` can change them. Query-only startup skips mapping
discovery. Successful queries replace the rows and column definitions together;
failed or superseded requests leave the prior result visible.

## Authentication

Set credentials and custom CA certificates through environment variables:

```sh
ELASTIC_API_KEY=... tview --format elasticsearch https://elastic.example:9200 --table 'logs-*'
ELASTIC_USERNAME=elastic ELASTIC_PASSWORD=... \
  tview --format elasticsearch https://elastic.example:9200 --table 'logs-*'
ELASTIC_CA_CERT=/path/to/ca.pem \
  tview --format elasticsearch https://elastic.example:9200 --table 'logs-*'
```

API-key and username/password modes are mutually exclusive. Credentials in the
endpoint URL are rejected, and URL query strings, fragments, and userinfo are
never included in diagnostics or saved-view identities. The account needs
permission to resolve index metadata and read mappings/field capabilities for
picker/table mode, plus permission to run ES|QL and read the target data.
Partial ES|QL results are labeled in the TUI and warned on stderr; stdout
remains table data only.

## Client and limitations

The optional adapter pins the official Rust client at `9.1.0-alpha.1`, uses its
`native-tls` backend, and adds the client, HTTP, TLS, and URL dependency graph
only when the feature is enabled. The [integration
fixture](../tests/fixtures/elasticsearch/README.md) tests the adapter against
Elasticsearch 9.1.0. Tview does not support server-side async-query progress,
connection profiles, or aliases in the picker.
