use assert_cmd::Command;
use predicates::prelude::*;
use std::io::{BufRead, Cursor};
use std::process::Stdio;

#[cfg(feature = "elasticsearch")]
mod support;
#[cfg(feature = "elasticsearch")]
use support::elasticsearch_mock::{
    Response as ElasticsearchResponse, Server as ElasticsearchServer,
};

#[cfg(feature = "elasticsearch")]
fn elasticsearch_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn fixture(contents: &str, suffix: &str) -> tempfile::NamedTempFile {
    let file = tempfile::Builder::new()
        .suffix(suffix)
        .tempfile()
        .expect("temp file");
    std::fs::write(file.path(), contents).expect("write fixture");
    file
}

fn tview_command() -> Command {
    let mut command = Command::cargo_bin("tview").expect("binary");
    command.env(
        "XDG_CONFIG_HOME",
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-empty-config"),
    );
    command
}

#[cfg(feature = "sqlite")]
fn sqlite_fixture(statements: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().expect("sqlite fixture directory");
    let path = directory.path().join("fixture.db");
    let runtime = tokio::runtime::Runtime::new().expect("sqlite runtime");
    runtime.block_on(async {
        let database = turso::Builder::new_local(path.to_str().expect("utf8 path"))
            .experimental_generated_columns(true)
            .experimental_without_rowid(true)
            .build()
            .await
            .expect("sqlite database");
        let connection = database.connect().expect("sqlite connection");
        for statement in statements {
            connection
                .execute(statement, ())
                .await
                .expect("sqlite fixture statement");
        }
    });
    (directory, path)
}

#[test]
fn direct_table_and_automatic_redirection_match() {
    let file = fixture("Name,Count\nalpha,2\nbeta,10\n", ".csv");
    let expected = "Name   Count\nalpha      2\nbeta      10\n";

    tview_command()
        .args(["-o", "table"])
        .arg(file.path())
        .assert()
        .success()
        .stdout(expected)
        .stderr("");

    tview_command()
        .arg(file.path())
        .assert()
        .success()
        .stdout(expected)
        .stderr("");
}

#[test]
fn stdin_pipeline_uses_data_stream_without_terminal_access() {
    tview_command()
        .args(["-o", "table", "-"])
        .write_stdin("A,B\n1,2\n3,4\n")
        .assert()
        .success()
        .stdout("A  B\n1  2\n3  4\n")
        .stderr("");
}

#[test]
fn json_output_preserves_labels_controls_and_unclipped_display_values() {
    let file = fixture("Name,Name\n\"a\nb\",long-value\n", ".csv");
    let output = tview_command()
        .args(["--output", "json", "--width", "2"])
        .arg(file.path())
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let document: serde_json::Value = serde_json::from_slice(&output).expect("JSON document");
    assert_eq!(
        document,
        serde_json::json!({
            "columns": ["Name", "Name"], "rows": [["a\nb", "long-value"]]
        })
    );
}

#[test]
fn jsonl_output_frames_each_row_and_handles_late_columns() {
    let output = tview_command()
        .args(["--format", "ndjson", "--output", "jsonl", "-"])
        .write_stdin("{\"a\":1}\n{\"a\":2,\"b\":true}\n")
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let records: Vec<serde_json::Value> = Cursor::new(output.as_slice())
        .lines()
        .map(|line| serde_json::from_str(&line.expect("line")).expect("JSONL record"))
        .collect();
    assert_eq!(
        records,
        vec![
            serde_json::json!({"columns": ["a", "b"], "values": ["1", ""]}),
            serde_json::json!({"columns": ["a", "b"], "values": ["2", "true"]}),
        ]
    );
    assert!(output.ends_with(b"\n"));
}

#[test]
fn structured_output_rejects_ansi_and_leaves_stdout_empty_on_source_failure() {
    for format in ["json", "jsonl"] {
        tview_command()
            .args(["--output", format, "--color", "always", "-"])
            .write_stdin("A\nvalue\n")
            .assert()
            .code(1)
            .stdout("");
        tview_command()
            .args(["--format", "json", "--output", format, "-"])
            .write_stdin("[{ broken]")
            .assert()
            .code(1)
            .stdout("");
    }
}

#[test]
fn version_and_usage_have_stable_exit_codes() {
    tview_command()
        .arg("--version")
        .assert()
        .code(0)
        .stdout(concat!("tview ", env!("CARGO_PKG_VERSION"), "\n"))
        .stderr("");
    tview_command().arg("--help").assert().code(0).stderr("");
    tview_command()
        .args(["--output", "invalid", "-"])
        .assert()
        .code(2)
        .stdout("");
}

#[test]
fn stdin_pipeline_preserves_keyed_object_modes() {
    let input = r#"{"alpha":{"stars":1},"beta":{"stars":2},"gamma":{"stars":3}}"#;

    tview_command()
        .args(["--format", "json", "-o", "table", "-"])
        .write_stdin(input)
        .assert()
        .success()
        .stdout("name   stars\nalpha      1\nbeta       2\ngamma      3\n");

    tview_command()
        .args([
            "--format",
            "json",
            "--object-mode",
            "record",
            "-o",
            "table",
            "-",
        ])
        .write_stdin(input)
        .assert()
        .success()
        .stdout("alpha.stars  beta.stars  gamma.stars\n          1           2            3\n");
}

#[test]
fn structured_sources_include_late_columns_and_ignore_start_position() {
    let json = fixture(
        "[{\"id\":1,\"name\":\"alpha\"},{\"id\":2,\"name\":\"beta\",\"late\":true}]",
        ".json",
    );
    tview_command()
        .args(["-o", "table", "--start_pos", "2,2"])
        .arg(json.path())
        .assert()
        .success()
        .stdout("id  name   late\n 1  alpha  \n 2  beta   true\n");

    let ndjson = fixture(
        "{\"id\":1,\"name\":\"alpha\"}\n{\"id\":2,\"name\":\"beta\",\"late\":true}\n",
        ".ndjson",
    );
    tview_command()
        .args(["-o", "table"])
        .arg(ndjson.path())
        .assert()
        .success()
        .stdout("id  name   late\n 1  alpha  \n 2  beta   true\n");
}

