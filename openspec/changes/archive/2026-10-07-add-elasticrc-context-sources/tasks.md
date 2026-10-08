## 1. Dependency and target syntax

- [x] 1.1 Add published `elasticrc` under the existing `elasticsearch` Cargo feature and update the lockfile. Verify default and minimal dependency graphs omit it, and Elasticsearch-only and all-feature graphs include it without raising Rust 1.90 compatibility.
- [x] 1.2 Add side-effect-free dot-context target parsing before generic URL and path handling. Verify both service aliases, current and named contexts, dotted context names, literal suffixes, unsupported services, malformed reserved syntax, ordinary dot-prefixed files, stdin, file URIs, and Windows drive paths through consumer-visible parsing behavior.
- [x] 1.3 Integrate implied Elasticsearch format and suffix selection into existing CLI-over-saved source merging. Verify context format overrides saved format, explicit incompatible formats and suffix-plus-CLI selectors fail before resolver execution, suffixes override saved queries or tables, and empty suffixes retain CLI and saved selections.

## 2. Selected-service resolution and transport

- [x] 2.1 Add one private selected-service resolution entry point using `elasticrc` configuration discovery and service resolution. Verify `ELASTIC_CLI_CONFIG_FILE`, home discovery order, current and named selection, missing config/context/service failures, and read-only config handling with isolated fixtures.
- [x] 2.2 Build context transport from API-key, basic, or unauthenticated service authentication without calling environment-auth override logic. Verify captured HTTP requests use context credentials even with conflicting Tview credential variables, while explicit context env resolvers work and direct URL environment behavior remains unchanged.
- [x] 2.3 Retain custom CA handling, certificate validation, request timeouts, and HTTP(S)/userinfo checks for resolved endpoints. Verify a local TLS fixture trusts the configured CA, rejects untrusted certificates, and rejects unsupported or credential-bearing endpoint URLs before HTTP requests.
- [x] 2.4 Keep configuration and selected resolver work off the terminal event loop and resolve no other service. Verify an unselected command resolver leaves its marker absent, a selected bounded resolver executes only when opening the source, and a delayed selected resolver does not block TUI input.
- [x] 2.5 Redact runtime context credentials and sanitize resolution and transport diagnostics. Verify secret sentinels do not appear in debug output, stdout, stderr, query provenance, or generated YAML, including resolver failures containing captured output. Confirm batch preparation failures return 1 with empty stdout.

## 3. Invocation connection and saved views

- [x] 3.1 Retain a resolved context connection snapshot with the existing source lifecycle and reuse it for discovery, mapping, query replacement, and reload. Verify changing the config's current context or credential resolver after initial open does not change endpoint/authentication or rerun the resolver; a new invocation observes the change.
- [x] 3.2 Canonicalize `es` to `elasticsearch` for safe source identity and generated view filenames without resolving the endpoint. Verify both aliases select the same saved view, named contexts remain distinct, current-context identity remains current-context selection, and view discovery has no resolver side effects.
- [x] 3.3 Serialize committed context source and local view settings using existing YAML fields, excluding resolved endpoints, credentials, and resolver expressions. Verify pending or failed query changes retain the previous committed source and latest activation failure still blocks final interactive export.

## 4. End-to-end behavior and feature compatibility

- [x] 4.1 Exercise actual CLI invocations for current/named aliases, a pattern suffix, explicit tables and ES|QL, and saved table/query selections against the existing Elasticsearch fixture. Verify returned rows, existing 1,000-row limits, partial-result diagnostics, mappings, and local-filter no-refill behavior without substituting a mock for the live source smoke run.
- [x] 4.2 Exercise interactive context discovery, source replacement, reload, save, and final export through the existing terminal integration setup. Verify responsiveness, connection pinning, compatible view-state retention, and failure behavior with observed terminal/output evidence.
- [x] 4.3 Verify default and no-default-feature binaries reject dot-context sources as unavailable without loading config or running resolvers. Run default, minimal, Elasticsearch-only, all-feature, pinned-MSRV, and supported native macOS/Linux checks, and retain deterministic regression tests for selection, precedence, resolver isolation, secrets, and lifecycle transitions.

## 5. Documentation and acceptance

- [x] 5.1 After runtime proof, update Elasticsearch, file-input, saved-view, and installation/help guidance with both aliases, dotted context names, literal suffix semantics, selector conflicts, context-owned authentication, config trust, CA handling, and restart-to-refresh behavior. Verify examples against the built CLI and run `bash scripts/preflight.sh docs`.
- [x] 5.2 Add a concise user-facing changelog entry under repository policy and attach a verified PR reference when available. Verify it describes context reuse without claiming default or Homebrew Elasticsearch support.
- [x] 5.3 Run required repository preflight and `OPENSPEC_TELEMETRY=0 openspec validate add-elasticrc-context-sources --strict`, and verify implementation against every delta scenario. Keep the change active for PR review; synchronize and archive only after post-review implementation verification, then confirm final committed-head required checks before any separately authorized merge.
