use assert_cmd::Command;
use serde_json::{json, Value};
use std::path::Path;
use tview::ingest::{open_source, source::InputSource, InputFormat, OpenOptions};
use tview::table::{CellValue, ColumnSourceIdentity, RowIndex};

fn command(config: &Path) -> Command {
    let mut command = Command::cargo_bin("tview").expect("binary");
    command.env("XDG_CONFIG_HOME", config);
    command
}

fn document(output: &[u8]) -> Value {
    serde_json::from_slice(output).expect("JSON output")
}

#[test]
fn selected_toon_table_preserves_nested_header_context() {
    let root = tempfile::tempdir().expect("test directory");
    let output = command(root.path())
        .args([
            "--format",
            "toon",
            "--json-path",
            "/forecast",
            "--output",
            "json",
            "-",
        ])
        .write_stdin("location:\n  city: Berlin\nforecast[1]{day,temp{min,max}}:\n  Mon,-2,4\n")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        document(&output),
        json!({"columns": ["day", "temp.min", "temp.max"], "rows": [["Mon", "-2", "4"]]})
    );
}

#[test]
fn explicit_delimited_options_override_toon_extension_under_auto() {
    let root = tempfile::tempdir().expect("test directory");
    let path = root.path().join("records.toon");
    std::fs::write(&path, "id|name\n1|Ada\n").expect("write delimited input");
    let output = command(root.path())
        .args(["--format", "auto", "--delimiter", "|", "--output", "json"])
        .arg(&path)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        document(&output),
        json!({"columns": ["id", "name"], "rows": [["1", "Ada"]]})
    );
}

#[test]
fn later_toon_columns_keep_nested_context_after_bounded_schema_scan() {
    let root = tempfile::tempdir().expect("test directory");
    let path = root.path().join("forecast.toon");
    std::fs::write(
        &path,
        "[2]:\n  - temp:\n      min: -2\n  - temp:\n      max: 4\n",
    )
    .expect("write TOON");
    let mut table = open_source(
        InputSource::Path(path),
        &OpenOptions {
            schema_scan_bytes: 1,
            ..OpenOptions::default()
        },
    )
    .expect("open TOON")
    .into_implicit_table()
    .expect("table");
    assert_eq!(table.definition.columns[0].display_name, "temp.min");
    let progress = table
        .store
        .ensure_indexed_through(RowIndex(1))
        .expect("discover later fields");
    table
        .definition
        .apply_delta(progress.schema_delta)
        .expect("apply schema");
    let labels: Vec<_> = table
        .definition
        .columns
        .iter()
        .map(|column| column.display_name.as_str())
        .collect();
    assert_eq!(labels, ["temp.min", "temp.max"]);
    assert_eq!(
        table
            .store
            .row(RowIndex(0))
            .expect("first row")
            .expect("present")
            .cells,
        [CellValue::Integer(-2), CellValue::Null]
    );
    assert_eq!(
        table
            .store
            .row(RowIndex(1))
            .expect("second row")
            .expect("present")
            .cells,
        [CellValue::Null, CellValue::Integer(4)]
    );
}

#[test]
fn later_toon_name_field_does_not_rename_or_collide_with_entry_key() {
    let root = tempfile::tempdir().expect("test directory");
    let path = root.path().join("entries.toon");
    std::fs::write(&path, "alpha:\n  id: 1\nbeta:\n  name: Berlin\n").expect("write TOON");
    let mut table = open_source(
        InputSource::Path(path),
        &OpenOptions {
            object_mode: tview::ingest::ObjectMode::Entries,
            schema_scan_bytes: 1,
            ..OpenOptions::default()
        },
    )
    .expect("open entries")
    .into_implicit_table()
    .expect("table");
    assert_eq!(table.definition.columns[0].display_name, "name");
    let progress = table
        .store
        .ensure_indexed_through(RowIndex(1))
        .expect("discover name field");
    table
        .definition
        .apply_delta(progress.schema_delta)
        .expect("apply schema");
    let labels: Vec<_> = table
        .definition
        .columns
        .iter()
        .map(|column| column.display_name.as_str())
        .collect();
    assert_eq!(labels, ["name", "id", "/name"]);
    assert_eq!(
        table.definition.columns[0].source_identity,
        ColumnSourceIdentity::ObjectKey
    );
    assert_eq!(
        table.definition.columns[2].source_identity,
        ColumnSourceIdentity::StructuredPath("/name".parse().expect("path"))
    );
    assert_eq!(
        table
            .store
            .row(RowIndex(1))
            .expect("second row")
            .expect("present")
            .cells,
        [
            CellValue::Text("beta".into()),
            CellValue::Null,
            CellValue::Text("Berlin".into())
        ]
    );
}