#[test]
fn color_is_plain_by_default_and_opt_in() {
    let file = fixture("A,B\n1,2\n", ".csv");
    tview_command()
        .args(["-o", "table"])
        .arg(file.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("\u{1b}[").not());

    tview_command()
        .args(["-o", "table", "--color", "always"])
        .arg(file.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("\u{1b}["));
}

#[test]
fn unsupported_formats_and_colors_fail_during_cli_parsing() {
    tview_command()
        .args(["-o", "tui", "-"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid value 'tui'"));

    tview_command()
        .args(["--color", "sometimes", "-"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid value 'sometimes'"));
}

#[test]
fn source_errors_leave_stdout_empty() {
    let file = fixture("[{ broken]", ".json");
    tview_command()
        .args(["-o", "table"])
        .arg(file.path())
        .assert()
        .failure()
        .stdout("")
        .stderr(predicate::str::is_empty().not());
}

#[cfg(feature = "elasticsearch")]
#[test]
fn elasticsearch_direct_native_query_waits_and_emits_only_table_bytes() {
    let _guard = elasticsearch_test_lock();
    let server = ElasticsearchServer::start(vec![ElasticsearchResponse::delayed(
        r#"{"columns":[{"name":"level","type":"keyword"}],"values":[["error"]]}"#,
        std::time::Duration::from_millis(40),
    )]);
    let started = std::time::Instant::now();
    tview_command()
        .args([
            "--format",
            "elasticsearch",
            "--query",
            "FROM logs-* | KEEP level",
            "--color",
            "never",
            server.endpoint(),
        ])
        .assert()
        .success()
        .stdout("level\nerror\n")
        .stderr("");
    assert!(started.elapsed() >= std::time::Duration::from_millis(35));
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains(r#""query":"FROM logs-* | KEEP level\n| LIMIT 1001""#));
    assert!(!requests[0].contains("Authorization"));
}

#[cfg(feature = "elasticsearch")]
#[test]
fn elasticsearch_selected_target_preserves_multivalues_and_warns_partial_on_stderr() {
    let _guard = elasticsearch_test_lock();
    let server = ElasticsearchServer::start(vec![
        ElasticsearchResponse::ok(
            r#"{"logs-a":{"mappings":{"properties":{"tags":{"type":"keyword"}}}}}"#,
        ),
        ElasticsearchResponse::ok(
            r#"{"fields":{"tags":{"keyword":{"searchable":true,"aggregatable":true}}}}"#,
        ),
        ElasticsearchResponse::ok(
            r#"{"columns":[{"name":"tags","type":"keyword"}],"values":[[["prod","api"]]],"is_partial":true,"warnings":["fixture shard warning"]}"#,
        ),
    ]);
    tview_command()
        .args([
            "--format",
            "elasticsearch",
            "--table",
            "logs-a",
            "--color",
            "never",
            server.endpoint(),
        ])
        .assert()
        .success()
        .stdout("tags\n[\"prod\",\"api\"]\n")
        .stderr(
            predicate::str::contains("partial result")
                .and(predicate::str::contains("fixture shard warning")),
        );
    assert_eq!(server.requests().len(), 3);
}

#[cfg(feature = "elasticsearch")]
#[test]
fn elasticsearch_direct_output_without_selection_and_query_failures_keep_stdout_clean() {
    let _guard = elasticsearch_test_lock();
    for discovery_body in [
        r#"{"indices":[{"name":"logs-a","attributes":["open"]}],"aliases":[],"data_streams":[]}"#,
        r#"{"indices":[],"aliases":[],"data_streams":[]}"#,
    ] {
        let discovery = ElasticsearchServer::start(vec![ElasticsearchResponse::ok(discovery_body)]);
        tview_command()
            .args(["--format", "elasticsearch", discovery.endpoint()])
            .assert()
            .failure()
            .stdout("")
            .stderr(predicate::str::contains("--table or --query"));
    }

    let failure = ElasticsearchServer::start(vec![ElasticsearchResponse::error(
        400,
        r#"{"error":{"reason":"invalid ES|QL fixture"}}"#,
    )]);
    tview_command()
        .args([
            "--format",
            "elasticsearch",
            "--query",
            "FROM broken",
            failure.endpoint(),
        ])
        .assert()
        .failure()
        .stdout("")
        .stderr(predicate::str::contains("invalid ES|QL fixture"));
}

#[cfg(feature = "sqlite")]
#[test]
fn sqlite_batch_selects_a_sole_table() {
    let (_directory, path) = sqlite_fixture(&[
        "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT)",
        "INSERT INTO users VALUES (1, 'Ada')",
        "INSERT INTO users VALUES (2, 'Grace')",
        "INSERT INTO users VALUES (3, 'Linus')",
    ]);

    tview_command()
        .args(["-o", "table"])
        .arg(&path)
        .assert()
        .success()
        .stdout(predicate::str::contains("id  name"))
        .stdout(predicate::str::contains("Ada"))
        .stdout(predicate::str::contains("Grace"))
        .stdout(predicate::str::contains("Linus"))
        .stderr("");
}

#[cfg(feature = "sqlite")]
#[test]
fn bundled_sqlite_sample_opens_as_one_thousand_rows() {
    let source =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sample/us-counties.sqlite3");
    let directory = tempfile::tempdir().expect("sample copy directory");
    let path = directory.path().join("us-counties.sqlite3");
    std::fs::copy(source, &path).expect("copy bundled SQLite sample");
    let output = tview_command()
        .args(["-o", "table"])
        .arg(path)
        .output()
        .expect("open bundled SQLite sample");

    assert!(output.status.success(), "status: {:?}", output.status);
    assert!(output.stderr.is_empty(), "stderr: {:?}", output.stderr);

    let stdout = String::from_utf8(output.stdout).expect("UTF-8 table output");
    let mut lines = stdout.lines();
    let header = lines.next().expect("table header");
    assert!(header.contains("fips"));
    assert!(header.contains("county_name"));
    assert!(header.contains("net_migration_rate_2020"));
    assert_eq!(lines.count(), 1_000);
}

#[cfg(feature = "sqlite")]
#[test]
fn sqlite_ambiguous_batch_requires_table_without_emitting_stdout() {
    let (_directory, path) = sqlite_fixture(&[
        "CREATE TABLE users(id INTEGER PRIMARY KEY)",
        "CREATE TABLE events(id INTEGER PRIMARY KEY)",
    ]);

    tview_command()
        .args(["-o", "table"])
        .arg(&path)
        .assert()
        .failure()
        .stdout("")
        .stderr(predicate::str::contains("--table"));

    tview_command()
        .args(["--table", "events", "-o", "table"])
        .arg(&path)
        .assert()
        .success()
        .stdout(predicate::str::contains("id"));
}

#[cfg(feature = "sqlite")]
#[test]
fn sqlite_stdin_and_remote_sources_are_rejected_cleanly() {
    tview_command()
        .args(["--format", "sqlite", "-o", "table", "-"])
        .write_stdin(b"SQLite format 3\0".as_slice())
        .assert()
        .failure()
        .stdout("")
        .stderr(predicate::str::contains("stdin"));

    tview_command()
        .args([
            "--format",
            "sqlite",
            "-o",
            "table",
            "libsql://example.turso.io/database",
        ])
        .assert()
        .failure()
        .stdout("")
        .stderr(predicate::str::contains("remote"));
}

#[test]
fn warnings_use_stderr_without_corrupting_table_bytes() {
    let config = tempfile::tempdir().expect("config dir");
    let themes = config.path().join("tview/themes");
    std::fs::create_dir_all(&themes).expect("themes dir");
    std::fs::write(themes.join("broken.yml"), "name: broken\nstyles: nope\n")
        .expect("broken theme");
    std::fs::write(
        themes.join("also-broken.yml"),
        "name: also-broken\nstyles: nope\n",
    )
    .expect("second broken theme");
    let file = fixture("A,B\n1,2\n", ".csv");

    let output = tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["-o", "table"])
        .arg(file.path())
        .output()
        .expect("run tview");
    assert!(output.status.success());
    assert_eq!(output.stdout, b"A  B\n1  2\n");
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert_eq!(stderr.matches("theme warning:").count(), 2, "{stderr}");
}

#[test]
fn early_closing_consumer_is_a_clean_exit() {
    let mut contents = String::from("id,value\n");
    for index in 0..100_000 {
        contents.push_str(&format!("{index},row-{index}\n"));
    }
    let file = fixture(&contents, ".csv");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_tview"))
        .env(
            "XDG_CONFIG_HOME",
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-empty-config"),
        )
        .args(["-o", "table"])
        .arg(file.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn tview");
    let mut stdout = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    let mut first_line = String::new();
    stdout.read_line(&mut first_line).expect("first line");
    assert_eq!(first_line.trim(), "id  value");
    drop(stdout);

    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success(), "status: {:?}", output.status);
    assert!(output.stderr.is_empty(), "stderr: {:?}", output.stderr);
}

#[cfg(feature = "saved-views")]
#[test]
fn saved_view_controls_non_interactive_projection_and_can_be_disabled() {
    let config = tempfile::tempdir().expect("config dir");
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).expect("views dir");
    std::fs::write(
        views.join("scripted.yml"),
        r#"
name: scripted
filenames:
  - "*"
source: {}
view:
  columns:
    Name:
      label: NAME
      format: uppercase
    Count:
      type: integer
      width: 4
      align: right
    Extra:
      visible: false
  sort:
    - column: Count
      direction: desc
      kind: numeric
  filters:
    - column: Count
      action: in
      kind: numeric
      condition: ">2"
"#,
    )
    .expect("saved view");
    let file = fixture(
        "Name,Count,Extra\nalpha,2,x\nbeta,10,y\ngamma,5,z\n",
        ".csv",
    );

    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["-o", "table", "--view", "scripted"])
        .arg(file.path())
        .assert()
        .success()
        .stdout("NAME   Coun\nBETA     10\nGAMMA     5\n")
        .stderr("");

    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["-o", "table", "--no-view"])
        .arg(file.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Extra"))
        .stdout(predicate::str::contains("alpha"));
}

#[cfg(all(feature = "saved-views", feature = "sqlite"))]
#[test]
fn sqlite_saved_source_and_view_layers_apply_in_order() {
    let config = tempfile::tempdir().expect("config dir");
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).expect("views dir");
    std::fs::write(
        views.join("sqlite.yml"),
        r#"
name: sqlite
filenames: ["*"]
source:
  format: sqlite
  table: events
  limit: 2
  filters:
    - column: active
      operator: equal
      value: true
  sort:
    - column: id
      direction: desc
view:
  filters:
    - column: name
      action: in
      kind: text
      condition: a
  sort:
    - column: name
      direction: asc
      kind: lexical
"#,
    )
    .expect("saved view");
    let (_directory, path) = sqlite_fixture(&[
        "CREATE TABLE events(id INTEGER PRIMARY KEY, name TEXT, active INTEGER)",
        "INSERT INTO events VALUES (1, 'alpha', 1)",
        "INSERT INTO events VALUES (2, 'beta', 0)",
        "INSERT INTO events VALUES (3, 'gamma', 1)",
        "INSERT INTO events VALUES (4, 'delta', 1)",
    ]);

    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["--view", "sqlite", "-o", "table"])
        .arg(&path)
        .assert()
        .success()
        .stdout(predicate::str::contains("delta"))
        .stdout(predicate::str::contains("gamma"))
        .stdout(predicate::str::contains("alpha").not())
        .stdout(predicate::str::contains("beta").not());
}

