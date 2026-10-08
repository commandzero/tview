## Context

See [proposal.md](proposal.md) for motivation and [the context requirements](specs/elasticrc-contexts/spec.md) for behavior.

`SourceTarget` currently parses paths, stdin, and URLs. Its infallible URL-first parser sends a leading-dot scheme to the filesystem because standard URL schemes cannot start with a dot. The Elasticsearch adapter accepts HTTP(S) URLs and constructs transport settings from `ELASTIC_API_KEY`, `ELASTIC_USERNAME`, `ELASTIC_PASSWORD`, and `ELASTIC_CA_CERT`. Saved-view discovery uses a safe source identity before opening a source. Source replacement and reload already have committed-result ownership.

The published `elasticrc` 0.1.0 crate has MSRV 1.89, below Tview's 1.90 minimum. It loads configuration without running resolvers and resolves one typed service on demand. Runtime authentication uses redacted secrets. Its platform credential-store and command-resolver dependencies need feature and native-target verification during implementation.

## Goals / Non-goals

### Goals

- Keep syntax parsing and saved-view discovery free of credential-resolution side effects.
- Reuse the existing Elasticsearch adapter, client transport, and committed-result lifecycle.
- Keep dependency-owned discovery, context selection, and resolver behavior in `elasticrc` rather than duplicate them.
- Preserve direct URL behavior and make context connection identity stable during an invocation.

### Non-goals

- No Kibana or Cloud source adapters, context editor, config writer, or new credential CLI flags.
- No `env:` targets, dotenv loading, espipe known-host files, or automatic context use when no source is supplied.
- No new retry policy, credential refresh loop, insecure TLS flag, or query behavior.
- No support for espipe's single-slash `:/index` spelling in this change. Tview uses the requested `://` spelling and does not construct an ingest URL from the suffix.

## Decisions

### Parse a context target before generic URLs

Reserve a leading-dot reference followed by `://`. Store its unresolved service reference and optional table suffix as source data, not as a fabricated URL. The rightmost service segment accepts `es` and `elasticsearch`; a named context retains all preceding segments exactly. The suffix is the literal `--table` value, with no URL decoding or endpoint-path concatenation. Reuse existing table-target validation.

Examples:

```sh
tview .es://
tview .elasticsearch:// --table 'logs-*'
tview .production.es://logs-*
tview .production.us-west.elasticsearch:// --query 'FROM logs-* | KEEP message'
```

The parser must distinguish `.es` and `./.production.es` local paths from context sources. Malformed reserved context syntax is an error, not a filesystem fallback. Help and syntax validation must not execute configuration resolvers.

A fabricated URL would lose context identity or misinterpret the suffix as authority. Sending the reference through file probing would produce misleading missing-file errors. A dedicated unresolved target keeps those cases explicit. Builds without Elasticsearch must recognize the reserved syntax enough to report unsupported feature availability, without linking `elasticrc`.

### Treat the marker as an explicit source-format choice

A dot-context marker supplies Elasticsearch format with positional-source precedence over saved format settings. An explicit non-Elasticsearch CLI format is a conflict. A non-empty suffix supplies an explicit table selection and replaces saved table or query selection. Reject suffix plus explicit `--table` or `--query` before resolving credentials, even if the values appear compatible. Do not silently combine competing selectors.

An empty suffix leaves ordinary CLI-over-saved selection intact. Thus the marker alone supports interactive discovery, `--table`, `--query`, and saved selections. Ordinary HTTP(S) URLs still need explicit or saved Elasticsearch format; this change does not imply a general URL format detector.

### Resolve one selected service through elasticrc

Use `ConfigFile::load_with_options(None, None)` so `ELASTIC_CLI_CONFIG_FILE` and home discovery follow the dependency's contract. Parse the service reference through `ContextServiceReference`, select its named context or `current_context_name`, and resolve only its Elasticsearch `ServiceConfig`. Do not call broad validation or resolution routines that execute unselected resolvers.

Perform invalid-format and selector checks before loading the config. Run blocking file, keyring, and subprocess resolution away from the terminal event loop, using the existing source-opening work model. Check resolved endpoint scheme and userinfo with the existing HTTP(S) transport rules before network requests. Surface missing config, context, service, and resolution failures without falling back to a different source.

The integration needs one private connection-resolution entry point returning a client-ready connection and non-secret identity. Callers should not orchestrate configuration discovery, context selection, and secret conversion separately. Do not introduce a new general connection-provider trait for this single integration.