#[test]
fn toon_extension_decodes_nested_tabular_cells_with_comments_bom_and_crlf() {
    let root = tempfile::tempdir().expect("test directory");
    let path = root.path().join("people.ToOn");
    std::fs::write(
        &path,
        "\u{feff}# comment before root\r\n[2]{id,person{name,enabled},score}:\r\n  1,Ada,true,3.5\r\n  # comment between rows\r\n  2,Bob,false,null\r\n",
    )
    .expect("write TOON");

    let output = command(root.path())
        .args(["--output", "json"])
        .arg(&path)
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        document(&output),
        json!({
            "columns": ["id", "person.name", "person.enabled", "score"],
            "rows": [["1", "Ada", "true", "3.5"], ["2", "Bob", "false", ""]]
        })
    );

    let mut table = open_source(
        InputSource::Path(path),
        &OpenOptions {
            format: InputFormat::Auto,
            ..OpenOptions::default()
        },
    )
    .expect("open TOON source")
    .into_implicit_table()
    .expect("TOON table");
    let paths: Vec<_> = table
        .definition
        .columns
        .iter()
        .map(|column| match &column.source_identity {
            ColumnSourceIdentity::StructuredPath(path) => path.as_str(),
            other => panic!("expected structured source path, got {other:?}"),
        })
        .collect();
    assert_eq!(paths, ["/id", "/person/name", "/person/enabled", "/score"]);
    assert_eq!(
        table
            .store
            .row(RowIndex(0))
            .expect("first row")
            .expect("present")
            .cells,
        [
            CellValue::Integer(1),
            CellValue::Text("Ada".into()),
            CellValue::Boolean(true),
            CellValue::Float(3.5),
        ]
    );
    assert_eq!(
        table
            .store
            .row(RowIndex(1))
            .expect("second row")
            .expect("present")
            .cells,
        [
            CellValue::Integer(2),
            CellValue::Text("Bob".into()),
            CellValue::Boolean(false),
            CellValue::Null,
        ]
    );
}

#[test]
fn explicit_stdin_selects_nested_keyed_entries_and_record_mode() {
    let root = tempfile::tempdir().expect("test directory");
    let input = "payload[2:]{id,enabled}:\n  alpha: 1,true\n  beta: 2,false\n";
    let output = command(root.path())
        .args([
            "--format",
            "toon",
            "--json-path",
            "/payload",
            "--object-mode",
            "entries",
            "-o",
            "json",
            "-",
        ])
        .write_stdin(input)
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        document(&output),
        json!({"columns": ["name", "id", "enabled"], "rows": [["alpha", "1", "true"], ["beta", "2", "false"]]})
    );
    let automatic = command(root.path())
        .args([
            "--format",
            "toon",
            "--json-path",
            "/payload",
            "-o",
            "json",
            "-",
        ])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let output = command(root.path())
        .args([
            "--format",
            "toon",
            "--json-path",
            "/payload",
            "--object-mode",
            "record",
            "-o",
            "json",
            "-",
        ])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        document(&output),
        json!({"columns": ["alpha.id", "alpha.enabled", "beta.id", "beta.enabled"], "rows": [["1", "true", "2", "false"]]})
    );
    // Two members do not satisfy the shared automatic keyed-object heuristic.
    assert_eq!(document(&automatic), document(&output));
}

#[test]
fn plain_stdin_remains_delimited_without_explicit_toon_format() {
    let root = tempfile::tempdir().expect("test directory");
    let output = command(root.path())
        .args(["--output", "json", "-"])
        .write_stdin("id,name\n1,Ada\n")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        document(&output),
        json!({"columns": ["id", "name"], "rows": [["1", "Ada"]]})
    );
}