#[cfg(feature = "saved-views")]
#[test]
fn saved_view_warnings_are_emitted_once() {
    let config = tempfile::tempdir().expect("config dir");
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).expect("views dir");
    std::fs::write(
        views.join("warning.yml"),
        r#"
name: warning
filenames: ["*"]
source: {}
view:
  columns:
    Missing:
      width: 5
"#,
    )
    .expect("saved view");
    let file = fixture("A,B\n1,2\n", ".csv");

    let output = tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["-o", "table", "--view", "warning"])
        .arg(file.path())
        .output()
        .expect("run tview");
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert_eq!(
        stderr.matches("saved view: view.columns.Missing:").count(),
        1,
        "{stderr}"
    );
}

#[test]
fn preview_options_reject_non_table_modes_before_input() {
    for mode in [
        vec!["--output", "json"],
        vec!["--output", "jsonl"],
        vec!["--interactive"],
        vec!["--interactive", "--output", "table"],
    ] {
        for option in [vec!["--sorted", "true"], vec!["-n", "2"]] {
            tview_command()
                .args(&mode)
                .args(&option)
                .arg("/does/not/exist")
                .assert()
                .code(1)
                .stdout("")
                .stderr(predicate::str::contains("require direct table output"));
        }
    }
}

#[test]
fn previews_stop_before_unread_suffix_and_freeze_layout() {
    for (suffix, contents, expected) in [
        (
            ".csv",
            "A,B\n1,x\n2,y\n3,very-long-value\n4,z\n",
            "A  B\n1  x\n2  y\nmore rows...\n",
        ),
        (
            ".json",
            r#"[{"a":1},{"a":2},{"a":3,"late":true},invalid]"#,
            "a\n1\n2\nmore rows...\n",
        ),
        (
            ".ndjson",
            "{\"a\":1}\n{\"a\":2}\n{\"a\":3,\"late\":true}\ninvalid\n",
            "a\n1\n2\nmore rows...\n",
        ),
    ] {
        let file = fixture(contents, suffix);
        tview_command()
            .args(["--sorted", "false", "-n", "2"])
            .arg(file.path())
            .assert()
            .success()
            .stdout(expected)
            .stderr("");
    }
}

