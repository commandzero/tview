use super::column::Columns;
use super::{
    ColumnDisplayMetadata, ColumnWidthMode, Position, SharedTableStore, TableView, Viewport,
};
use crate::ingest::OpenedTable;
use crate::table::{RowIndex, SourceGeneration, TableDefinition};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

impl TableView {
    pub fn classify(rows: Vec<Vec<String>>, viewport: Viewport) -> Self {
        let has_header = rows.len() > 1
            && rows
                .first()
                .is_some_and(|row| !row.iter().any(|cell| cell.parse::<f64>().is_ok()));

        let (header, rows) = if has_header {
            let mut rows = rows;
            (Some(rows.remove(0)), rows)
        } else {
            (None, rows)
        };

        Self::from_rows(header, rows, viewport)
    }

    fn from_rows(header: Option<Vec<String>>, rows: Vec<Vec<String>>, viewport: Viewport) -> Self {
        let columns = Columns::infer(header.as_deref(), &rows);
        let column_count = columns.len();

        Self {
            table_definition: None,
            source_store: None,
            incremental_store: None,
            query_store: None,
            base_cached_rows: None,
            base_cached_row_ids: None,
            active_source_query: None,
            source_capabilities: crate::table::SourceOperationCapabilities::default(),
            source_result_extent: Some(crate::table::ResultExtent::Complete {
                source_rows: rows.len(),
            }),
            source_result_is_partial: false,
            source_result_warnings: Vec::new(),
            pending_projection_schema: RefCell::new(None),
            source_query_provenance: None,
            source_query_coordinator: Rc::default(),
            committed_source: None,
            pending_source: None,
            latest_activation_error: None,
            active_view_transform: None,
            committed_filters: Vec::new(),
            committed_sort_keys: Vec::new(),
            row_ids: Vec::new(),
            object_mode: None,
            source_status: None,
            header_visible: header.is_some(),
            header,
            visible_rows: (0..rows.len()).collect(),
            rows,
            filters: Vec::new(),
            cursor: Position::default(),
            viewport,
            mark: None,
            mark_identity: None,
            column_width_mode: ColumnWidthMode::Mode,
            column_gap: 2,
            column_widths: Vec::new(),
            sampled_column_widths: Vec::new(),
            computed_column_widths_cache: Vec::new(),
            terminal_width: 0,
            column_width_modified: BTreeSet::new(),
            hidden_columns: BTreeSet::new(),
            column_alignment_overrides: vec![None; column_count],
            column_label_overrides: vec![None; column_count],
            view_nulls: None,
            column_nulls: vec![None; column_count],
            column_display: vec![ColumnDisplayMetadata::default(); column_count],
            column_color_rules: vec![Vec::new(); column_count],
            compiled_column_colors: vec![None; column_count],
            color_cache_valid: true,
            exact_color_profiles: None,
            column_metadata_modified: BTreeSet::new(),
            sort_keys: Vec::new(),
            columns,
            #[cfg(feature = "saved-views")]
            resident_source_header: None,
            #[cfg(feature = "saved-views")]
            saved_binding: None,
            #[cfg(feature = "saved-views")]
            saved_binding_warnings: Vec::new(),
            #[cfg(feature = "saved-views")]
            saved_column_widths: vec![None; column_count],
        }
    }

    pub fn from_opened_table(mut opened: OpenedTable, viewport: Viewport) -> anyhow::Result<Self> {
        let header_visible = opened.definition.relation.header_visible;
        let generation = opened.definition.generation;
        let object_mode = opened.object_mode;
        let mut source_result_warnings = opened.warnings;
        source_result_warnings.extend(opened.store.result_warnings().iter().cloned());
        source_result_warnings.sort();
        source_result_warnings.dedup();
        let source_status =
            (!source_result_warnings.is_empty()).then(|| source_result_warnings.join("; "));
        let active_source_query = opened.store.active_source_query().cloned();
        let source_capabilities = opened.store.source_capabilities();
        let source_result_extent = opened.store.result_extent();
        let source_result_is_partial = opened.store.result_is_partial();
        let source_query_provenance = opened.store.query_provenance().cloned();
        let initial_target = viewport.height.saturating_sub(1);
        let progress = opened
            .store
            .ensure_indexed_through(RowIndex(initial_target))?;
        opened.definition.apply_delta(progress.schema_delta)?;
        let mut rows = Vec::new();
        let mut row_ids = Vec::new();
        for index in 0..=initial_target {
            let Some(row) = opened.store.row(RowIndex(index))? else {
                break;
            };
            row_ids.push(row.id);
            rows.push(row.display_cells());
        }
        let header = Some(
            opened
                .definition
                .columns
                .iter()
                .map(|column| column.display_name.clone())
                .collect::<Vec<_>>(),
        );
        let mut view = Self::from_rows(header, rows, viewport);
        view.table_definition = Some(opened.definition);
        view.incremental_store = Some(SharedTableStore(Rc::new(RefCell::new(opened.store))));
        view.active_source_query = active_source_query;
        view.source_capabilities = source_capabilities;
        view.source_result_extent = source_result_extent;
        view.source_result_is_partial = source_result_is_partial;
        view.source_result_warnings = source_result_warnings;
        view.source_query_provenance = source_query_provenance;
        view.active_view_transform = Some(crate::table::ViewTransform {
            generation,
            filters: Vec::new(),
            order_by: Vec::new(),
        });
        view.row_ids = row_ids;
        view.object_mode = object_mode;
        view.source_status = source_status;
        view.header_visible = header_visible;
        Ok(view)
    }

    pub fn source_generation(&self) -> Option<SourceGeneration> {
        self.table_definition
            .as_ref()
            .map(|definition| definition.generation)
    }

    pub fn table_definition(&self) -> Option<&TableDefinition> {
        self.table_definition.as_ref()
    }
}
