#![cfg(feature = "saved-views")]

use assert_cmd::Command;

fn run_saved(
    config: &std::path::Path,
    input: &std::path::Path,
    name: &str,
    format: &str,
) -> (String, String) {
    let output = Command::cargo_bin("tview")
        .expect("tview binary")
        .env("XDG_CONFIG_HOME", config)
        .args(["--view", name, "-o", format])
        .arg(input)
        .output()
        .expect("output");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (
        String::from_utf8(output.stdout).expect("UTF-8 output"),
        String::from_utf8(output.stderr).expect("UTF-8 diagnostics"),
    )
}

#[test]
fn canonical_operations_bind_after_label_overrides_while_ambiguous_source_labels_do_not() {
    let root = tempfile::tempdir().expect("config root");
    let views = root.path().join("tview/views/bundle");
    std::fs::create_dir_all(&views).expect("view bundle");
    let input = root.path().join("records.json");
    std::fs::write(
        &input,
        r#"[{"left":{"id":1},"right":{"id":2},"name":"alpha"},{"left":{"id":3},"right":{"id":9},"name":"beta"}]"#,
    )
    .expect("JSON fixture");
    std::fs::write(
        views.join("canonical.yml"),
        "name: canonical\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    /left/id: {label: Left ID}\n    /right/id: {label: Right ID}\n  sort:\n    - {column: /right/id, direction: desc, kind: numeric}\n  filters:\n    - {column: /right/id, action: in, kind: numeric, condition: '>2'}\n",
    )
    .expect("canonical view");
    std::fs::write(
        views.join("ambiguous.yml"),
        "name: ambiguous\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    /left/id: {label: Left ID}\n    /right/id: {label: Right ID}\n  sort:\n    - {column: id, direction: desc, kind: numeric}\n  filters:\n    - {column: id, action: in, kind: numeric, condition: '>2'}\n",
    )
    .expect("ambiguous view");

    let (canonical, canonical_warnings) = run_saved(root.path(), &input, "canonical", "table");
    assert!(canonical.contains("Right ID"), "{canonical}");
    assert!(canonical.contains("beta"), "{canonical}");
    assert!(!canonical.contains("alpha"), "{canonical}");
    assert!(canonical_warnings.is_empty(), "{canonical_warnings}");

    let (ambiguous, warnings) = run_saved(root.path(), &input, "ambiguous", "table");
    let alpha = ambiguous.find("alpha").expect("first source row");
    let beta = ambiguous.find("beta").expect("second source row");
    assert!(
        alpha < beta,
        "ambiguous sort changed source order: {ambiguous}"
    );
    assert_eq!(
        warnings.matches("view.sort[0].column:").count(),
        1,
        "{warnings}"
    );
    assert_eq!(
        warnings.matches("view.filters[0].column:").count(),
        1,
        "{warnings}"
    );
}

