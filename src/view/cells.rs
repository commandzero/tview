use super::render;
use super::TableView;

impl TableView {
    pub fn header(&self) -> Option<&[String]> {
        self.header.as_deref()
    }

    pub fn rendered_header(&self) -> Option<Vec<String>> {
        self.rendered_source_header().map(|header| {
            header
                .iter()
                .enumerate()
                .filter(|(source_column, _)| self.source_column_visible(*source_column))
                .map(|(_, cell)| cell.clone())
                .collect()
        })
    }

    /// Header labels for document output, excluding TUI sort/filter glyphs.
    pub fn output_header(&self) -> Option<Vec<String>> {
        self.header.as_ref().map(|header| {
            header
                .iter()
                .enumerate()
                .filter(|(source_column, _)| self.source_column_visible(*source_column))
                .map(|(_, cell)| cell.clone())
                .collect()
        })
    }

    pub(super) fn rendered_source_header(&self) -> Option<Vec<String>> {
        self.header.as_ref().map(|header| {
            header
                .iter()
                .enumerate()
                .map(|(source_column, cell)| {
                    format!(
                        "{}{}{cell}",
                        self.source_column_sort_indicator(source_column)
                            .unwrap_or_default(),
                        self.source_column_filter_indicator(source_column)
                            .unwrap_or_default()
                    )
                })
                .collect()
        })
    }

    pub fn header_visible(&self) -> bool {
        self.header_visible
    }

    pub fn rows(&self) -> &[Vec<String>] {
        &self.rows
    }

    pub fn row_count(&self) -> usize {
        self.visible_rows.len()
    }

    pub fn total_row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn visible_rows(&self) -> impl Iterator<Item = &Vec<String>> {
        self.visible_rows
            .iter()
            .filter_map(|row_idx| self.rows.get(*row_idx))
    }

    pub fn visible_rows_vec(&self) -> Vec<Vec<String>> {
        self.visible_rows()
            .map(|row| self.visible_cells_for_row(row))
            .collect()
    }

    pub fn visible_row_count(&self) -> usize {
        self.visible_rows.len()
    }

    pub fn rendered_visible_row(&self, row: usize) -> Option<Vec<String>> {
        self.visible_row(row)
            .map(|row| self.visible_cells_for_row(row))
    }

    pub fn visible_raw_rows_vec(&self) -> Vec<Vec<String>> {
        self.visible_rows()
            .map(|row| {
                self.visible_source_columns()
                    .into_iter()
                    .map(|source_column| row.get(source_column).cloned().unwrap_or_default())
                    .collect()
            })
            .collect()
    }

    fn visible_cells_for_row(&self, row: &[String]) -> Vec<String> {
        self.visible_source_columns()
            .into_iter()
            .map(|source_column| {
                self.render_source_cell(source_column, row.get(source_column).map(String::as_str))
            })
            .collect()
    }

    pub fn current_cell(&self) -> Option<&str> {
        let source_column = self.source_column_for_visible(self.cursor.column)?;
        self.visible_row(self.cursor.row)?
            .get(source_column)
            .map(String::as_str)
    }

    pub fn current_raw_cell(&self) -> Option<&str> {
        self.current_cell()
    }

    pub fn current_cell_rendered(&self) -> Option<String> {
        let source_column = self.source_column_for_visible(self.cursor.column)?;
        let raw = self
            .visible_row(self.cursor.row)?
            .get(source_column)
            .map(String::as_str);
        Some(self.render_source_cell(source_column, raw))
    }

    pub fn visible_row(&self, row: usize) -> Option<&Vec<String>> {
        self.visible_rows
            .get(row)
            .and_then(|row_idx| self.rows.get(*row_idx))
    }

    pub fn source_row_for_visible_row(&self, row: usize) -> Option<usize> {
        self.visible_rows.get(row).copied()
    }

    pub(super) fn render_source_cell(&self, source_column: usize, raw: Option<&str>) -> String {
        render::cell(
            self.column_display
                .get(source_column)
                .copied()
                .unwrap_or_default(),
            self.source_numeric_column_profile(source_column),
            raw.unwrap_or_default(),
        )
    }
}
