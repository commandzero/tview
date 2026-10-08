---
type: Guide
title: Elasticsearch sources
description: Elastic CLI contexts, ES|QL queries, index selection, and authentication.
generated: { by: openai-codex/gpt-6.1-sol, at: 2026-10-07T03:56:21Z }
---

# Elasticsearch sources

Elasticsearch support is optional and is not included in default, minimal, or
Homebrew builds. Install it with `cargo install tview --features elasticsearch`.
Provide an HTTP or HTTPS cluster endpoint with explicit or saved Elasticsearch
format:

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

## Elastic CLI contexts

Reuse a configured Elastic CLI Elasticsearch service without copying its
endpoint or credentials:

```sh
tview .es://
tview .elasticsearch:// --table 'logs-*'
tview '.production.es://logs-*'
tview '.production.elasticsearch://logs-*'
tview .production.us-west.es:// --query 'FROM logs-* | KEEP message'
```

`.es://` and `.elasticsearch://` select the current context. Named forms select
the exact context name before the rightmost `es` or `elasticsearch` segment,
so `.production.us-west.es://` selects `production.us-west`. These sources
imply Elasticsearch format and override a saved format. An explicit
non-Elasticsearch `--format` conflicts with them.

A non-empty suffix after `://` is a literal index or pattern, with the same
meaning as `--table`. It is not URL-decoded or appended to the endpoint.
It overrides a saved table or query and cannot combine with explicit `--table`
or `--query`. These conflicts are rejected before credential resolvers run.
An empty suffix keeps normal CLI-over-saved selection and the interactive
picker. Batch output still needs a table or query.

### Configuration and trust

Tview uses `ELASTIC_CLI_CONFIG_FILE` if set. Otherwise it uses the first readable
file in your home directory, in this order: `.elasticrc`, `.elasticrc.json`,
`.elasticrc.yaml`, `.elasticrc.yml`. Configuration is read-only. Tview does not
load `.env` or espipe known-host files, and direct HTTP(S) sources do not load
Elastic CLI configuration.

Use configuration you trust. Only the selected Elasticsearch service is
resolved, but its configured command or `pass` resolver can execute a program.
Resolvers in unselected contexts and services remain inert. Commands are not
interpreted as shell expressions. Missing configuration, contexts, services,
or failed resolution produce an error without falling back to another context
or a local file.

### Connection lifetime and saved views

Tview resolves the concrete context, endpoint, and authentication once per
process. Discovery, table selection, query replacement, and reload reuse that
connection without rereading configuration or rerunning resolvers. Restart
Tview to pick up changed configuration or credentials, including expired
credentials.

Saved-view matching and YAML normalize `es` to `elasticsearch`, retaining the
exact context name and suffix, for example
`.production.elasticsearch://logs-*`. Generated filenames are filesystem-safe.
Resolved endpoints, credentials, and resolver expressions are not saved.
A saved `.elasticsearch://` reference follows the then-current context in the
next process; reload in the original process remains on its pinned connection.

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

For direct HTTP(S) endpoints, set credentials through environment variables:

```sh
ELASTIC_API_KEY=... tview --format elasticsearch https://elastic.example:9200 --table 'logs-*'
ELASTIC_USERNAME=elastic ELASTIC_PASSWORD=... \
  tview --format elasticsearch https://elastic.example:9200 --table 'logs-*'
```

API-key and username/password modes cannot be combined, and basic
authentication requires both username and password.

For context sources, authentication belongs to the selected service: API key,
basic, or none. Tview ignores `ELASTIC_API_KEY`, `ELASTIC_USERNAME`, and
`ELASTIC_PASSWORD` as overrides, even for an unauthenticated context. A context's
explicit environment resolver can still read the variables it names.

Both source forms use native TLS and retain certificate validation and request
timeouts. Set `ELASTIC_CA_CERT` to a custom CA certificate file when needed:

```sh
ELASTIC_CA_CERT=/path/to/ca.pem tview '.production.es://logs-*'
```

Non-HTTP(S) endpoints and credentials in endpoint URLs are rejected. URL query
strings, fragments, and userinfo never appear in diagnostics or saved-view
identities. The account needs permission to resolve index metadata and read
mappings and field capabilities for picker and table mode, and to run ES|QL
and read the target data.

Partial ES|QL results are labeled in the viewer and warned on stderr; stdout
contains only table data. Context diagnostics retain the failed operation and
safe source identity, and warnings retain counts, but omit raw resolver or
server error and warning text. Runtime secrets are excluded from diagnostics,
saved YAML, and query provenance.

## Client and limitations

Server-side async-query progress is unsupported. Tview has no connection-profile
UI or store; external Elastic CLI contexts are supported as described above.
Index aliases can be selected with `--table` but do not appear in the picker.
