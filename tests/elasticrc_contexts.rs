#[path = "support/elasticrc.rs"]
pub mod elasticrc;
#[cfg(feature = "elasticsearch")]
#[path = "support/elasticsearch_mock.rs"]
pub mod elasticsearch_mock;

use crate::elasticrc::{preparation_failure, Fixture};
use serde_json::json;

#[cfg(not(feature = "elasticsearch"))]
#[test]
fn unavailable_context_sources_never_probe_files_or_run_resolvers() {
    let fixture = Fixture::new();
    #[cfg(unix)]
    {
        let marker = fixture.root.path().join("resolver-ran");
        let resolver = fixture.resolver(
            "resolver",
            &format!(
                "touch {}\nprintf secret",
                crate::elasticrc::shell_quote(&marker)
            ),
        );
        fixture.write("production", json!({ "production": crate::elasticrc::service("http://127.0.0.1:1", Some(json!({"api_key": resolver}))) }));
        for source in [".es://", ".production.elasticsearch://logs-*"] {
            let output = fixture.run(&[source]);
            preparation_failure(&output);
            assert!(!marker.exists(), "disabled feature executed a resolver");
            let diagnostic = String::from_utf8_lossy(&output.stderr).to_lowercase();
            assert!(diagnostic.contains("elasticsearch"));
            assert!(
                !diagnostic.contains("no such file"),
                "context was treated as a path"
            );
        }
    }
    std::fs::write(&fixture.config, "not valid configuration").unwrap();
    preparation_failure(&fixture.run(&[".es://logs-*"]));
}

#[cfg(feature = "elasticsearch")]
mod enabled {
    use super::*;
    use crate::elasticrc::{
        authorization, no_secrets, request_query, service, success, FIELD_CAPS, MAPPING, QUERY,
        ROWS,
    };
    use crate::elasticsearch_mock::{Response, Server};

    fn query_server() -> Server {
        Server::start(vec![Response::ok(ROWS)])
    }
    fn table_server() -> Server {
        Server::start(vec![
            Response::ok(MAPPING),
            Response::ok(FIELD_CAPS),
            Response::ok(ROWS),
        ])
    }