#[cfg(feature = "saved-views")]
#[test]
fn preview_sort_override_preserves_filters_and_saved_file() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    let yaml = "name: preview\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    Name: {format: uppercase}\n  sort:\n    - {column: Count, direction: desc, kind: numeric}\n  filters:\n    - {column: Count, action: in, kind: numeric, condition: '>2'}\n";
    let saved = views.join("preview.yml");
    std::fs::write(&saved, yaml).unwrap();
    let file = fixture("Name,Count\nalpha,1\nbeta,3\ngamma,5\ndelta,4\n", ".csv");
    for (sorting, expected) in [
        ("false", "Name  Count\nBETA      3\n2 more rows...\n"),
        ("true", "Name   Count\nGAMMA      5\n2 more rows...\n"),
    ] {
        tview_command()
            .env("XDG_CONFIG_HOME", config.path())
            .args(["--sorted", sorting, "-n", "1"])
            .arg(file.path())
            .assert()
            .success()
            .stdout(expected);
    }
    assert_eq!(std::fs::read_to_string(saved).unwrap(), yaml);
}

#[test]
fn preview_boundaries_multiline_and_nested_objects() {
    for (contents, suffix, extra, expected) in [
        ("A\n1\n2\n", ".csv", vec![], "A\n1\n2\n"),
        ("", ".csv", vec![], ""),
        (
            "A,B\n1,\"a\nb\"\n2,c\n3,d\n",
            ".csv",
            vec![],
            "A  B\n1  a\\nb\n2  c\nmore rows...\n",
        ),
        (
            r#"{"data":[{"a":1},{"a":2},{"a":3}],"tail":invalid}"#,
            ".json",
            vec!["--json-path", "/data"],
            "a\n1\n2\nmore rows...\n",
        ),
        (
            r#"{"x":{"a":1},"y":{"a":2},"z":{"a":3},"bad":invalid}"#,
            ".json",
            vec!["--object-mode", "entries"],
            "name  a\nx     1\ny     2\nmore rows...\n",
        ),
    ] {
        let file = fixture(contents, suffix);
        tview_command()
            .args(["--sorted", "false", "--top-lines", "2"])
            .args(extra)
            .arg(file.path())
            .assert()
            .success()
            .stdout(expected);
    }
}

#[test]
fn preview_waits_only_for_required_stdin_rows() {
    use std::io::Write;
    use std::time::{Duration, Instant};
    for (format, input) in [
        ("delimited", "A\n1\n2\n3\n"),
        ("ndjson", "{\"a\":1}\n{\"a\":2}\n{\"a\":3}\n"),
        ("json", "[{\"a\":1},{\"a\":2},{\"a\":3},"),
    ] {
        let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("tview"))
            .env("XDG_CONFIG_HOME", tempfile::tempdir().unwrap().path())
            .args(["--format", format, "--sorted", "false", "-n", "2", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input_pipe = child.stdin.take().unwrap();
        input_pipe.write_all(input.as_bytes()).unwrap();
        input_pipe.flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("{format} preview waited for EOF");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.ends_with(b"more rows...\n"));
        drop(input_pipe);
    }
}

