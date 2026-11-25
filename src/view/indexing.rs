use super::column::Columns;
use super::{ColumnDisplayMetadata, TableView};
use crate::table::{
    IndexProgress, Row, RowCount, RowIndex, ScanDirection, ScanRequest, SchemaDelta, TableStore,
};

impl TableView {
    pub fn row_count_state(&self) -> RowCount {
        if self.view_transform_is_active() {
            return self
                .query_store
                .as_ref()
                .map(|store| store.0.borrow().row_count())
                .unwrap_or(RowCount::Exact(self.rows.len()));
        }
        self.source_store
            .as_ref()
            .map(TableStore::row_count)
            .or_else(|| {
                self.incremental_store
                    .as_ref()
                    .map(|store| store.0.borrow().row_count())
            })
            .unwrap_or(RowCount::Exact(self.rows.len()))
    }

    pub fn ensure_source_indexed_through(&mut self, row: usize) -> anyhow::Result<RowCount> {
        if let Some(source) = &self.source_store {
            return Ok(source.row_count());
        }
        let loading_query_result = self.view_transform_is_active() && self.query_store.is_some();
        let shared = if loading_query_result {
            self.query_store.clone()
        } else {
            self.incremental_store.clone()
        };
        let Some(shared) = shared else {
            return Ok(RowCount::Exact(self.rows.len()));
        };
        let previous_len = self.rows.len();
        let result = index_rows(shared.0.borrow_mut().as_mut(), row, previous_len);
        let (progress, appended) = result.inspect_err(|error| {
            self.source_status = Some(format!("Indexing failed: {error}"));
        })?;
        self.apply_source_schema_delta(progress.schema_delta)?;
        if self.view_transform_is_active() && !loading_query_result {
            return Ok(self.row_count_state());
        }
        self.append_indexed_rows(appended, previous_len);
        Ok(progress.row_count)
    }

    fn append_indexed_rows(&mut self, appended: Vec<Row>, previous_len: usize) {
        for row in appended {
            self.append_source_row(row);
        }
        if self.rows.len() != previous_len {
            self.refresh_resident_color_facts_on_append();
        }
        self.visible_rows.extend(previous_len..self.rows.len());
        if self.rows.len().saturating_sub(previous_len) >= 256 {
            self.source_status = Some(format!("Indexed {} rows", self.rows.len()));
        }
    }

    pub(super) fn append_source_row(&mut self, row: Row) {
        self.row_ids.push(row.id);
        let mut cells = row.display_cells();
        cells.resize(self.source_column_count(), String::new());
        self.rows.push(cells);
    }

    pub(super) fn view_transform_is_active(&self) -> bool {
        self.active_view_transform
            .as_ref()
            .is_some_and(|query| !query.filters.is_empty() || !query.order_by.is_empty())
    }

    pub fn take_source_status(&mut self) -> Option<String> {
        self.source_status.take()
    }

    pub(super) fn apply_source_schema_delta(&mut self, delta: SchemaDelta) -> anyhow::Result<()> {
        self.replay_projection_schema()?;
        if delta.is_empty() {
            #[cfg(feature = "saved-views")]
            if delta.completed {
                self.bind_after_schema_change(true);
            }
            return Ok(());
        }
        #[cfg(feature = "saved-views")]
        let completed = delta.completed;
        let Some(definition) = self.table_definition.as_mut() else {
            return Ok(());
        };
        definition.apply_delta(delta)?;
        let new_count = definition.columns.len();
        self.header = Some(schema_header(definition, &self.column_label_overrides));
        self.resize_for_schema(new_count);
        #[cfg(feature = "saved-views")]
        self.bind_after_schema_change(completed);
        Ok(())
    }

    fn replay_projection_schema(&mut self) -> anyhow::Result<()> {
        let pending = self.pending_projection_schema.borrow_mut().take();
        if let Some((_, replay)) =
            pending.filter(|(generation, _)| self.source_generation() == Some(*generation))
        {
            self.apply_source_schema_delta(replay)?;
        }
        Ok(())
    }

    fn resize_for_schema(&mut self, new_count: usize) {
        for row in &mut self.rows {
            row.resize(new_count, String::new());
        }
        self.column_alignment_overrides.resize(new_count, None);
        self.column_label_overrides.resize(new_count, None);
        self.column_nulls.resize(new_count, None);
        self.column_display
            .resize(new_count, ColumnDisplayMetadata::default());
        self.column_color_rules.resize(new_count, Vec::new());
        self.compiled_column_colors.resize(new_count, None);
        self.invalidate_color_source_facts();
        #[cfg(feature = "saved-views")]
        self.saved_column_widths.resize(new_count, None);
        self.columns = Columns::infer(self.header.as_deref(), &self.rows);
        self.computed_column_widths_cache.clear();
    }

    #[cfg(feature = "saved-views")]
    fn bind_after_schema_change(&mut self, completed: bool) {
        self.advance_saved_binding(false);
        if completed {
            self.advance_saved_binding(true);
        }
    }
}

fn index_rows(
    store: &mut dyn TableStore,
    row: usize,
    start: usize,
) -> anyhow::Result<(IndexProgress, Vec<Row>)> {
    let mut appended = Vec::new();
    let mut collect_row = |_: RowIndex, source_row: &Row| {
        appended.push(source_row.clone());
        std::ops::ControlFlow::Continue(())
    };
    if row >= start {
        let max_rows = row.saturating_sub(start).saturating_add(1);
        let progress = store.index_and_scan_rows(
            RowIndex(row),
            forward_scan(start, max_rows),
            &mut collect_row,
        )?;
        return Ok((progress.index, appended));
    }
    let progress = store.ensure_indexed_through(RowIndex(row))?;
    let available = match progress.row_count {
        RowCount::Exact(count) | RowCount::AtLeast(count) => count,
        RowCount::Unknown => 0,
    };
    let max_rows = available.saturating_sub(start).min(256);
    if max_rows > 0 {
        store.scan_rows(forward_scan(start, max_rows), &mut collect_row)?;
    }
    Ok((progress, appended))
}

fn forward_scan(start: usize, max_rows: usize) -> ScanRequest {
    ScanRequest {
        start: RowIndex(start),
        direction: ScanDirection::Forward,
        max_rows,
    }
}

fn schema_header(
    definition: &crate::table::TableDefinition,
    labels: &[Option<String>],
) -> Vec<String> {
    definition
        .columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            labels
                .get(index)
                .and_then(Option::as_ref)
                .unwrap_or(&column.display_name)
                .clone()
        })
        .collect()
}