#[test]
fn complete_schema_reports_missing_and_unavailable_filters_without_hiding_rows() {
    let root = tempfile::tempdir().expect("config root");
    let views = root.path().join("tview/views");
    std::fs::create_dir_all(&views).expect("views");
    let input = root.path().join("records.json");
    std::fs::write(&input, r#"[{"present":"words"},{"present":"more words"}]"#)
        .expect("JSON fixture");
    std::fs::write(
        views.join("warnings.yml"),
        "name: warnings\nfilenames: ['*']\nsource:\n  schema_scan: full\nview:\n  columns:\n    /absent: {label: Absent}\n  filters:\n    - {column: /absent, action: in, kind: text, condition: yes}\n    - {column: /present, action: in, kind: numeric, condition: '>2'}\n",
    )
    .expect("saved view");

    for format in ["table", "json", "jsonl"] {
        let (output, warnings) = run_saved(root.path(), &input, "warnings", format);
        assert!(output.contains("words"), "{format}: {output}");
        for field in [
            "view.columns./absent:",
            "view.filters[0].column:",
            "view.filters[1]:",
        ] {
            assert_eq!(warnings.matches(field).count(), 1, "{format}: {warnings}");
        }
        assert!(!output.contains("saved view:"), "{format}: {output}");
    }
}

#[test]
fn delimited_source_label_survives_override_for_numeric_filter_diagnostic() {
    let root = tempfile::tempdir().expect("config root");
    let views = root.path().join("tview/views");
    std::fs::create_dir_all(&views).expect("views");
    let input = root.path().join("records.csv");
    std::fs::write(&input, "count\nwords\nletters\n").expect("CSV fixture");
    std::fs::write(
        views.join("numeric.yml"),
        "name: numeric\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    count: {label: Total}\n  filters:\n    - {column: count, action: in, kind: numeric, condition: '>2'}\n",
    )
    .expect("saved view");

    let (output, warnings) = run_saved(root.path(), &input, "numeric", "json");
    let document: serde_json::Value = serde_json::from_str(&output).expect("JSON output");
    assert_eq!(
        document,
        serde_json::json!({"columns": ["Total"], "rows": [["words"], ["letters"]]})
    );
    assert_eq!(
        warnings.matches("view.filters[0]:").count(),
        1,
        "{warnings}"
    );
    assert!(!warnings.contains("view.filters[0].column:"), "{warnings}");
}

#[test]
fn delimited_saved_sort_keeps_original_source_label_after_override() {
    let root = tempfile::tempdir().expect("config root");
    let views = root.path().join("tview/views");
    std::fs::create_dir_all(&views).expect("views");
    let input = root.path().join("records.csv");
    std::fs::write(&input, "count\n10\n2\n1\n").expect("CSV fixture");
    std::fs::write(
        views.join("sorted.yml"),
        "name: sorted\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    count: {label: Total}\n  sort:\n    - {column: count, direction: asc, kind: numeric}\n",
    )
    .expect("saved view");

    let (output, warnings) = run_saved(root.path(), &input, "sorted", "json");
    let document: serde_json::Value = serde_json::from_str(&output).expect("JSON output");
    assert_eq!(
        document,
        serde_json::json!({"columns": ["Total"], "rows": [["1"], ["2"], ["10"]]})
    );
    assert!(warnings.is_empty(), "{warnings}");
}

#[test]
fn delimited_exact_and_wildcard_specificity_preserves_labels_and_warns_once_for_unmatched() {
    let root = tempfile::tempdir().expect("config root");
    let views = root.path().join("tview/views");
    std::fs::create_dir_all(&views).expect("views");
    let input = root.path().join("records.csv");
    std::fs::write(&input, "Count,docs_count,store_count\n1,2,3\n").expect("CSV fixture");
    std::fs::write(
        views.join("wildcard.yml"),
        "name: wildcard\nfilenames: ['*']\nsource: {}\nview:\n  columns:\n    count: {label: Total}\n    '*count': {label: Metric}\n    'docs_count*': {label: Documents}\n    '*ount': {label: Ignored}\n    '*absent*': {label: Missing}\n",
    )
    .expect("saved view");

    let (output, warnings) = run_saved(root.path(), &input, "wildcard", "json");
    let document: serde_json::Value = serde_json::from_str(&output).expect("JSON output");
    assert_eq!(
        document,
        serde_json::json!({"columns": ["Total", "Documents", "Metric"], "rows": [["1", "2", "3"]]})
    );
    assert_eq!(
        warnings.matches("view.columns.*absent*:").count(),
        1,
        "{warnings}"
    );
    assert_eq!(
        warnings.matches("view.columns.*ount:").count(),
        1,
        "{warnings}"
    );
    assert!(!warnings.contains("view.columns.*count:"), "{warnings}");
    assert!(!warnings.contains("view.columns.count:"), "{warnings}");
}

#[test]
fn missing_forced_view_fails_before_attempting_to_open_input() {
    let root = tempfile::tempdir().expect("config root");
    let input = root.path().join("nonexistent.csv");
    let output = Command::cargo_bin("tview")
        .expect("tview binary")
        .env("XDG_CONFIG_HOME", root.path())
        .args(["--view", "absent", "-o", "table"])
        .arg(input)
        .output()
        .expect("output");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("saved view 'absent'"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn disabled_view_ignores_malformed_files_and_retains_default_presentation() {
    let root = tempfile::tempdir().expect("config root");
    let views = root.path().join("tview/views");
    std::fs::create_dir_all(&views).expect("views");
    std::fs::write(views.join("invalid.yml"), "name: [").expect("malformed YAML");
    let input = root.path().join("data.csv");
    std::fs::write(&input, "Name\nalpha\n").expect("CSV fixture");

    let output = Command::cargo_bin("tview")
        .expect("tview binary")
        .env("XDG_CONFIG_HOME", root.path())
        .args(["--no-view", "-o", "table"])
        .arg(input)
        .output()
        .expect("output");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("alpha"));
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!views.join("data.yml").exists());
}

#[test]
fn absent_structured_wildcard_reference_retains_first_header_compatibility() {
    let root = tempfile::tempdir().expect("config root");
    let views = root.path().join("tview/views");
    std::fs::create_dir_all(&views).expect("views");
    let input = root.path().join("records.json");
    std::fs::write(
        &input,
        r#"[{"first_count":"keep","second_count":"drop"},{"first_count":"drop","second_count":"keep"}]"#,
    )
    .expect("JSON fixture");
    std::fs::write(
        views.join("compatible.yml"),
        "name: compatible\nfilenames: ['*']\nsource: {}\nview:\n  filters:\n    - {column: '*count', action: in, kind: text, condition: keep}\n",
    )
    .expect("saved view");

    let (output, warnings) = run_saved(root.path(), &input, "compatible", "json");
    let result: serde_json::Value = serde_json::from_str(&output).expect("JSON output");
    assert_eq!(result["rows"], serde_json::json!([["keep", "drop"]]));
    assert!(warnings.is_empty(), "{warnings}");
}