#[test]
fn preview_auto_keyed_object_does_not_wait_for_object_eof() {
    use std::io::Write;
    use std::time::{Duration, Instant};

    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("tview"))
        .args(["--format", "json", "--sorted", "false", "-n", "1", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input
        .write_all(br#"{"a":{"id":1},"b":{"id":2},"c":{"id":3},"d":{"id":4}"#)
        .unwrap();
    input.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("keyed-object preview waited for EOF");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "name  id\na      1\nmore rows...\n"
    );
}

#[test]
fn preview_ndjson_rejects_scalar_documents() {
    tview_command()
        .args(["--format", "ndjson", "--sorted", "false", "-n", "1", "-"])
        .write_stdin("1\n")
        .assert()
        .code(1)
        .stdout("")
        .stderr(predicate::str::contains(
            "JSON starting path does not identify an object or array",
        ));
}

#[test]
fn preview_errors_in_required_lookahead_leave_stdout_empty() {
    let file = fixture(r#"[{"a":1},{"a":2},invalid]"#, ".json");
    tview_command()
        .args(["-n", "2"])
        .arg(file.path())
        .assert()
        .code(1)
        .stdout("");
}

#[test]
fn preview_full_schema_and_fixed_width_are_honored() {
    let file = fixture(r#"[{"a":"long"},{"a":"b"},{"late":true}]"#, ".json");
    tview_command()
        .args(["-n", "1", "--schema-scan", "full", "--width", "2"])
        .arg(file.path())
        .assert()
        .success()
        .stdout("a   la\nlo\n2 more rows...\n");
}

#[test]
fn preview_full_delimited_schema_includes_late_columns() {
    let file = fixture("a,b\n1,x\n2,y,z\n", ".csv");
    tview_command()
        .args(["-n", "1", "--schema-scan", "full"])
        .arg(file.path())
        .assert()
        .success()
        .stdout("a  b  Column 3\n1  x\n1 more rows...\n")
        .stderr("");
}

#[cfg(feature = "saved-views")]
#[test]
fn preview_source_filters_do_not_expose_filtered_schema_columns() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(
        views.join("filtered.yml"),
        "name: filtered\nfilenames: ['*']\nsource:\n  filters:\n    - {column: id, operator: equal, value: '2'}\nview: {}\n",
    )
    .unwrap();
    let file = fixture("id,name\n0,ignored,late\n2,kept\n", ".csv");

    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["--view", "filtered", "--schema-scan", "full", "-n", "1"])
        .arg(file.path())
        .assert()
        .success()
        .stdout("id  name\n 2  kept\n")
        .stderr("");
}

#[cfg(feature = "saved-views")]
#[test]
fn preview_source_filters_do_not_expose_rejected_schema_columns_when_empty() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(
        views.join("filtered.yml"),
        "name: filtered\nfilenames: ['*']\nsource:\n  filters:\n    - {column: id, operator: equal, value: '2'}\nview: {}\n",
    )
    .unwrap();
    let file = fixture("id,name\n0,ignored,late\n", ".csv");

    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["--view", "filtered", "--schema-scan", "full", "-n", "1"])
        .arg(file.path())
        .assert()
        .success()
        .stdout("id  name\n")
        .stderr("");
}

#[cfg(feature = "saved-views")]
#[test]
fn preview_source_filters_expose_columns_from_accepted_rows() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(
        views.join("filtered.yml"),
        "name: filtered\nfilenames: ['*']\nsource:\n  filters:\n    - {column: id, operator: equal, value: '2'}\nview: {}\n",
    )
    .unwrap();
    let file = fixture("id,name\n0,ignored,rejected\n2,kept,accepted\n", ".csv");

    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["--view", "filtered", "--schema-scan", "full", "-n", "1"])
        .arg(file.path())
        .assert()
        .success()
        .stdout("id  name  Column 3\n 2  kept  accepted\n")
        .stderr("");
}

#[cfg(feature = "saved-views")]
#[test]
fn preview_view_filters_do_not_expose_rejected_schema_columns() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(
        views.join("filtered.yml"),
        "name: filtered\nfilenames: ['*']\nsource: {}\nview:\n  filters:\n    - {column: keep, action: in, kind: text, condition: yes}\n",
    )
    .unwrap();
    let file = fixture(
        r#"[{"keep":"yes","id":1},{"keep":"no","late":"rejected"}]"#,
        ".json",
    );

    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["--view", "filtered", "--sorted", "false", "-n", "1"])
        .arg(file.path())
        .assert()
        .success()
        .stdout("keep  id\nyes    1\n")
        .stderr("");
}

#[cfg(feature = "saved-views")]
#[test]
fn preview_view_filters_with_no_matches_do_not_render_initial_schema() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(
        views.join("filtered.yml"),
        "name: filtered\nfilenames: ['*']\nsource: {}\nview:\n  filters:\n    - {column: keep, action: in, kind: text, condition: yes}\n",
    )
    .unwrap();
    let file = fixture(r#"[{"keep":"no","late":"rejected"}]"#, ".json");

    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["--view", "filtered", "--sorted", "false", "-n", "1"])
        .arg(file.path())
        .assert()
        .success()
        .stdout("")
        .stderr("");
}

#[cfg(feature = "saved-views")]
#[test]
fn preview_view_filters_full_schema_keeps_late_accepted_fields() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(
        views.join("filtered.yml"),
        "name: filtered\nfilenames: ['*']\nsource: {}\nview:\n  filters:\n    - {column: keep, action: in, kind: text, condition: yes}\n",
    )
    .unwrap();
    let file = fixture(
        r#"[{"keep":"yes","id":1},{"keep":"yes","late":"accepted"},{"keep":"no","rejected":true}]"#,
        ".json",
    );

    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args([
            "--view",
            "filtered",
            "--schema-scan",
            "full",
            "--sorted",
            "false",
            "-n",
            "1",
        ])
        .arg(file.path())
        .assert()
        .success()
        .stdout("keep  id  late\nyes    1\n1 more rows...\n")
        .stderr("");
}