#[cfg(feature = "saved-views")]
#[test]
fn saved_view_restores_toon_format_pointer_and_entry_key_binding() {
    let root = tempfile::tempdir().expect("test directory");
    let views = root.path().join("tview/views");
    std::fs::create_dir_all(&views).expect("saved views directory");
    std::fs::write(
        views.join("keyed.yml"),
        "name: keyed\nfilenames: ['*']\nsource:\n  format: toon\n  json_path: /payload\n  object_mode: entries\nview:\n  columns:\n    '@key': {label: Key}\n",
    )
    .expect("saved view");
    let path = root.path().join("input.data");
    std::fs::write(&path, "payload[2:]{id}:\n  alpha: 1\n  beta: 2\n").expect("TOON input");
    let output = command(root.path())
        .args(["--view", "keyed", "-o", "json"])
        .arg(&path)
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        document(&output),
        json!({"columns": ["Key", "id"], "rows": [["alpha", "1"], ["beta", "2"]]})
    );
}

#[cfg(feature = "saved-views")]
#[test]
fn toon_source_filter_precedes_saved_source_limit() {
    let root = tempfile::tempdir().expect("test directory");
    let views = root.path().join("tview/views");
    std::fs::create_dir_all(&views).expect("saved views directory");
    std::fs::write(
        views.join("filtered.yml"),
        "name: filtered\nfilenames: ['*']\nsource:\n  format: toon\n  limit: 1\n  filters:\n    - {column: /keep, operator: equal, value: true}\nview: {}\n",
    )
    .expect("saved view");
    let path = root.path().join("input.toon");
    std::fs::write(
        &path,
        "[4]{id,keep}:\n  1,false\n  2,true\n  3,false\n  4,true\n",
    )
    .expect("TOON input");
    let output = command(root.path())
        .args(["--view", "filtered", "-o", "json"])
        .arg(&path)
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        document(&output),
        json!({"columns": ["id", "keep"], "rows": [["2", "true"]]})
    );
}

#[test]
fn strict_toon_errors_leave_serialized_output_empty() {
    let root = tempfile::tempdir().expect("test directory");
    for (input, reason) in [
        ("[2]{id}:\n  1\n".as_bytes(), "tabular row count mismatch"),
        ("[1]{id,enabled}:\n  1\n".as_bytes(), "row width mismatch"),
        ("id: 1\nid: 2\n".as_bytes(), "duplicate key"),
        (b"id: \xff\n".as_slice(), "invalid UTF-8"),
        ("x: 1e999\n".as_bytes(), "nonfinite numeric overflow"),
    ] {
        let output = command(root.path())
            .args(["--format", "toon", "-o", "json", "-"])
            .write_stdin(input)
            .assert()
            .code(1)
            .get_output()
            .clone();
        assert!(output.stdout.is_empty(), "{reason}: unexpected stdout");
    }
}

#[test]
fn explicit_toon_rejects_delimited_and_native_relation_options() {
    let root = tempfile::tempdir().expect("test directory");
    for (flag, value) in [
        ("--delimiter", ","),
        ("--encoding", "utf-8"),
        ("--quoting", "QUOTE_NONE"),
        ("--quote-char", "'"),
    ] {
        command(root.path())
            .args(["--format", "toon", flag, value, "-"])
            .write_stdin("[1]{id}:\n  1\n")
            .assert()
            .failure()
            .stdout("");
    }
    #[cfg(any(feature = "sqlite", feature = "elasticsearch"))]
    for (flag, value) in [("--table", "records"), ("--query", "SELECT * FROM records")] {
        command(root.path())
            .args(["--format", "toon", flag, value, "-"])
            .write_stdin("[1]{id}:\n  1\n")
            .assert()
            .failure()
            .stdout("");
    }
}

#[test]
fn automatic_keyed_detection_finishes_entry_crossing_byte_budget() {
    let root = tempfile::tempdir().expect("test directory");
    let large_value = "x".repeat(1024 * 1024);
    let input = format!("alpha:\n  value: {large_value}\nbeta:\n  value: b\ngamma:\n  value: c\n");
    let output = command(root.path())
        .args(["--format", "toon", "--output", "json", "-"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        document(&output),
        json!({
            "columns": ["alpha.value", "beta.value", "gamma.value"],
            "rows": [[large_value, "b", "c"]]
        })
    );
}
