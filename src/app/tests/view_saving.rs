use super::*;
use crate::app::view_file::SavedViewYaml;

#[test]
fn saved_view_modal_displays_placeholder_and_saves_immediately() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("views").join("cat_shards.yml");
    let mut app = app_with_saved_view_target(rows(&[&["Name"], &["alpha"]]), target.clone());

    app.open_saved_view_modal();
    let modal = app.view_modal.as_ref().expect("modal");
    assert!(modal.filename.contains("cat_shards.yml"));
    assert!(modal.yaml.contains("name: cat_shards"));
    assert!(modal.yaml.contains("filenames:\n  - cat_shards.txt"));

    app.handle_saved_view_key(key(KeyCode::Char('s')));

    let saved = std::fs::read_to_string(&target).expect("saved file");
    assert!(saved.contains("name: cat_shards"));
    assert!(app
        .message
        .as_deref()
        .is_some_and(|message| message.contains("saved view")));
}

#[test]
fn saved_view_modal_confirms_and_declines_overwrite() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("cat_shards.yml");
    std::fs::write(
        &target,
        concat!(
            "# keep me\n",
            "# describe this view\n",
            "name: old # view name\n",
            "filenames: # filename patterns\n",
            "  - old # first filename\n",
        ),
    )
    .expect("write old");
    let mut app = app_with_saved_view_target(rows(&[&["Name"], &["alpha"]]), target.clone());
    app.open_saved_view_modal();

    app.handle_saved_view_key(key(KeyCode::Char('s')));
    assert!(app.view_modal.as_ref().expect("modal").confirming_overwrite);
    app.handle_saved_view_key(key(KeyCode::Char('n')));
    assert!(!app.view_modal.as_ref().expect("modal").confirming_overwrite);
    assert!(std::fs::read_to_string(&target)
        .expect("old")
        .contains("name: old"));

    app.handle_saved_view_key(key(KeyCode::Char('s')));
    app.handle_saved_view_key(key(KeyCode::Char('y')));
    let saved = std::fs::read_to_string(&target).expect("saved file");
    assert!(saved.starts_with("# keep me\n# describe this view\n"));
    assert!(saved.contains("name: cat_shards # view name"));
    assert!(saved.contains("filenames: # filename patterns"));
    assert!(
        saved.contains("  - cat_shards.txt # first filename"),
        "{saved}"
    );
}

#[test]
fn saved_view_comment_merge_keeps_header_and_inline_comments() {
    let existing = "# header\nname: old # view name\nfilenames: # filename patterns\n  - old # first filename\n";
    let yaml = "name: cat_shards\nfilenames:\n  - cat_shards.txt\n";

    let merged = SavedViewYaml(yaml).with_comments_from(SavedViewYaml(existing));

    assert!(merged.starts_with("# header\n"));
    assert!(merged.contains("name: cat_shards # view name"));
    assert!(merged.contains("filenames: # filename patterns"));
    assert!(
        merged.contains("  - cat_shards.txt # first filename"),
        "{merged}"
    );
}

#[test]
fn saved_view_modal_scrolls_and_reports_save_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().to_path_buf();
    let mut app = app_with_saved_view_target(
        rows(&[&["A", "B", "C"], &["1", "2", "3"], &["4", "5", "6"]]),
        target,
    );
    app.open_saved_view_modal();

    app.handle_saved_view_key(key(KeyCode::Char('j')));
    assert_eq!(app.view_modal.as_ref().expect("modal").scroll, 1);

    app.handle_saved_view_key(key(KeyCode::Char('s')));
    assert!(app.view_modal.as_ref().expect("modal").confirming_overwrite);
    app.handle_saved_view_key(key(KeyCode::Char('y')));

    assert_eq!(app.popup, Some(ui::Popup::SavedView));
    assert!(app
        .message
        .as_deref()
        .is_some_and(|message| message.contains("failed to save view")));
}

#[test]
fn saved_view_binding_reports_disabled_when_no_view_context() {
    let mut app = app_with_rows(rows(&[&["Name"], &["alpha"]]));

    app.apply(command::KeyAction {
        command: command::Command::SavedView,
        count: None,
    })
    .expect("apply");

    assert_eq!(app.popup, None);
    assert_eq!(app.message.as_deref(), Some("saved views are disabled"));
}