#[cfg(feature = "saved-views")]
#[test]
fn preview_source_limit_reports_exact_remainder() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(
        views.join("limited.yml"),
        "name: limited\nfilenames: ['*']\nsource:\n  limit: 2\nview: {}\n",
    )
    .unwrap();
    let file = fixture("id\n1\n2\n3\n", ".csv");

    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["--view", "limited", "--sorted", "false", "-n", "1"])
        .arg(file.path())
        .assert()
        .success()
        .stdout("id\n 1\n1 more rows...\n")
        .stderr("");
}

#[cfg(feature = "saved-views")]
#[test]
fn preview_source_limit_validates_unresolved_source_filters() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(
        views.join("unknown.yml"),
        "name: unknown\nfilenames: ['*']\nsource:\n  limit: 1\n  filters:\n    - {column: late, operator: is_null}\nview: {}\n",
    )
    .unwrap();
    let file = fixture("id\n1\n", ".csv");

    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["--view", "unknown", "--sorted", "false", "-n", "1"])
        .arg(file.path())
        .assert()
        .code(1)
        .stdout("")
        .stderr(predicate::str::contains(
            "source operation column 'late' was not found",
        ));
}

#[cfg(feature = "saved-views")]
#[test]
fn preview_missing_saved_filters_do_not_discard_rows() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(
        views.join("missing.yml"),
        "name: missing\nfilenames: ['*']\nsource: {}\nview:\n  filters:\n    - {column: /missing, action: in, kind: text, condition: yes}\n",
    )
    .unwrap();
    let file = fixture(r#"[{"a":1},{"a":2}]"#, ".json");

    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["--view", "missing", "--sorted", "false", "-n", "1"])
        .arg(file.path())
        .assert()
        .success()
        .stdout("a\n1\n1 more rows...\n")
        .stderr("");
}

#[cfg(feature = "saved-views")]
#[test]
fn preview_numeric_saved_filters_use_late_profile_evidence() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(
        views.join("numeric.yml"),
        r#"name: numeric
filenames: ['*']
source: {}
view:
  filters:
    - {column: Value, action: in, kind: numeric, condition: '>2m'}
"#,
    )
    .unwrap();
    let file = fixture("Name,Value\na,1\nb,2m\nc,3s\nd,1h\n", ".csv");

    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["--view", "numeric", "--sorted", "false", "-n", "1"])
        .arg(file.path())
        .assert()
        .success()
        .stdout("Name  Value\nd        1h\n")
        .stderr("");
}

#[cfg(feature = "saved-views")]
#[test]
fn preview_sorted_numeric_profile_uses_materialized_rows() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(
        views.join("sorted.yml"),
        "name: sorted\nfilenames: ['*']\nsource: {}\nview:\n  sort:\n    - {column: Value, direction: asc, kind: numeric}\n",
    )
    .unwrap();
    let file = fixture("Name,Value\na,1\nb,2m\nc,3s\n", ".csv");

    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["--view", "sorted", "--sorted", "true", "-n", "1"])
        .arg(file.path())
        .assert()
        .success()
        .stdout("Name  Value\na         1\n2 more rows...\n")
        .stderr("");
}

#[cfg(feature = "elasticsearch")]
#[test]
fn elasticsearch_preview_uses_one_bounded_query_and_known_remainder() {
    let _guard = elasticsearch_test_lock();
    let server = ElasticsearchServer::start(vec![ElasticsearchResponse::ok(
        r#"{"columns":[{"name":"level","type":"keyword"}],"values":[["error"],["warning"],["info"]]}"#,
    )]);
    tview_command()
        .args([
            "--format",
            "elasticsearch",
            "--query",
            "FROM logs-* | KEEP level",
            "--sorted",
            "false",
            "-n",
            "1",
            server.endpoint(),
        ])
        .assert()
        .success()
        .stdout("level\nerror\n2 more rows...\n")
        .stderr("");
    assert_eq!(server.requests().len(), 1);
    assert!(server.requests()[0].contains("LIMIT 1001"));
}

#[cfg(all(feature = "saved-views", feature = "sqlite"))]
#[test]
fn sqlite_preview_preserves_native_order_and_does_not_refill_filtered_limit() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(views.join("sqlite.yml"), "name: sqlite\nfilenames: ['*']\nsource:\n  format: sqlite\n  table: events\n  limit: 3\n  sort:\n    - {column: id, direction: desc}\nview:\n  filters:\n    - {column: id, action: in, kind: numeric, condition: '<4'}\n  sort:\n    - {column: id, direction: asc, kind: numeric}\n").unwrap();
    let (_directory, path) = sqlite_fixture(&[
        "CREATE TABLE events(id INTEGER PRIMARY KEY)",
        "INSERT INTO events VALUES (1), (2), (3), (4)",
    ]);
    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["--sorted", "false", "-n", "10"])
        .arg(path)
        .assert()
        .success()
        .stdout("id\n 3\n 2\n");
}

#[cfg(feature = "saved-views")]
#[test]
fn preview_file_source_limit_and_selective_view_filters_stop_at_the_right_rows() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(views.join("filtered.yml"), "name: filtered\nfilenames: ['*']\nsource:\n  limit: 105\n  filters:\n    - {column: id, operator: is_not_null}\nview:\n  filters:\n    - {column: id, action: in, kind: numeric, condition: '>100'}\n").unwrap();
    let content = format!(
        "id\n{}",
        (1..1000).map(|n| format!("{n}\n")).collect::<String>()
    );
    let file = fixture(&content, ".csv");
    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["--sorted", "false", "-n", "3"])
        .arg(file.path())
        .assert()
        .success()
        .stdout(" id\n101\n102\n103\n2 more rows...\n");
    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["--sorted", "false", "-n", "10"])
        .arg(file.path())
        .assert()
        .success()
        .stdout(" id\n101\n102\n103\n104\n105\n");
}

