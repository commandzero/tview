use super::{Position, TableView, Viewport};

impl TableView {
    pub fn cursor(&self) -> Position {
        self.cursor
    }

    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    pub fn mark(&self) -> Option<Position> {
        self.mark
    }

    pub fn resize_viewport(&mut self, height: usize, width: usize) {
        self.viewport.height = height.max(1);
        self.viewport.width = width.max(1);
        self.keep_cursor_visible();
        let last_visible_row = self
            .viewport
            .origin
            .row
            .saturating_add(self.viewport.height.saturating_sub(1));
        let _ = self.ensure_source_indexed_through(last_visible_row);
    }

    pub fn goto(&mut self, row: usize, column: usize) {
        let _ = self.ensure_source_indexed_through(row);
        self.cursor.row = row.min(self.visible_rows.len().saturating_sub(1));
        self.cursor.column = column.min(self.column_count().saturating_sub(1));
        self.keep_cursor_visible();
    }

    pub fn move_by(&mut self, row_delta: isize, column_delta: isize) {
        let row = self.cursor.row.saturating_add_signed(row_delta);
        let column = self.cursor.column.saturating_add_signed(column_delta);
        self.goto(row, column);
    }

    pub fn page_by(&mut self, row_pages: isize, column_pages: isize, count: usize) {
        let count = count.max(1) as isize;
        let row_delta = row_pages * self.viewport.height.max(1) as isize * count;
        let column_delta = column_pages * self.viewport.width.max(1) as isize * count;
        self.move_by(row_delta, column_delta);
    }

    pub fn goto_top(&mut self) {
        self.goto(0, self.cursor.column);
    }

    pub fn goto_bottom(&mut self) {
        let _ = self.ensure_source_indexed_through(usize::MAX);
        self.goto(
            self.visible_rows.len().saturating_sub(1),
            self.cursor.column,
        );
    }

    pub fn goto_user_row(&mut self, row: usize) {
        self.goto(row.saturating_sub(1), self.cursor.column);
    }

    pub fn goto_user_column(&mut self, column: usize) {
        self.goto(self.cursor.row, column.saturating_sub(1));
    }

    pub fn set_mark(&mut self) {
        self.mark = Some(self.cursor);
        self.mark_identity = self
            .visible_rows
            .get(self.cursor.row)
            .and_then(|row| self.row_ids.get(*row))
            .copied()
            .zip(
                self.source_column_for_visible(self.cursor.column)
                    .and_then(|column| {
                        self.table_definition
                            .as_ref()
                            .and_then(|definition| definition.columns.get(column))
                            .map(|column| column.id)
                    }),
            );
    }

    pub fn goto_mark(&mut self) {
        if let Some((row_id, column_id)) = self.mark_identity {
            let row = self
                .row_ids
                .iter()
                .position(|candidate| *candidate == row_id);
            let column = self
                .table_definition
                .as_ref()
                .and_then(|definition| {
                    definition
                        .columns
                        .iter()
                        .position(|column| column.id == column_id)
                })
                .and_then(|source| self.visible_column_for_source(source));
            if let (Some(row), Some(column)) = (row, column) {
                self.goto(row, column);
                self.mark = Some(self.cursor);
            }
            return;
        }
        if let Some(mark) = self.mark {
            self.goto(mark.row, mark.column);
        }
    }

    pub(super) fn keep_cursor_visible(&mut self) {
        self.cursor.row = self
            .cursor
            .row
            .min(self.visible_rows.len().saturating_sub(1));
        self.cursor.column = self
            .cursor
            .column
            .min(self.column_count().saturating_sub(1));
        if self.cursor.row < self.viewport.origin.row {
            self.viewport.origin.row = self.cursor.row;
        } else if self.cursor.row >= self.viewport.origin.row + self.viewport.height {
            self.viewport.origin.row = self.cursor.row + 1 - self.viewport.height;
        }

        if self.cursor.column < self.viewport.origin.column {
            self.viewport.origin.column = self.cursor.column;
        } else if self.cursor.column >= self.viewport.origin.column + self.viewport.width {
            self.viewport.origin.column = self.cursor.column + 1 - self.viewport.width;
        }
    }
}
