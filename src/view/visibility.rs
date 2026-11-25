use super::TableView;

impl TableView {
    pub fn column_count(&self) -> usize {
        self.visible_source_columns().len()
    }

    pub fn source_column_count(&self) -> usize {
        self.columns.len()
    }

    pub fn hidden_column_count(&self) -> usize {
        self.hidden_columns.len()
    }

    pub fn hide_current_column(&mut self) {
        if self.column_count() <= 1 {
            return;
        }
        if let Some(source_column) = self.source_column_for_visible(self.cursor.column) {
            self.hidden_columns.insert(source_column);
            self.keep_cursor_visible();
        }
    }

    pub fn hide_columns_left(&mut self, count: usize) {
        let count = count.max(1);
        let current = self.cursor.column;
        let start = current.saturating_sub(count);
        let to_hide = (start..current)
            .filter_map(|column| self.source_column_for_visible(column))
            .collect::<Vec<_>>();
        for source_column in to_hide {
            self.hidden_columns.insert(source_column);
        }
        self.goto(
            self.cursor.row,
            start.min(self.column_count().saturating_sub(1)),
        );
    }

    pub fn hide_columns_right(&mut self, count: usize) {
        let count = count.max(1);
        let current = self.cursor.column;
        let to_hide = (current + 1..=current.saturating_add(count))
            .filter_map(|column| self.source_column_for_visible(column))
            .collect::<Vec<_>>();
        for source_column in to_hide {
            self.hidden_columns.insert(source_column);
        }
        self.keep_cursor_visible();
    }

    pub fn show_hidden_left(&mut self, count: usize) {
        let Some(current_source) = self.source_column_for_visible(self.cursor.column) else {
            return;
        };
        for (shown, source_column) in (0..current_source).rev().enumerate() {
            if shown >= count.max(1) || !self.hidden_columns.contains(&source_column) {
                break;
            }
            self.hidden_columns.remove(&source_column);
        }
        self.cursor.column = self
            .visible_column_for_source(current_source)
            .unwrap_or(self.cursor.column);
        self.keep_cursor_visible();
    }

    pub fn show_hidden_right(&mut self, count: usize) {
        let Some(current_source) = self.source_column_for_visible(self.cursor.column) else {
            return;
        };
        for (shown, source_column) in (current_source + 1..self.source_column_count()).enumerate() {
            if shown >= count.max(1) || !self.hidden_columns.contains(&source_column) {
                break;
            }
            self.hidden_columns.remove(&source_column);
        }
        self.keep_cursor_visible();
    }

    pub fn hidden_boundary_before(&self, column: usize) -> bool {
        let visible_columns = self.visible_source_columns();
        let Some(&source_column) = visible_columns.get(column) else {
            return false;
        };
        let previous_source = column
            .checked_sub(1)
            .and_then(|previous| visible_columns.get(previous).copied());
        let start = previous_source.map_or(0, |previous| previous + 1);
        (start..source_column).any(|source_column| self.hidden_columns.contains(&source_column))
    }

    pub fn hidden_boundary_after_last(&self) -> bool {
        let Some(last_visible) = self.visible_source_columns().last().copied() else {
            return false;
        };
        (last_visible + 1..self.source_column_count())
            .any(|source_column| self.hidden_columns.contains(&source_column))
    }

    pub(super) fn source_column_visible(&self, source_column: usize) -> bool {
        !self.hidden_columns.contains(&source_column)
    }

    pub(super) fn visible_source_columns(&self) -> Vec<usize> {
        self.visible_source_columns_iter().collect()
    }

    pub(crate) fn visible_source_columns_vec(&self) -> Vec<usize> {
        self.visible_source_columns()
    }

    pub(super) fn visible_source_columns_iter(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.source_column_count())
            .filter(|source_column| self.source_column_visible(*source_column))
    }

    pub(super) fn source_column_for_visible(&self, column: usize) -> Option<usize> {
        self.visible_source_columns_iter().nth(column)
    }

    pub(super) fn visible_column_for_source(&self, source_column: usize) -> Option<usize> {
        self.visible_source_columns_iter()
            .position(|candidate| candidate == source_column)
    }
}
