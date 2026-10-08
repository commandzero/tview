## Why

Tview cannot reuse Elasticsearch connections stored in Elastic CLI contexts. Users must repeat endpoints and arrange credentials separately, even when `.elasticrc` already defines both.

## What changes

- Add `elasticrc` as an optional dependency of the existing `elasticsearch` Cargo feature.
- Accept `.es://` and `.elasticsearch://` for the current context, and `.name.es://` and `.name.elasticsearch://` for a named context. These sources imply Elasticsearch format without `--format elasticsearch`.
- Accept an optional index or pattern suffix, such as `.production.es://logs-*`, as an explicit table selection. Empty suffixes retain the existing picker, CLI query, and saved-view selection behavior.
- Resolve only the selected context's Elasticsearch service. Use its authentication, including resolver-backed credentials, without merging Tview's credential environment variables. Direct URL authentication remains unchanged.
- Keep resolved secrets runtime-only, preserve context identity in saved views, and keep reloads on the connection selected for the invocation.
- Retain existing ES|QL limits, TLS checks, asynchronous query behavior, and output guarantees. No connection-management UI or credential-writing commands are added.

## Capabilities

### New capabilities

- `elasticrc-contexts`: Dot-context syntax, context discovery, lazy service resolution, authentication selection, errors, and invocation connection stability.

### Modified capabilities

- `elasticsearch-data-source`: Add context endpoint resolution and context authentication alongside direct URL behavior; gate the dependency with Elasticsearch support.
- `data-ingestion`: Recognize dot-context targets before URL or file parsing and dispatch them without filesystem probing.
- `saved-views`: Match and serialize non-secret context identities without copying resolved connection secrets.

## Impact

Source-target parsing and format resolution, saved-view matching and source merging, Elasticsearch client construction, and reload ownership need integration. Cargo.toml and Cargo.lock gain the optional dependency and its platform-specific resolver dependencies. Documentation and help need examples and precedence rules; behavior tests must cover parsing, selection, secrets, and feature-disabled builds.

This change is additive. Ordinary paths, stdin, explicit HTTP(S) endpoints, and their existing authentication keep their behavior. The uncommitted Homebrew documentation changes are separate from this proposal.