### Context authentication is authoritative

The user selected context authentication, not environment overrides. For context sources, convert `elasticrc::Auth::{ApiKey, Basic, None}` into transport authentication only at client construction. Keep secrets redacted until the official client requires their values, then avoid unnecessary copies and do not put them in public source options or serializable configuration.

Do not call the existing environment-authentication reader for context sources. Even invalid or mixed `ELASTIC_API_KEY`, `ELASTIC_USERNAME`, and `ELASTIC_PASSWORD` transport variables must not change context behavior. A selected context's explicit `env` resolver still reads the variables it names. Read `ELASTIC_CA_CERT` independently and retain current certificate validation and timeouts.

Direct URL sources keep the current environment rules, including API-key/basic conflicts and paired username/password validation. Tview does not adopt espipe's credential flags because it has no corresponding CLI contract.

### Pin the connection for the invocation

Resolve the concrete current or named context once before initial Elasticsearch opening. Keep the resulting client and safe identity with the source connection already owned by the active source result. Discovery, selected-table opening, query replacement, and reload reuse it. They must not reread `.elasticrc` or rerun a credential resolver.

This avoids a reload silently switching production to staging after an external current-context change. It also prevents repeated keychain prompts or command execution. A new invocation picks up changed configuration or credentials. Expired credentials can therefore require restarting Tview; automatic refresh is deliberately outside this proposal.

### Keep saved-view identity unresolved and non-secret

Normalize the `es` alias to `elasticsearch` for safe identity and generated view filenames. Preserve the original current-context or exact named-context selector and suffix. Do not resolve secrets or the endpoint to discover a saved view.

A saved current-context identity remains `.elasticsearch://`, so a future invocation intentionally follows the then-current context. That differs from reload inside the current invocation, which stays pinned. Named contexts remain named. Source table/query, filters, sorts, and limits continue using the existing YAML fields and committed configuration. An explicit positional suffix keeps its normal precedence when the view is used again.

No new saved credential field is needed. Do not serialize the resolved endpoint, inline secrets, resolver expressions, transport headers, or the client. Preserve existing once-per-invocation saved-view binding and latest-activation export behavior.

### Gate the dependency with Elasticsearch

Add the published `elasticrc` dependency only to the existing `elasticsearch` feature. Do not enable Elasticsearch in default releases or change Homebrew archive features as part of this change. Verify the lockfile and dependency graph with default, minimal, Elasticsearch-only, and all-feature builds. Platform keyring support must follow the dependency's platform implementation without new unsafe-code exceptions in Tview.

## Risks / trade-offs

- Trusted configuration can execute commands. Resolve only the selected service, document that trust requirement, and retain the dependency's execution and output limits. Do not add a shell interpreter.
- Resolver errors can contain credentials or captured program output. Render bounded, safe context and stage diagnostics instead of blindly forwarding dependency error chains. Verify with secret sentinels in success and failure paths.
- Dot-context syntax is not a standard URL. Keep it out of URL parsing and make unsupported services and malformed references explicit failures.
- Alias canonicalization affects view names. Both aliases must select the same canonical view; context names must stay exact and distinct. Test this through actual selection and generated YAML.
- New resolver dependencies may affect build compatibility. Verify pinned MSRV and native macOS/Linux targets before changing support claims.
- A pinned connection does not refresh changed or expired credentials. Users restart Tview to resolve them again; this keeps reload from changing clusters.

## Migration plan

This is additive for Elasticsearch-enabled builds. Existing direct endpoint, stdin, file, and saved-query users need no migration. Keep the change active during implementation and review. After runtime smoke evidence, document the aliases, suffix conflicts, context-authentication precedence, resolver trust, and restart behavior, then add the user-visible changelog entry. Verify the reviewed implementation against the deltas before synchronization and archival.

## References

1. [Published elasticrc crate](https://crates.io/crates/elasticrc) and [0.1.0 API](https://docs.rs/elasticrc/0.1.0/elasticrc/).
2. [elasticrc discovery and resolution documentation](https://github.com/elastic/esdiag/tree/main/crates/elasticrc).
3. [Espipe context authentication](https://github.com/VimCommando/espipe/blob/main/docs/authentication.md).
4. [Espipe target parsing and service resolution](https://github.com/VimCommando/espipe/blob/main/src/output/mod.rs). It uses `:/index` and rejects a `//` path. The precedent is the dot-context service selection, not identical target-path parsing.