#[cfg(feature = "saved-views")]
#[test]
fn preview_pending_columns_and_sorts_do_not_force_a_scan() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(views.join("late.yml"), "name: late\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    /late: {format: uppercase}\n  sort:\n    - {column: /sort, direction: desc, kind: numeric}\n  filters:\n    - {column: /late, action: in, kind: text, condition: yes}\n").unwrap();
    let file = fixture(
        r#"[{"a":1},{"late":"yes"},{"late":"yes","sort":2},invalid]"#,
        ".json",
    );
    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["--sorted", "false", "-n", "1"])
        .arg(file.path())
        .assert()
        .success()
        .stdout("late\nYES\nmore rows...\n");
}

#[test]
fn preview_delimiter_detection_uses_bounded_followup_lines() {
    let file = fixture("\n# Name Value\nalpha 1\nbeta 2\n", ".txt");

    tview_command()
        .args(["--sorted", "false", "-n", "2"])
        .arg(file.path())
        .assert()
        .success()
        .stdout("Name   Value\nalpha      1\nbeta       2\n")
        .stderr("");
}

#[cfg(feature = "saved-views")]
#[test]
fn preview_saved_content_width_uses_selected_rows() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(
        views.join("width.yml"),
        "name: width\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    Name:\n      width: content\n",
    )
    .unwrap();
    let file = fixture("Name,Kind\na,x\nvery-long-value,y\n", ".csv");

    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["--view", "width", "--sorted", "false", "-n", "2"])
        .arg(file.path())
        .assert()
        .success()
        .stdout("Name             Kind\na                x\nvery-long-value  y\n")
        .stderr("");
}

#[test]
fn preview_keyed_object_auto_stops_before_malformed_suffix() {
    let file = fixture(
        r#"{"a":{"id":1},"b":{"id":2},"c":{"id":3},invalid}"#,
        ".json",
    );

    tview_command()
        .args([
            "--format",
            "json",
            "--object-mode",
            "auto",
            "--sorted",
            "false",
            "-n",
            "1",
        ])
        .arg(file.path())
        .assert()
        .success()
        .stdout("name  id\na      1\nmore rows...\n")
        .stderr("");
}

#[cfg(feature = "saved-views")]
#[test]
fn preview_source_limit_does_not_widen_headerless_schema() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(
        views.join("limited.yml"),
        "name: limited\nfilenames: ['*']\nsource:\n  limit: 1\nview: {}\n",
    )
    .unwrap();
    let file = fixture("1,2\n3,4,late\n", ".csv");

    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["--view", "limited", "--sorted", "false", "-n", "1"])
        .arg(file.path())
        .assert()
        .success()
        .stdout("       1         2\n")
        .stderr("");
}

#[cfg(feature = "saved-views")]
#[test]
fn preview_color_profiles_ignore_omitted_values() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(views.join("colors.yml"), "name: colors\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    /a:\n      type: number\n      colors:\n        - gradient: {mode: auto, steps: 8, colors: [green, yellow]}\n").unwrap();
    let mut rendered = Vec::new();
    for tail in ["3", "99999999"] {
        let file = fixture(
            &format!("[{{\"a\":1}},{{\"a\":2}},{{\"a\":{tail}}},invalid]"),
            ".json",
        );
        let output = tview_command()
            .env("XDG_CONFIG_HOME", config.path())
            .args([
                "--sorted", "false", "-n", "2", "--color", "always", "--width", "max",
            ])
            .arg(file.path())
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert!(output.windows(2).any(|bytes| bytes == b"\x1b["));
        assert!(output.ends_with(b"more rows...\n"));
        rendered.push(output);
        tview_command()
            .env("XDG_CONFIG_HOME", config.path())
            .args(["--sorted", "false", "-n", "2", "--color", "never"])
            .arg(file.path())
            .assert()
            .success()
            .stdout("a\n1\n2\nmore rows...\n");
    }
    assert_eq!(rendered[0], rendered[1]);
}

#[cfg(feature = "saved-views")]
#[test]
fn preview_source_filters_resolve_late_json_fields() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(views.join("source.yml"), "name: source\nfilenames: ['*']\nsource:\n  filters:\n    - {column: /late, operator: equal, value: 'yes'}\nview: {}\n").unwrap();
    let file = fixture(
        r#"[{"a":1},{"late":"yes"},{"late":"yes"},invalid]"#,
        ".json",
    );
    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["--sorted", "false", "-n", "1"])
        .arg(file.path())
        .assert()
        .success()
        .stdout("late\nyes\nmore rows...\n")
        .stderr("");
}

#[cfg(feature = "saved-views")]
#[test]
fn sorted_json_preview_profiles_only_selected_rows() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(views.join("sort.yml"), "name: sort\nfilenames: ['*']\nsource: {}\nview:\n  sort:\n    - {column: /a, direction: desc, kind: numeric}\n").unwrap();
    let file = fixture(
        r#"[{"a":1,"other":"wide-value"},{"a":9,"winner":"yes"},{"a":2}]"#,
        ".json",
    );
    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args(["-n", "1"])
        .arg(file.path())
        .assert()
        .success()
        .stdout("a  winner\n9  yes\n2 more rows...\n")
        .stderr("");
}