    #[test]
    fn current_named_and_dotted_context_aliases_select_the_exact_service() {
        for source in [
            ".es://",
            ".elasticsearch://",
            ".production.es://",
            ".production.elasticsearch://",
            ".production.us-west.es://",
            ".production.us-west.elasticsearch://",
        ] {
            let fixture = Fixture::new();
            let selected = query_server();
            let other = Server::start(vec![]);
            let name = if source.contains("us-west") {
                "production.us-west"
            } else {
                "production"
            };
            fixture.write(name, json!({ (name): service(selected.endpoint(), None), "production.us": service(other.endpoint(), None), "fallback": service(other.endpoint(), None) }));
            let original = std::fs::read(&fixture.config).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&fixture.config, std::fs::Permissions::from_mode(0o444))
                    .unwrap();
            }
            let output = fixture.run(&[source, "--query", QUERY]);
            success(&output);
            assert_eq!(output.stdout, b"message\ncontext-row\n");
            assert_eq!(selected.requests().len(), 1);
            assert!(other.requests().is_empty());
            assert_eq!(
                std::fs::read(&fixture.config).unwrap(),
                original,
                "configuration was modified"
            );
        }
    }

    #[test]
    fn explicit_config_overrides_home_and_home_discovery_follows_all_four_names() {
        let names = [
            ".elasticrc",
            ".elasticrc.json",
            ".elasticrc.yaml",
            ".elasticrc.yml",
        ];
        for first in 0..names.len() {
            let fixture = Fixture::new();
            let selected = query_server();
            let ignored = Server::start(vec![]);
            for name in &names[first..] {
                Fixture::write_at(
                    &fixture.home.join(name),
                    "home",
                    json!({"home": service(if *name == names[first] { selected.endpoint() } else { ignored.endpoint() }, None)}),
                );
            }
            let output = fixture
                .command()
                .env_remove("ELASTIC_CLI_CONFIG_FILE")
                .args([".es://", "--query", QUERY])
                .output()
                .unwrap();
            success(&output);
            assert_eq!(selected.requests().len(), 1);
            assert!(ignored.requests().is_empty());
        }
        let fixture = Fixture::new();
        let selected = query_server();
        let ignored = Server::start(vec![]);
        fixture.write(
            "override",
            json!({"override": service(selected.endpoint(), None)}),
        );
        Fixture::write_at(
            &fixture.home.join(".elasticrc"),
            "home",
            json!({"home": service(ignored.endpoint(), None)}),
        );
        success(&fixture.run(&[".es://", "--query", QUERY]));
        assert!(ignored.requests().is_empty());
    }

    #[test]
    fn context_authentication_ignores_even_invalid_transport_environment_modes() {
        for (auth, expected) in [
            (
                Some(json!({"api_key": "context-key-secret"})),
                Some("ApiKey context-key-secret"),
            ),
            (
                Some(json!({"username": "context-user", "password": "context-password-secret"})),
                Some("Basic Y29udGV4dC11c2VyOmNvbnRleHQtcGFzc3dvcmQtc2VjcmV0"),
            ),
            (None, None),
        ] {
            let fixture = Fixture::new();
            let server = table_server();
            fixture.write(
                "production",
                json!({"production": service(server.endpoint(), auth)}),
            );
            let output = fixture
                .command()
                .env("ELASTIC_API_KEY", "unrelated-api-secret")
                .env("ELASTIC_USERNAME", "unpaired-env-user")
                .args([".production.es://logs-a"])
                .output()
                .unwrap();
            success(&output);
            assert_eq!(server.requests().len(), 3);
            for request in server.requests() {
                assert_eq!(authorization(&request), expected);
            }
            no_secrets(
                &output,
                &[
                    "context-key-secret",
                    "context-password-secret",
                    "unrelated-api-secret",
                ],
            );
        }
    }

    #[test]
    fn explicit_context_environment_resolvers_remain_available() {
        let fixture = Fixture::new();
        let server = query_server();
        fixture.write("production", json!({"production": service(server.endpoint(), Some(json!({"api_key": "$(env:TVIEW_CONTEXT_SECRET)"})))}));
        let output = fixture
            .command()
            .env("TVIEW_CONTEXT_SECRET", "resolved-env-secret")
            .env("ELASTIC_PASSWORD", "unpaired-env-password")
            .args([".es://", "--query", QUERY])
            .output()
            .unwrap();
        success(&output);
        assert_eq!(
            authorization(&server.requests()[0]),
            Some("ApiKey resolved-env-secret")
        );
        no_secrets(&output, &["resolved-env-secret", "unpaired-env-password"]);
    }

    #[test]
    fn direct_url_auth_remains_environment_owned_and_never_loads_elasticrc() {
        for (variables, expected) in [
            (
                vec![("ELASTIC_API_KEY", "direct-env-key")],
                Some("ApiKey direct-env-key"),
            ),
            (
                vec![("ELASTIC_USERNAME", "user"), ("ELASTIC_PASSWORD", "pass")],
                Some("Basic dXNlcjpwYXNz"),
            ),
            (vec![], None),
        ] {
            let fixture = Fixture::new();
            std::fs::write(&fixture.config, "invalid configuration").unwrap();
            let server = query_server();
            let output = fixture
                .command()
                .envs(variables)
                .args([
                    server.endpoint(),
                    "--format",
                    "elasticsearch",
                    "--query",
                    QUERY,
                ])
                .output()
                .unwrap();
            success(&output);
            assert_eq!(authorization(&server.requests()[0]), expected);
        }
        for variables in [
            vec![("ELASTIC_USERNAME", "alone")],
            vec![("ELASTIC_PASSWORD", "alone")],
            vec![
                ("ELASTIC_API_KEY", "key"),
                ("ELASTIC_USERNAME", "user"),
                ("ELASTIC_PASSWORD", "pass"),
            ],
        ] {
            let fixture = Fixture::new();
            let server = Server::start(vec![]);
            let output = fixture
                .command()
                .envs(variables)
                .args([
                    server.endpoint(),
                    "--format",
                    "elasticsearch",
                    "--query",
                    QUERY,
                ])
                .output()
                .unwrap();
            preparation_failure(&output);
            assert!(server.requests().is_empty());
        }
    }

    #[cfg(unix)]
    #[test]
    fn syntax_and_selector_conflicts_fail_before_any_selected_resolver() {
        use crate::elasticrc::shell_quote;
        for args in [
            vec![".es://", "--format", "json", "--query", QUERY],
            vec![".es://logs-*", "--table", "logs-*"],
            vec![".es://logs-*", "--query", QUERY],
            vec![".production.kibana://", "--query", QUERY],
            vec![".production.cloud://", "--query", QUERY],
            vec![".production.unknown://", "--query", QUERY],
            vec![".://", "--query", QUERY],
            vec!["..es://", "--query", QUERY],
        ] {
            let fixture = Fixture::new();
            let marker = fixture.root.path().join("selected-ran");
            let resolver = fixture.resolver(
                "selected",
                &format!("touch {}\nprintf secret", shell_quote(&marker)),
            );
            fixture.write("production", json!({"production": service("http://127.0.0.1:1", Some(json!({"api_key": resolver})))}));
            let output = fixture.run(&args);
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            assert!(
                !marker.exists(),
                "validation ran a selected resolver: {args:?}"
            );
            assert!(!String::from_utf8_lossy(&output.stderr).contains("No such file"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn selected_command_runs_once_and_unselected_services_and_contexts_are_inert() {
        use crate::elasticrc::shell_quote;
        let fixture = Fixture::new();
        let server = table_server();
        let selected = fixture.root.path().join("selected-counter");
        let ignored = fixture.root.path().join("unselected-marker");
        let selected_resolver = fixture.resolver(
            "selected",
            &format!(
                "printf 'run\\n' >> {}\nprintf selected-key",
                shell_quote(&selected)
            ),
        );
        let ignored_resolver = fixture.resolver(
            "ignored",
            &format!("touch {}\nprintf unused-key", shell_quote(&ignored)),
        );
        let mut context = service(
            server.endpoint(),
            Some(json!({"api_key": selected_resolver})),
        );
        context["kibana"] =
            json!({"url": "http://127.0.0.1:1", "auth": {"api_key": ignored_resolver}});
        context["cloud"] =
            json!({"url": "http://127.0.0.1:1", "auth": {"api_key": ignored_resolver}});
        fixture.write("production", json!({"production": context, "staging": service("http://127.0.0.1:1", Some(json!({"api_key": ignored_resolver})))}));
        assert!(!selected.exists());
        success(&fixture.run(&[".es://logs-a"]));
        assert_eq!(std::fs::read_to_string(&selected).unwrap(), "run\n");
        assert!(!ignored.exists());
        for request in server.requests() {
            assert_eq!(authorization(&request), Some("ApiKey selected-key"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn selected_resolver_output_is_bounded_and_failure_output_is_redacted() {
        for body in [
            "printf secret-stdout-sentinel; printf secret-stderr-sentinel >&2; exit 7",
            "printf secret-stdout-sentinel; dd if=/dev/zero bs=1048576 count=2 2>/dev/null",
        ] {
            let fixture = Fixture::new();
            let server = Server::start(vec![]);
            let resolver = fixture.resolver("failing", body);
            fixture.write("production", json!({"production": service(server.endpoint(), Some(json!({"api_key": resolver})))}));
            let output = fixture.run(&[".es://", "--query", QUERY]);
            preparation_failure(&output);
            no_secrets(
                &output,
                &["secret-stdout-sentinel", "secret-stderr-sentinel", "$(cmd:"],
            );
            assert!(server.requests().is_empty());
        }
    }

    #[test]
    fn missing_config_context_service_and_invalid_endpoints_never_fallback() {
        let fixture = Fixture::new();
        preparation_failure(&fixture.run(&[".es://", "--query", QUERY]));
        preparation_failure(
            &fixture
                .command()
                .env_remove("ELASTIC_CLI_CONFIG_FILE")
                .args([".es://", "--query", QUERY])
                .output()
                .unwrap(),
        );
        std::fs::write(&fixture.config, "secret-config-sentinel {").unwrap();
        let malformed = fixture.run(&[".es://", "--query", QUERY]);
        preparation_failure(&malformed);
        no_secrets(&malformed, &["secret-config-sentinel"]);
        fixture.write(
            "missing-current",
            json!({"production": service("http://127.0.0.1:1", None)}),
        );
        preparation_failure(&fixture.run(&[".es://", "--query", QUERY]));
        preparation_failure(&fixture.run(&[".missing.es://", "--query", QUERY]));
        fixture.write(
            "production",
            json!({"production": {"kibana": {"url": "http://127.0.0.1:1"}}}),
        );
        preparation_failure(&fixture.run(&[".es://", "--query", QUERY]));
        for endpoint in [
            "file:///secret-endpoint-sentinel",
            "ftp://secret-endpoint-sentinel",
            "http://secret-user-sentinel:secret-password-sentinel@127.0.0.1:1",
            "not-a-url-secret-sentinel",
        ] {
            fixture.write("production", json!({"production": service(endpoint, None)}));
            let output = fixture.run(&[".es://", "--query", QUERY]);
            preparation_failure(&output);
            no_secrets(
                &output,
                &[endpoint, "secret-user-sentinel", "secret-password-sentinel"],
            );
        }
        let server = Server::start(vec![]);
        let credential_endpoint = server.endpoint().replacen(
            "http://",
            "http://secret-user-sentinel:secret-password-sentinel@",
            1,
        );
        fixture.write(
            "production",
            json!({"production": service(&credential_endpoint, None)}),
        );
        let output = fixture.run(&[".es://", "--query", QUERY]);
        preparation_failure(&output);
        no_secrets(
            &output,
            &["secret-user-sentinel", "secret-password-sentinel"],
        );
        assert!(
            server.requests().is_empty(),
            "userinfo rejected only after HTTP request"
        );
    }

    #[test]
    fn literal_suffix_is_a_table_pattern_not_an_endpoint_path_or_decoded_url() {
        for suffix in ["logs-*", "logs-a,logs-b"] {
            let fixture = Fixture::new();
            let server = table_server();
            fixture.write(
                "production",
                json!({"production": service(server.endpoint(), None)}),
            );
            success(&fixture.run(&[&format!(".es://{suffix}")]));
            let requests = server.requests();
            let query = request_query(requests.last().unwrap());
            assert!(
                query.contains(suffix),
                "suffix was lost or decoded: {query}"
            );
            let request_target = requests
                .last()
                .unwrap()
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap();
            let path = request_target
                .split_once('?')
                .map_or(request_target, |(path, _)| path);
            assert_eq!(path, "/_query", "suffix appended to endpoint");
        }
        for suffix in ["logs%2A", "logs-a/path"] {
            let fixture = Fixture::new();
            let server = Server::start(vec![]);
            fixture.write(
                "production",
                json!({"production": service(server.endpoint(), None)}),
            );
            preparation_failure(&fixture.run(&[&format!(".es://{suffix}")]));
            assert!(
                server.requests().is_empty(),
                "invalid literal suffix became a URL/path"
            );
        }
    }

    #[test]
    fn dot_prefixed_local_files_and_file_uris_are_not_context_sources() {
        let fixture = Fixture::new();
        std::fs::write(&fixture.config, "invalid").unwrap();
        for filename in [".es", ".production.es"] {
            let path = fixture.root.path().join(filename);
            std::fs::write(&path, "message\nfile-row\n").unwrap();
            for source in [
                filename.to_owned(),
                format!("./{filename}"),
                url::Url::from_file_path(&path).unwrap().to_string(),
            ] {
                let output = fixture.run(&[&source, "--format", "delimited"]);
                success(&output);
                assert_eq!(output.stdout, b"message\nfile-row\n");
            }
        }
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn suffix_overrides_saved_format_table_and_query_and_aliases_match_canonical_views() {
        for saved_selection in [
            "  query: 'FROM saved-query | KEEP message'\n",
            "  table: saved-table\n",
        ] {
            for source in [
                ".production.es://logs-*",
                ".production.elasticsearch://logs-*",
            ] {
                let fixture = Fixture::new();
                let server = table_server();
                fixture.write(
                    "production",
                    json!({"production": service(server.endpoint(), None)}),
                );
                fixture.view("canonical", &format!("name: canonical\nfilenames: ['.production.elasticsearch://logs-*']\nsource:\n  format: json\n{saved_selection}view:\n  columns:\n    message: {{label: SAVED_LABEL}}\n"));
                let output = fixture.run(&[source]);
                success(&output);
                assert_eq!(output.stdout, b"SAVED_LABEL\ncontext-row\n");
                let query = request_query(server.requests().last().unwrap());
                assert!(query.contains("logs-*"));
                assert!(!query.contains("saved-query"));
                assert!(!query.contains("saved-table"));
            }
        }
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn empty_suffix_preserves_saved_selection_and_cli_query_overrides_it() {
        for (selection, cli, expected) in [
            (
                "  query: 'FROM saved-query | KEEP message'\n",
                vec![],
                "FROM saved-query | KEEP message",
            ),
            (
                "  query: 'FROM saved-query | KEEP message'\n",
                vec!["--query", QUERY],
                QUERY,
            ),
            ("  table: saved-table\n", vec!["--query", QUERY], QUERY),
        ] {
            let fixture = Fixture::new();
            let server = query_server();
            fixture.write(
                "production",
                json!({"production": service(server.endpoint(), None)}),
            );
            fixture.view("canonical", &format!("name: canonical\nfilenames: ['.production.elasticsearch://']\nsource:\n  format: elasticsearch\n{selection}view: {{}}\n"));
            let mut args = vec![".production.es://"];
            args.extend(cli);
            success(&fixture.run(&args));
            assert_eq!(
                request_query(&server.requests()[0]),
                format!("{expected}\n| LIMIT 1001")
            );
        }
        let fixture = Fixture::new();
        let server = table_server();
        fixture.write(
            "production",
            json!({"production": service(server.endpoint(), None)}),
        );
        fixture.view(
            "table",
            "name: table\nfilenames: ['.elasticsearch://']\nsource:\n  table: logs-a\nview: {}\n",
        );
        success(&fixture.run(&[".es://"]));
        assert!(request_query(server.requests().last().unwrap()).contains("logs-a"));
        let fixture = Fixture::new();
        let server = table_server();
        fixture.write(
            "production",
            json!({"production": service(server.endpoint(), None)}),
        );
        fixture.view("query", "name: query\nfilenames: ['.elasticsearch://']\nsource:\n  query: 'FROM saved-query | KEEP message'\nview: {}\n");
        success(&fixture.run(&[".es://", "--table", "logs-a"]));
        let query = request_query(server.requests().last().unwrap());
        assert!(query.contains("logs-a"));
        assert!(!query.contains("saved-query"));
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn named_context_saved_identity_is_exact_and_does_not_match_current_prefix_or_case() {
        let fixture = Fixture::new();
        let server = Server::start(vec![
            Response::ok(ROWS),
            Response::ok(ROWS),
            Response::ok(ROWS),
            Response::ok(ROWS),
        ]);
        fixture.write("production.us-west", json!({"production.us-west": service(server.endpoint(), None), "Production.us-west": service(server.endpoint(), None), "production": service(server.endpoint(), None)}));
        fixture.view("named", "name: named\nfilenames: ['.production.us-west.elasticsearch://']\nsource:\n  query: 'FROM exact-name | KEEP message'\nview:\n  columns:\n    message: {label: EXACT_NAME}\n");
        let named = fixture.run(&[".production.us-west.es://"]);
        success(&named);
        assert_eq!(named.stdout, b"EXACT_NAME\ncontext-row\n");
        for source in [".es://", ".production.es://", ".Production.us-west.es://"] {
            let output = fixture.run(&[source, "--query", QUERY]);
            success(&output);
            assert_eq!(output.stdout, b"message\ncontext-row\n");
        }
    }

    #[test]
    fn empty_suffix_without_selection_reports_batch_requirement_after_discovery() {
        let fixture = Fixture::new();
        let server = Server::start(vec![Response::ok(crate::elasticrc::DISCOVERY)]);
        fixture.write(
            "production",
            json!({"production": service(server.endpoint(), None)}),
        );
        let output = fixture.run(&[".es://"]);
        preparation_failure(&output);
        assert_eq!(server.requests().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn custom_ca_trusts_context_tls_but_untrusted_certificates_are_rejected() {
        let tls = crate::elasticrc::TlsServer::start();
        let fixture = Fixture::new();
        fixture.write(
            "production",
            json!({"production": service(&tls.endpoint, None)}),
        );
        let untrusted = fixture.run(&[".es://", "--query", QUERY]);
        preparation_failure(&untrusted);
        let trusted = fixture
            .command()
            .env("ELASTIC_CA_CERT", &tls.ca)
            .args([".es://", "--query", QUERY])
            .output()
            .unwrap();
        success(&trusted);
        assert_eq!(trusted.stdout, b"message\ncontext-row\n");
        let invalid_ca = fixture.root.path().join("invalid-ca.pem");
        std::fs::write(&invalid_ca, "not a certificate").unwrap();
        preparation_failure(
            &fixture
                .command()
                .env("ELASTIC_CA_CERT", invalid_ca)
                .args([".es://", "--query", QUERY])
                .output()
                .unwrap(),
        );
    }

    #[test]
    fn failed_context_request_does_not_emit_server_secrets_or_partial_stdout() {
        let fixture = Fixture::new();
        let server = Server::start(vec![Response::error(
            401,
            r#"{"error":{"reason":"secret-server-sentinel"}}"#,
        )]);
        fixture.write("production", json!({"production": service(server.endpoint(), Some(json!({"api_key": "secret-api-sentinel"})))}));
        let output = fixture.run(&[".production.es://", "--query", QUERY]);
        preparation_failure(&output);
        no_secrets(
            &output,
            &[
                "secret-server-sentinel",
                "secret-api-sentinel",
                server.endpoint(),
            ],
        );
    }

    #[test]
    fn successful_context_warnings_redact_resolved_credentials_and_retain_partial_status() {
        let fixture = Fixture::new();
        let server = Server::start(vec![Response::ok(
            r#"{"columns":[{"name":"message","type":"keyword"}],"values":[["context-row"]],"is_partial":true,"warnings":["resolver returned resolved-warning-secret-sentinel"]}"#,
        )]);
        fixture.write("production", json!({"production": service(server.endpoint(), Some(json!({"api_key": "$(env:TVIEW_CONTEXT_SECRET)"})))}));
        let output = fixture
            .command()
            .env("TVIEW_CONTEXT_SECRET", "resolved-warning-secret-sentinel")
            .args([".production.es://", "--query", QUERY])
            .output()
            .unwrap();
        success(&output);
        assert_eq!(output.stdout, b"message\ncontext-row\n");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("partial result"),
            "partial-result indication was lost"
        );
        no_secrets(
            &output,
            &["resolved-warning-secret-sentinel", "resolver returned"],
        );
        assert_eq!(
            authorization(&server.requests()[0]),
            Some("ApiKey resolved-warning-secret-sentinel")
        );
    }
}
