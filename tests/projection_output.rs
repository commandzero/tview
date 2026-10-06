use assert_cmd::Command;
use std::io::{BufRead, Cursor};

fn fixture(contents: &str, suffix: &str) -> tempfile::NamedTempFile {
    let file = tempfile::Builder::new().suffix(suffix).tempfile().unwrap();
    std::fs::write(file.path(), contents).unwrap();
    file
}

fn command() -> Command {
    let mut command = Command::cargo_bin("tview").unwrap();
    command.env(
        "XDG_CONFIG_HOME",
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-empty-config"),
    );
    command
}

#[cfg(feature = "saved-views")]
#[test]
fn full_scan_includes_accepted_explicit_null_and_empty_but_not_rejected_fields() {
    let directory = tempfile::tempdir().unwrap();
    let view_dir = directory.path().join("tview/views");
    std::fs::create_dir_all(&view_dir).unwrap();
    std::fs::write(
        view_dir.join("accepted.yml"),
        "name: accepted\nfilenames: ['*']\nsource: {}\nview:\n  filters:\n    - {column: /keep, action: in, kind: text, condition: yes}\n",
    )
    .unwrap();
    let file = fixture(
        r#"[{"keep":"yes","explicit":null},{"keep":"no","rejected":"x"},{"keep":"yes","empty":""}]"#,
        ".json",
    );
    let output = Command::cargo_bin("tview")
        .unwrap()
        .env("XDG_CONFIG_HOME", directory.path())
        .args([
            "--view",
            "accepted",
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
        .get_output()
        .stdout
        .clone();
    let lines = String::from_utf8(output).unwrap();
    assert!(lines.lines().next().unwrap().contains("explicit"));
    assert!(lines.lines().next().unwrap().contains("empty"));
    assert!(!lines.contains("rejected"));
    assert!(lines.ends_with("1 more rows...\n"));
}

#[cfg(feature = "saved-views")]
#[test]
fn complete_filtered_schema_keeps_rejected_source_fields_but_prefix_does_not() {
    let directory = tempfile::tempdir().unwrap();
    let view_dir = directory.path().join("tview/views");
    std::fs::create_dir_all(&view_dir).unwrap();
    std::fs::write(
        view_dir.join("accepted.yml"),
        "name: accepted\nfilenames: ['*']\nsource: {}\nview:\n  filters:\n    - {column: /keep, action: in, kind: text, condition: yes}\n",
    )
    .unwrap();
    let file = fixture(
        r#"[{"keep":"yes","id":1},{"keep":"no","rejected":"discard"}]"#,
        ".json",
    );
    Command::cargo_bin("tview")
        .unwrap()
        .env("XDG_CONFIG_HOME", directory.path())
        .args([
            "--view",
            "accepted",
            "--sorted",
            "false",
            "--schema-scan",
            "full",
            "-n",
            "1",
        ])
        .arg(file.path())
        .assert()
        .success()
        .stdout("keep  id\nyes    1\n");
    let stdout = Command::cargo_bin("tview")
        .unwrap()
        .env("XDG_CONFIG_HOME", directory.path())
        .args(["--view", "accepted", "--output", "jsonl"])
        .arg(file.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let complete: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(
        complete["columns"],
        serde_json::json!(["keep", "id", "rejected"])
    );
}

#[cfg(feature = "saved-views")]
#[test]
fn type_sorts_use_saved_numeric_metadata_or_completed_source_types() {
    let directory = tempfile::tempdir().unwrap();
    let view_dir = directory.path().join("tview/views");
    std::fs::create_dir_all(&view_dir).unwrap();
    let cases = [
        (
            "score\n10\n2\n",
            ".csv",
            "  columns:\n    score: {type: number}\n",
        ),
        (r#"[{"score":10},{"score":2}]"#, ".json", ""),
    ];
    for (contents, suffix, columns) in cases {
        std::fs::write(
            view_dir.join("typed.yml"),
            format!(
                "name: typed\nfilenames: ['*']\nsource: {{}}\nview:\n{columns}  sort:\n    - {{column: score, direction: asc, kind: type}}\n"
            ),
        )
        .unwrap();
        let source = fixture(contents, suffix);
        Command::cargo_bin("tview")
            .unwrap()
            .env("XDG_CONFIG_HOME", directory.path())
            .args(["--view", "typed", "-n", "1"])
            .arg(source.path())
            .assert()
            .success()
            .stdout("score\n    2\n1 more rows...\n");
    }
}

#[cfg(feature = "saved-views")]
#[test]
fn saved_rendered_filter_retains_raw_source_identity() {
    let directory = tempfile::tempdir().unwrap();
    let view_dir = directory.path().join("tview/views");
    std::fs::create_dir_all(&view_dir).unwrap();
    std::fs::write(
        view_dir.join("formatted.yml"),
        "name: formatted\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    Word: {format: uppercase}\n  filters:\n    - {column: Word, action: in, kind: text, condition: YES}\n",
    )
    .unwrap();
    let source = fixture("Word\nyes\nno\n", ".csv");
    Command::cargo_bin("tview")
        .unwrap()
        .env("XDG_CONFIG_HOME", directory.path())
        .args(["--view", "formatted", "--sorted", "false", "-n", "1"])
        .arg(source.path())
        .assert()
        .success()
        .stdout("Word\nYES\n");
}

#[test]
fn complete_structured_output_ignores_start_position() {
    let source = fixture(
        "{\"id\":1}\n{\"id\":2,\"late\":null}\n{\"id\":3,\"late\":\"\"}\n",
        ".ndjson",
    );
    let result = command()
        .args([
            "--format",
            "ndjson",
            "--output",
            "jsonl",
            "--start_pos",
            "2,2",
        ])
        .arg(source.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let records: Vec<serde_json::Value> = Cursor::new(result)
        .lines()
        .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
        .collect();
    assert_eq!(records.len(), 3);
    for record in &records {
        assert_eq!(record["columns"], serde_json::json!(["id", "late"]));
    }
    assert_eq!(records[0]["values"], serde_json::json!(["1", ""]));
    assert_eq!(records[1]["values"], serde_json::json!(["2", ""]));
    assert_eq!(records[2]["values"], serde_json::json!(["3", ""]));
}

#[test]
fn lookahead_only_explicit_fields_do_not_change_default_schema() {
    let source = fixture(
        r#"[{"id":1,"empty":""},{"id":2,"lookahead":null},{"id":3,"later":1}]"#,
        ".json",
    );
    command()
        .args(["--sorted", "false", "-n", "1"])
        .arg(source.path())
        .assert()
        .success()
        .stdout("id  empty\n 1\nmore rows...\n");
}

#[test]
fn required_matching_lookahead_error_never_publishes_partial_rows() {
    let source = fixture("{\"id\":1}\n{\"id\":2}\ninvalid\n", ".ndjson");
    command()
        .args(["--format", "ndjson", "--sorted", "false", "-n", "2"])
        .arg(source.path())
        .assert()
        .failure()
        .stdout("");
}

#[cfg(feature = "saved-views")]
#[test]
fn complete_filtered_colors_use_resident_profile_but_preview_uses_emitted_rows() {
    let directory = tempfile::tempdir().unwrap();
    let view_dir = directory.path().join("tview/views");
    std::fs::create_dir_all(&view_dir).unwrap();
    let color_column = "  columns:\n    a:\n      type: number\n      colors:\n        - gradient: {mode: auto, steps: 4, colors: ['#000000FF', '#FFFFFFFF']}\n";
    std::fs::write(
        view_dir.join("unfiltered.yml"),
        format!("name: unfiltered\nfilenames: ['*']\nsource: {{}}\nview:\n{color_column}"),
    )
    .unwrap();
    std::fs::write(
        view_dir.join("filtered.yml"),
        format!(
            "name: filtered\nfilenames: ['*']\nsource: {{}}\nview:\n{color_column}  filters:\n    - {{column: a, action: in, kind: numeric, condition: '< 100'}}\n"
        ),
    )
    .unwrap();
    for source in [
        fixture("a\n0\n5\n10\n100\n", ".csv"),
        fixture(r#"[{"a":0},{"a":5},{"a":10},{"a":100}]"#, ".json"),
    ] {
        let colored = |view: &str, preview: bool| {
            let mut command = Command::cargo_bin("tview").unwrap();
            command
                .env("XDG_CONFIG_HOME", directory.path())
                .args(["--view", view, "--color", "always", "--width", "2"]);
            if preview {
                command.args(["--sorted", "false", "-n", "2"]);
            } else {
                command.args(["--output", "table"]);
            }
            String::from_utf8(
                command
                    .arg(source.path())
                    .assert()
                    .success()
                    .get_output()
                    .stdout
                    .clone(),
            )
            .unwrap()
        };
        let complete = colored("filtered", false);
        let source_domain = colored("unfiltered", false);
        let preview = colored("filtered", true);
        fn middle(text: &str) -> &str {
            text.lines().nth(2).unwrap()
        }
        assert_ne!(middle(&complete), middle(&source_domain));
        assert_ne!(middle(&complete), middle(&preview));
    }
}

#[cfg(feature = "saved-views")]
#[test]
fn saved_content_width_is_resolved_from_emitted_rows_only() {
    let directory = tempfile::tempdir().unwrap();
    let view_dir = directory.path().join("tview/views");
    std::fs::create_dir_all(&view_dir).unwrap();
    std::fs::write(
        view_dir.join("width.yml"),
        "name: width\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    field: {width: content}\n",
    )
    .unwrap();
    let source = fixture("field,tag\na,x\nbbbbb,y\nlonglonglong,z\n", ".csv");
    let preview = Command::cargo_bin("tview")
        .unwrap()
        .env("XDG_CONFIG_HOME", directory.path())
        .args(["--view", "width", "--sorted", "false", "-n", "1"])
        .arg(source.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let preview = String::from_utf8(preview).unwrap();
    let mut lines = preview.lines();
    assert_eq!(lines.next(), Some("f  tag"));
    assert_eq!(lines.next(), Some("a  x"));
    assert!(preview.ends_with("more rows...\n"));

    let complete = Command::cargo_bin("tview")
        .unwrap()
        .env("XDG_CONFIG_HOME", directory.path())
        .args(["--view", "width", "--output", "table"])
        .arg(source.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let complete = String::from_utf8(complete).unwrap();
    assert!(complete.lines().next().unwrap().starts_with("field       "));
    assert!(complete.contains("longlonglong  z\n"));
}

#[test]
fn table_clips_unicode_width_and_escapes_controls_while_json_keeps_raw_text() {
    let source = fixture("Field,Second\n\"a界b\",\"x\ny\"\n", ".csv");
    command()
        .args(["--output", "table", "--width", "3"])
        .arg(source.path())
        .assert()
        .success()
        .stdout("Fie  Sec\na界  x\\n\n");
    let result = command()
        .args(["--output", "json"])
        .arg(source.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&result).unwrap(),
        serde_json::json!({"columns": ["Field", "Second"], "rows": [["a界b", "x\ny"]]}),
    );
}

#[cfg(feature = "saved-views")]
#[test]
fn complete_empty_view_filtered_results_emit_no_columns() {
    let directory = tempfile::tempdir().unwrap();
    let view_dir = directory.path().join("tview/views");
    std::fs::create_dir_all(&view_dir).unwrap();
    std::fs::write(
        view_dir.join("empty.yml"),
        "name: empty\nfilenames: ['*']\nsource: {}\nview:\n  filters:\n    - {column: keep, action: in, kind: text, condition: yes}\n",
    )
    .unwrap();
    for (source, suffix) in [
        ("keep,other\nno,x\n", ".csv"),
        (r#"[{"keep":"no","other":"x"}]"#, ".json"),
    ] {
        let source = fixture(source, suffix);
        let output = Command::cargo_bin("tview")
            .unwrap()
            .env("XDG_CONFIG_HOME", directory.path())
            .args(["--view", "empty", "--output", "json"])
            .arg(source.path())
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output).unwrap(),
            serde_json::json!({"columns": [], "rows": []}),
        );
    }
}

#[cfg(feature = "saved-views")]
#[test]
fn invalid_numeric_filter_does_not_force_unread_json_suffix() {
    let directory = tempfile::tempdir().unwrap();
    let view_dir = directory.path().join("tview/views");
    std::fs::create_dir_all(&view_dir).unwrap();
    std::fs::write(
        view_dir.join("invalid.yml"),
        "name: invalid\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    /value: {type: number}\n  filters:\n    - {column: /value, action: in, kind: numeric, condition: not-a-number}\n",
    )
    .unwrap();
    let mut source = String::from("[{\"value\":1},{\"value\":2},{\"value\":3},");
    for _ in 0..20_000 {
        source.push_str("{\"value\":4},");
    }
    source.push_str("{broken]");
    let source = fixture(&source, ".json");
    let output = Command::cargo_bin("tview")
        .unwrap()
        .env("XDG_CONFIG_HOME", directory.path())
        .args(["--view", "invalid", "--color", "never", "-n", "2"])
        .arg(source.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        String::from_utf8(output).unwrap(),
        "value\n    1\n    2\nmore rows...\n"
    );
}

#[cfg(feature = "saved-views")]
#[test]
fn explicit_number_type_does_not_enable_numeric_filter_for_mixed_column() {
    let directory = tempfile::tempdir().unwrap();
    let view_dir = directory.path().join("tview/views");
    std::fs::create_dir_all(&view_dir).unwrap();
    std::fs::write(
        view_dir.join("mixed.yml"),
        "name: mixed\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    value: {type: number}\n  filters:\n    - {column: value, action: in, kind: numeric, condition: '> 1'}\n",
    )
    .unwrap();
    let source = fixture("value\n1\nbad\n3\n", ".csv");
    let output = Command::cargo_bin("tview")
        .unwrap()
        .env("XDG_CONFIG_HOME", directory.path())
        .args(["--view", "mixed", "--output", "json"])
        .arg(source.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output).unwrap()["rows"],
        serde_json::json!([["1"], ["bad"], ["3"]]),
    );
}

#[cfg(feature = "saved-views")]
#[test]
fn deferred_filter_replay_does_not_claim_an_undercounted_exact_remainder() {
    let directory = tempfile::tempdir().unwrap();
    let view_dir = directory.path().join("tview/views");
    std::fs::create_dir_all(&view_dir).unwrap();
    std::fs::write(
        view_dir.join("deferred.yml"),
        "name: deferred\nfilenames: []\nsource: {}\nview:\n  filters:\n    - {column: /a, action: in, kind: text, condition: '1'}\n    - {column: /missing, action: in, kind: text, condition: x}\n",
    ).unwrap();
    let source = fixture(r#"[{"a":1},{"a":2},{"a":1},{"a":1}]"#, ".json");
    Command::cargo_bin("tview")
        .unwrap()
        .env("XDG_CONFIG_HOME", directory.path())
        .args(["--view", "deferred", "--sorted", "false", "-n", "1"])
        .arg(source.path())
        .assert()
        .success()
        .stdout("a\n1\nmore rows...\n");
}

#[cfg(feature = "saved-views")]
#[test]
fn explicit_cli_width_overrides_saved_content_width() {
    let directory = tempfile::tempdir().unwrap();
    let view_dir = directory.path().join("tview/views");
    std::fs::create_dir_all(&view_dir).unwrap();
    std::fs::write(
        view_dir.join("width.yml"),
        "name: width\nfilenames: []\nsource: {}\nview:\n  columns:\n    field: {width: content}\n",
    )
    .unwrap();
    let source = fixture("field,tag\naaa,x\nbbbbb,y\n", ".csv");
    Command::cargo_bin("tview")
        .unwrap()
        .env("XDG_CONFIG_HOME", directory.path())
        .args(["--view", "width", "--width", "2"])
        .arg(source.path())
        .assert()
        .success()
        .stdout("fi  ta\naa  x\nbb  y\n");
}

#[cfg(feature = "saved-views")]
#[test]
fn conditional_foreground_resets_before_the_next_cell_and_newline() {
    let directory = tempfile::tempdir().unwrap();
    let view_dir = directory.path().join("tview/views");
    std::fs::create_dir_all(&view_dir).unwrap();
    std::fs::write(view_dir.join("red.yml"),
        "name: red\nfilenames: []\nsource: {}\nview:\n  columns:\n    value:\n      colors:\n        - match: {yes: '#FF0000FF'}\n",
    ).unwrap();
    let source = fixture("value,tag\nyes,no\n", ".csv");
    let output = Command::cargo_bin("tview")
        .unwrap()
        .env("XDG_CONFIG_HOME", directory.path())
        .env("COLORTERM", "truecolor")
        .env("TERM", "xterm-256color")
        .env_remove("NO_COLOR")
        .env_remove("FORCE_COLOR")
        .args([
            "--view", "red", "--output", "table", "--color", "always", "--width", "3",
        ])
        .arg(source.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();
    let row = text.lines().nth(1).unwrap();
    let (red, following) = row
        .split_once("\x1b[0m  ")
        .expect("reset before uncolored gap");
    assert_eq!(red, "\x1b[38;2;255;0;0myes");
    assert!(!following.contains("38;2;255;0;0"));
    assert!(following.ends_with("\x1b[0m"));
}
