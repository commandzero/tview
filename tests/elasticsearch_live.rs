#![cfg(feature = "elasticsearch")]

use assert_cmd::Command;
use predicates::prelude::*;
#[path = "support/elasticrc.rs"]
pub mod elasticrc;

use crate::elasticrc::{preparation_failure, service, success, Fixture};
use serde_json::json;

fn endpoint() -> String {
    std::env::var("TVIEW_ELASTICSEARCH_URL").unwrap_or_else(|_| "http://localhost:19200".to_owned())
}

fn tview() -> Command {
    Command::cargo_bin("tview").unwrap()
}

#[test]
#[ignore = "requires tests/fixtures/elasticsearch"]
fn generated_esql_reads_a_selected_index_without_mutating_it() {
    tview()
        .args([
            "--format",
            "elasticsearch",
            "--table",
            "logs-a",
            "--color",
            "never",
            &endpoint(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("boom"))
        .stdout(predicate::str::contains("ok"));
}

#[test]
#[ignore = "requires tests/fixtures/elasticsearch"]
fn complete_user_esql_supports_transforms_and_a_hard_limit() {
    tview()
        .args([
            "--format",
            "elasticsearch",
            "--query",
            "FROM logs-a | STATS count = COUNT(*) BY `log.level` | SORT count DESC",
            "--color",
            "never",
            &endpoint(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("count"));
}

#[test]
#[ignore = "requires tests/fixtures/elasticsearch"]
fn aliases_and_data_streams_work_as_explicit_from_targets() {
    tview()
        .args([
            "--format",
            "elasticsearch",
            "--table",
            "events-fixture",
            "--color",
            "never",
            &endpoint(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("stream"));
}

#[test]
#[ignore = "requires tests/fixtures/elasticsearch"]
fn discovery_mapping_field_caps_authentication_and_alias_passthrough_are_live() {
    tview()
        .args(["--format", "elasticsearch", &endpoint()])
        .assert()
        .failure()
        .stdout("")
        .stderr(predicate::str::contains("--table or --query"));

    tview()
        .env("ELASTIC_USERNAME", "fixture-user")
        .env("ELASTIC_PASSWORD", "fixture-password")
        .args([
            "--format",
            "elasticsearch",
            "--table",
            "logs-a",
            "--color",
            "never",
            &endpoint(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[\"api\",\"prod\"]"));

    tview()
        .args([
            "--format",
            "elasticsearch",
            "--table",
            "logs-current",
            "--color",
            "never",
            &endpoint(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("boom"));
}

#[test]
#[ignore = "requires tests/fixtures/elasticsearch"]
fn live_queries_apply_limits_transforms_and_remain_read_only() {
    let count = || {
        tview()
            .args([
                "--format",
                "elasticsearch",
                "--query",
                "FROM logs-a | STATS total = COUNT(*)",
                "--color",
                "never",
                &endpoint(),
            ])
            .output()
            .unwrap()
    };
    let before = count();
    assert!(before.status.success());

    tview()
        .args([
            "--format",
            "elasticsearch",
            "--query",
            "FROM logs-a | SORT @timestamp DESC | LIMIT 1",
            "--color",
            "never",
            &endpoint(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("ok"))
        .stdout(predicate::str::contains("boom").not());

    let after = count();
    assert!(after.status.success());
    assert_eq!(
        before.stdout, after.stdout,
        "viewing did not change documents"
    );
}

#[test]
#[ignore = "requires tests/fixtures/elasticsearch"]
fn live_conflict_partial_and_error_paths_keep_stdout_clean() {
    tview()
        .args([
            "--format",
            "elasticsearch",
            "--query",
            "FROM logs-a | THIS IS NOT ES|QL",
            &endpoint(),
        ])
        .assert()
        .failure()
        .stdout("");

    tview()
        .args([
            "--format",
            "elasticsearch",
            "--table",
            "logs-conflict_*",
            &endpoint(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("conflicted"));

    tview()
        .args([
            "--format",
            "elasticsearch",
            "--query",
            "FROM logs-a,logs-unavailable | KEEP message",
            "--color",
            "never",
            &endpoint(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("boom"))
        .stderr(predicate::str::contains("partial result"));
}

fn context_fixture() -> Fixture {
    let fixture = Fixture::new();
    fixture.write(
        "production",
        json!({
            "production": service(&endpoint(), None),
            "production.us-west": service(&endpoint(), None),
        }),
    );
    fixture
}

#[test]
#[ignore = "requires tests/fixtures/elasticsearch"]
fn live_context_current_named_dotted_aliases_and_pattern_suffixes_read_fixture_rows() {
    let fixture = context_fixture();
    for source in [
        ".es://logs-a",
        ".elasticsearch://logs-a",
        ".production.es://logs-a",
        ".production.elasticsearch://logs-a",
        ".production.us-west.es://logs-a",
        ".production.us-west.elasticsearch://logs-a",
        ".production.es://logs-current",
    ] {
        let output = fixture.run(&[source]);
        success(&output);
        let rows = String::from_utf8_lossy(&output.stdout);
        assert!(rows.contains("boom"), "{source}: {rows}");
        assert!(rows.contains("ok"), "{source}: {rows}");
        assert!(
            rows.contains("[\"api\",\"prod\"]"),
            "mapping/multivalue regression: {rows}"
        );
    }
    let output = fixture.run(&[".production.es://logs-conflict_*"]);
    success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("conflicted"));
}

#[test]
#[ignore = "requires tests/fixtures/elasticsearch"]
fn live_context_explicit_tables_queries_and_data_streams_preserve_existing_behavior() {
    let fixture = context_fixture();
    let table = fixture.run(&[".es://", "--table", "events-fixture"]);
    success(&table);
    assert!(String::from_utf8_lossy(&table.stdout).contains("stream"));
    let query = fixture.run(&[
        ".production.us-west.es://",
        "--query",
        "FROM logs-a | STATS count = COUNT(*) BY `log.level` | SORT count DESC",
    ]);
    success(&query);
    let rows = String::from_utf8_lossy(&query.stdout);
    assert!(rows.contains("count"));
    assert!(rows.contains("error"));
    assert!(rows.contains("info"));
}

#[test]
#[ignore = "requires tests/fixtures/elasticsearch"]
fn live_context_default_thousand_row_bound_is_enforced_by_real_esql() {
    let fixture = context_fixture();
    // Produce more than 1,000 rows on the real server without mutating the
    // shared fixture, so the source's default cap cannot pass accidentally.
    let values = (0..1200)
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let query = format!("ROW expansion = [{values}] | MV_EXPAND expansion");
    let output = fixture.run(&[".es://", "--query", &query]);
    success(&output);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().lines().count(),
        1001
    );
}

#[test]
#[ignore = "requires tests/fixtures/elasticsearch"]
fn live_context_partial_results_and_query_failures_keep_output_contract() {
    let fixture = context_fixture();
    let partial = fixture.run(&[
        ".es://",
        "--query",
        "FROM logs-a,logs-unavailable | KEEP message",
    ]);
    success(&partial);
    assert!(String::from_utf8_lossy(&partial.stdout).contains("boom"));
    assert!(String::from_utf8_lossy(&partial.stderr).contains("partial result"));
    let invalid = fixture.run(&[".es://", "--query", "FROM logs-a | THIS IS NOT ES|QL"]);
    preparation_failure(&invalid);
}

#[cfg(feature = "saved-views")]
#[test]
#[ignore = "requires tests/fixtures/elasticsearch"]
fn live_context_saved_table_and_query_selection_apply_through_canonical_identity() {
    for selection in [
        "  table: logs-a\n",
        "  query: 'FROM logs-a | KEEP message'\n",
    ] {
        let fixture = context_fixture();
        fixture.view("canonical", &format!(
            "name: canonical\nfilenames: ['.production.elasticsearch://']\nsource:\n{selection}view: {{}}\n"
        ));
        for source in [".production.es://", ".production.elasticsearch://"] {
            let output = fixture.run(&[source]);
            success(&output);
            let rows = String::from_utf8_lossy(&output.stdout);
            assert!(rows.contains("boom"));
            assert!(rows.contains("ok"));
        }
    }
}

#[cfg(feature = "saved-views")]
#[test]
#[ignore = "requires tests/fixtures/elasticsearch"]
fn live_context_saved_local_filter_does_not_refill_the_bounded_source_result() {
    let fixture = context_fixture();
    fixture.view("no-refill", "name: no-refill\nfilenames: ['.elasticsearch://']\nsource:\n  query: 'FROM logs-a | KEEP message | SORT message'\n  limit: 1\nview:\n  filters:\n    - {column: message, action: in, kind: text, condition: ok}\n");
    let output = fixture.run(&[".es://", "--output", "json"]);
    success(&output);
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON projection");
    assert_eq!(
        document["rows"],
        json!([]),
        "local filter was pushed down/refilled from the source"
    );
}