#[test]
fn preview_stdin_exact_boundary_waits_for_eof() {
    use std::io::Write;
    use std::time::{Duration, Instant};
    for (format, input, expected) in [
        ("delimited", "A\n1\n2\n", "A\n1\n2\n"),
        ("ndjson", "{\"a\":1}\n{\"a\":2}\n", "a\n1\n2\n"),
    ] {
        let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("tview"))
            .env("XDG_CONFIG_HOME", tempfile::tempdir().unwrap().path())
            .args(["--format", format, "--sorted", "false", "-n", "2", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut pipe = child.stdin.take().unwrap();
        pipe.write_all(input.as_bytes()).unwrap();
        pipe.flush().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            child.try_wait().unwrap().is_none(),
            "premature {format} preview"
        );
        drop(pipe);
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("{format} preview did not finish at EOF");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
    }
}

#[test]
fn preview_stdin_lookahead_errors_leave_stdout_empty() {
    tview_command()
        .args(["--format", "ndjson", "--sorted", "false", "-n", "1", "-"])
        .write_stdin("{\"a\":1}\ninvalid\n")
        .assert()
        .code(1)
        .stdout("");
}

#[test]
fn preview_auto_preserves_two_member_json_records() {
    let input = r#"{"user":{"id":1},"meta":{"id":2}}"#;
    let file = fixture(input, ".json");
    let expected = tview_command()
        .args(["--format", "json", "--sorted", "false"])
        .arg(file.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    tview_command()
        .args(["--format", "json", "--sorted", "false", "-n", "1", "-"])
        .write_stdin(input)
        .assert()
        .success()
        .stdout(expected);
    for malformed in [
        r#"{"user":{"id":1},"meta":{"id":2},invalid}"#,
        r#"{"user":{"id":1},"meta":{"id":2},"third":invalid}"#,
    ] {
        let file = fixture(malformed, ".json");
        tview_command()
            .args(["--sorted", "false", "-n", "1"])
            .arg(file.path())
            .assert()
            .code(1)
            .stdout("");
        tview_command()
            .args(["--format", "json", "--sorted", "false", "-n", "1", "-"])
            .write_stdin(malformed)
            .assert()
            .code(1)
            .stdout("");
    }
}

#[test]
fn preview_stdin_detects_delimiter_after_multiline_header() {
    tview_command()
        .args(["--format", "delimited", "--sorted", "false", "-n", "1", "-"])
        .write_stdin("\"First\nName\";Value\nalpha;1\nbeta;2\n")
        .assert()
        .success()
        .stdout("First\\nName  Value\nalpha            1\nmore rows...\n");
}

#[cfg(feature = "saved-views")]
#[test]
fn preview_full_schema_limits_replayed_rows_for_late_filters() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(views.join("filtered.yml"), "name: filtered\nfilenames: ['*']\nsource: {}\nview:\n  filters:\n    - {column: /late, action: out, kind: text, condition: yes}\n").unwrap();
    let file = fixture(
        r#"[{"id":1},{"id":2},{"id":3},{"id":4,"late":"yes"},{"id":5,"late":"no"}]"#,
        ".json",
    );
    tview_command()
        .env("XDG_CONFIG_HOME", config.path())
        .args([
            "--view",
            "filtered",
            "--schema-scan",
            "full",
            "--sorted",
            "false",
            "-n",
            "1",
        ])
        .arg(file.path())
        .assert()
        .success()
        .stdout("id  late\n 1\n3 more rows...\n");
}

#[test]
fn preview_full_schema_preserves_mixed_object_shape() {
    let input = r#"{"a":{"id":1},"b":{"id":2},"c":{"id":3},"meta":true}"#;
    let file = fixture(input, ".json");
    let expected = tview_command()
        .args(["--sorted", "false", "--schema-scan", "full"])
        .arg(file.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    tview_command()
        .args(["--sorted", "false", "--schema-scan", "full", "-n", "1"])
        .arg(file.path())
        .assert()
        .success()
        .stdout(expected.clone());
    tview_command()
        .args([
            "--format",
            "json",
            "--sorted",
            "false",
            "--schema-scan",
            "full",
            "-n",
            "1",
            "-",
        ])
        .write_stdin(input)
        .assert()
        .success()
        .stdout(expected);
}

#[cfg(feature = "elasticsearch")]
#[test]
fn elasticsearch_preview_partial_result_has_unknown_remainder() {
    let _guard = elasticsearch_test_lock();
    let server = ElasticsearchServer::start(vec![ElasticsearchResponse::ok(
        r#"{"columns":[{"name":"level","type":"keyword"}],"values":[["error"],["warning"],["info"]],"is_partial":true}"#,
    )]);
    tview_command()
        .args([
            "--format",
            "elasticsearch",
            "--query",
            "FROM logs-* | KEEP level",
            "--sorted",
            "false",
            "-n",
            "1",
            server.endpoint(),
        ])
        .assert()
        .success()
        .stdout("level\nerror\nmore rows...\n")
        .stderr(predicate::str::contains("partial result"));
    assert_eq!(server.requests().len(), 1);
}

#[cfg(feature = "saved-views")]
#[test]
fn preview_source_limit_does_not_hide_first_data_row_errors() {
    let config = tempfile::tempdir().unwrap();
    let views = config.path().join("tview/views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(
        views.join("limited.yml"),
        "name: limited\nfilenames: ['*']\nsource:\n  limit: 1\nview: {}\n",
    )
    .unwrap();
    for (input, success) in [
        (b"Name,Value\n\xff,broken\n".as_slice(), false),
        (b"1,2\n\xff,broken\n".as_slice(), true),
    ] {
        let mut command = tview_command();
        command
            .env("XDG_CONFIG_HOME", config.path())
            .args([
                "--view",
                "limited",
                "--format",
                "delimited",
                "--encoding",
                "utf-8",
                "--sorted",
                "false",
                "-n",
                "1",
                "-",
            ])
            .write_stdin(input);
        if success {
            command.assert().success().stdout("       1         2\n");
        } else {
            command.assert().code(1).stdout("");
        }
    }
}
