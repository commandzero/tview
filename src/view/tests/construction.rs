use super::*;

#[test]
fn source_column_name_for_id_preserves_duplicate_relation_occurrences() {
    let generation = SourceGeneration::new();
    let mut view = TableView::classify(
        rows(&[&["first", "second"], &["1", "2"]]),
        Viewport::new(10, 4),
    );
    view.table_definition = Some(TableDefinition {
        generation,
        columns: (0..2)
            .map(|ordinal| ColumnDefinition {
                id: ColumnId {
                    generation,
                    ordinal,
                },
                source_identity: ColumnSourceIdentity::RelationColumn {
                    relation: "duplicates".to_owned(),
                    name: "value".to_owned(),
                    ordinal: ordinal as usize,
                },
                display_name: "value".to_owned(),
                source_declared_type: None,
                source_type: LogicalType::Text,
                type_origin: TypeOrigin::Declared,
            })
            .collect(),
        schema_state: SchemaState::Complete,
        relation: RelationMetadata::implicit("duplicates", true),
    });

    assert_eq!(
        view.source_column_name_for_id(ColumnId {
            generation,
            ordinal: 0
        })
        .as_deref(),
        Some("value#1")
    );
    assert_eq!(
        view.source_column_name_for_id(ColumnId {
            generation,
            ordinal: 1
        })
        .as_deref(),
        Some("value#2")
    );
}

#[test]
fn classifies_non_numeric_first_row_as_header() {
    let view = TableView::classify(
        rows(&[&["Name", "Value"], &["alpha", "1"]]),
        Viewport::new(10, 4),
    );
    assert_eq!(view.header().expect("header"), ["Name", "Value"]);
    assert!(view.header_visible());
    assert_eq!(view.rows(), rows(&[&["alpha", "1"]]));
}

#[test]
fn keeps_numeric_first_row_as_data() {
    let view = TableView::classify(rows(&[&["1", "2"], &["3", "4"]]), Viewport::new(10, 4));
    assert!(view.header().is_none());
    assert_eq!(view.rows(), rows(&[&["1", "2"], &["3", "4"]]));
}

#[test]
fn toggles_header_structurally() {
    let mut view = TableView::classify(
        rows(&[&["Name", "Value"], &["Name", "Value"]]),
        Viewport::new(10, 4),
    );
    view.toggle_header();
    assert!(!view.header_visible());
    assert_eq!(view.header().expect("header"), ["Name", "Value"]);
    assert_eq!(view.rows(), rows(&[&["Name", "Value"]]));
}
