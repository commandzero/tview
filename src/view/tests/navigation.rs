use super::*;

#[test]
fn keeps_cursor_inside_table_and_viewport() {
    let mut view = TableView::classify(
        rows(&[&["A", "B"], &["1", "2"], &["3", "4"]]),
        Viewport::new(1, 1),
    );
    view.goto(10, 10);
    assert_eq!(view.cursor(), Position { row: 1, column: 1 });
    assert_eq!(view.viewport().origin, Position { row: 1, column: 1 });
}

#[test]
fn resizes_viewport_and_keeps_cursor_visible() {
    let mut view = TableView::classify(
        rows(&[
            &["A", "B", "C"],
            &["1", "2", "3"],
            &["4", "5", "6"],
            &["7", "8", "9"],
        ]),
        Viewport::new(3, 3),
    );
    view.goto(2, 2);
    view.resize_viewport(1, 1);
    assert_eq!(view.viewport().origin, Position { row: 2, column: 2 });
}

#[test]
fn page_motion_uses_viewport_size() {
    let mut view = TableView::classify(
        rows(&[
            &["A"],
            &["1"],
            &["2"],
            &["3"],
            &["4"],
            &["5"],
            &["6"],
            &["7"],
        ]),
        Viewport::new(3, 1),
    );
    view.page_by(1, 0, 2);
    assert_eq!(view.cursor().row, 6);
    view.page_by(-1, 0, 1);
    assert_eq!(view.cursor().row, 3);
}

#[test]
fn mark_round_trips_cursor_position() {
    let mut view = TableView::classify(
        rows(&[&["A", "B"], &["1", "2"], &["3", "4"]]),
        Viewport::new(3, 2),
    );
    view.goto(1, 1);
    view.set_mark();
    view.goto(0, 0);
    view.goto_mark();
    assert_eq!(view.cursor(), Position { row: 1, column: 1 });
}

#[test]
fn hidden_columns_are_omitted_from_visible_rows_and_navigation() {
    let mut view = TableView::classify(
        rows(&[&["A", "B", "C"], &["a1", "b1", "c1"]]),
        Viewport::new(10, 3),
    );
    view.goto(0, 1);
    view.hide_current_column();

    assert_eq!(view.column_count(), 2);
    assert_eq!(view.visible_rows_vec(), rows(&[&["a1", "c1"]]));
    assert_eq!(
        view.rendered_header().expect("header"),
        rows(&[&["A", "C"]])[0]
    );
    assert_eq!(view.current_cell(), Some("c1"));
    assert!(view.hidden_boundary_before(1));

    view.show_hidden_left(1);
    assert_eq!(view.column_count(), 3);
    assert_eq!(view.visible_rows_vec(), rows(&[&["a1", "b1", "c1"]]));
}

#[test]
fn hide_current_column_preserves_last_visible_column() {
    let mut view = TableView::classify(rows(&[&["A"], &["a1"]]), Viewport::new(10, 1));
    view.hide_current_column();

    assert_eq!(view.column_count(), 1);
    assert_eq!(view.current_cell(), Some("a1"));
}
