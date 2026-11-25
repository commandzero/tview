use super::{ColumnWidthMode, TableView};
use unicode_width::UnicodeWidthStr;

impl TableView {
    pub fn with_column_width_mode(mut self, mode: ColumnWidthMode) -> Self {
        self.column_width_mode = mode;
        self
    }

    pub fn column_gap(&self) -> usize {
        self.column_gap
    }

    pub fn column_width_mode(&self) -> ColumnWidthMode {
        self.column_width_mode
    }

    pub fn set_terminal_width(&mut self, width: usize) {
        let width = width.max(1);
        if self.terminal_width != width {
            self.terminal_width = width;
            self.computed_column_widths_cache.clear();
        }
    }

    pub fn set_column_gap(&mut self, gap: usize) {
        self.column_gap = gap;
        self.computed_column_widths_cache.clear();
    }

    pub fn adjust_column_gap(&mut self, delta: isize) {
        self.column_gap = self.column_gap.saturating_add_signed(delta);
        self.computed_column_widths_cache.clear();
    }

    pub fn set_column_width_mode(&mut self, mode: ColumnWidthMode) {
        self.column_width_mode = mode;
        self.column_widths.clear();
        self.sampled_column_widths.clear();
        self.computed_column_widths_cache.clear();
        self.column_width_modified.clear();
        if mode == ColumnWidthMode::Max {
            match self.exact_reduction_profiles() {
                Ok(Some(profiles)) => {
                    self.sampled_column_widths = profiles
                        .into_iter()
                        .map(|profile| profile.max_width.clamp(1, 250))
                        .collect();
                }
                Ok(None) => {}
                Err(error) => self.source_status = Some(format!("Profiling failed: {error}")),
            }
        }
    }

    pub fn toggle_variable_column_width_mode(&mut self) {
        self.column_width_mode = match self.column_width_mode {
            ColumnWidthMode::Mode => ColumnWidthMode::Max,
            ColumnWidthMode::Max | ColumnWidthMode::Fixed(_) => ColumnWidthMode::Mode,
        };
        self.column_widths.clear();
        self.sampled_column_widths.clear();
        self.computed_column_widths_cache.clear();
        self.column_width_modified.clear();
    }

    pub fn set_all_column_widths(&mut self, width: usize) {
        self.column_width_mode = ColumnWidthMode::Fixed(width.clamp(1, u16::MAX as usize) as u16);
        self.column_widths.clear();
        self.computed_column_widths_cache.clear();
        self.column_width_modified = (0..self.source_column_count()).collect();
    }

    pub fn set_current_column_width(&mut self, width: usize) {
        self.ensure_custom_column_widths();
        let Some(source_column) = self.source_column_for_visible(self.cursor.column) else {
            return;
        };
        if let Some(column_width) = self.column_widths.get_mut(source_column) {
            *column_width = width.max(1);
            self.column_width_modified.insert(source_column);
        }
    }

    pub fn maximize_current_column_width(&mut self) {
        let max_widths = self.computed_column_widths(ColumnWidthMode::Max);
        let Some(source_column) = self.source_column_for_visible(self.cursor.column) else {
            return;
        };
        if let Some(width) = max_widths.get(source_column) {
            self.set_current_column_width(*width);
        }
    }

    pub fn adjust_all_column_widths(&mut self, delta: isize) {
        self.ensure_custom_column_widths();
        for width in &mut self.column_widths {
            *width = width.saturating_add_signed(delta).max(1);
        }
        self.column_width_modified = (0..self.source_column_count()).collect();
    }

    pub fn adjust_current_column_width(&mut self, steps: isize) {
        if steps == 0 {
            return;
        }
        let Some(source_column) = self.source_column_for_visible(self.cursor.column) else {
            return;
        };
        let max_width = self
            .cached_rendered_value_width(source_column)
            .max(self.rendered_source_header_width(source_column));
        self.ensure_custom_column_widths();
        let Some(width) = self.column_widths.get_mut(source_column) else {
            return;
        };
        for _ in 0..steps.unsigned_abs() {
            let adjustment = (*width / 5).max(1);
            *width = if steps.is_positive() {
                width.saturating_add(adjustment).min(max_width).max(*width)
            } else {
                width.saturating_sub(adjustment).max(1)
            };
        }
        self.column_width_modified.insert(source_column);
    }

    pub fn effective_column_widths(&self) -> Vec<usize> {
        let mut source_widths = if self.column_widths.len() == self.source_column_count() {
            self.column_widths.clone()
        } else if self.sampled_column_widths.len() == self.source_column_count() {
            self.sampled_column_widths.clone()
        } else {
            self.computed_column_widths(self.column_width_mode)
        };
        if self.column_widths.len() != self.source_column_count()
            && !matches!(self.column_width_mode, ColumnWidthMode::Fixed(_))
        {
            self.cap_automatic_widths(&mut source_widths);
        }
        self.visible_source_columns()
            .into_iter()
            .map(|source_column| source_widths.get(source_column).copied().unwrap_or(1))
            .collect()
    }

    pub fn effective_column_widths_cached(&mut self) -> Vec<usize> {
        if self.column_widths.len() == self.source_column_count() {
            let source_widths = &self.column_widths;
            return self
                .visible_source_columns_iter()
                .map(|source_column| source_widths.get(source_column).copied().unwrap_or(1))
                .collect();
        }
        if matches!(self.column_width_mode, ColumnWidthMode::Fixed(_)) {
            return self.effective_column_widths();
        }
        if self.sampled_column_widths.len() != self.source_column_count() {
            let observed = self.computed_column_widths(self.column_width_mode);
            self.sampled_column_widths
                .truncate(self.source_column_count());
            let sampled_len = self.sampled_column_widths.len();
            self.sampled_column_widths
                .extend(observed.into_iter().skip(sampled_len));
        }
        if self.computed_column_widths_cache.len() != self.source_column_count() {
            self.computed_column_widths_cache = self.sampled_column_widths.clone();
            let cap = self.automatic_column_width_cap();
            for width in &mut self.computed_column_widths_cache {
                *width = (*width).min(cap);
            }
        }
        let source_widths = &self.computed_column_widths_cache;
        self.visible_source_columns_iter()
            .map(|source_column| source_widths.get(source_column).copied().unwrap_or(1))
            .collect()
    }

    pub(super) fn ensure_custom_column_widths(&mut self) {
        if self.column_widths.len() != self.source_column_count() {
            self.column_widths = self.effective_column_widths();
        }
        self.computed_column_widths_cache.clear();
    }

    fn automatic_column_width_cap(&self) -> usize {
        if self.terminal_width == 0 {
            usize::MAX
        } else {
            (self.terminal_width.saturating_mul(4) / 5).max(1)
        }
    }

    fn cap_automatic_widths(&self, widths: &mut [usize]) {
        let cap = self.automatic_column_width_cap();
        for width in widths {
            *width = (*width).min(cap);
        }
    }

    pub(super) fn computed_column_widths(&self, mode: ColumnWidthMode) -> Vec<usize> {
        if let ColumnWidthMode::Fixed(width) = mode {
            return vec![width as usize; self.source_column_count()];
        }

        let mut rows = Vec::new();
        if let Some(header) = self.rendered_source_header() {
            rows.push(header);
        }
        rows.extend(self.rendered_source_rows());
        column_widths(&rows, mode, self.column_gap)
    }

    #[cfg(feature = "saved-views")]
    pub(super) fn computed_header_widths(&self) -> Vec<usize> {
        if let Some(header) = self.rendered_source_header() {
            header
                .iter()
                .map(|cell| UnicodeWidthStr::width(cell.as_str()).max(1))
                .collect()
        } else {
            vec![1; self.source_column_count()]
        }
    }

    #[cfg(feature = "saved-views")]
    pub(super) fn computed_content_widths(&self) -> Vec<usize> {
        let mut rows = Vec::new();
        if let Some(header) = self.rendered_source_header() {
            rows.push(header);
        }
        rows.extend(self.rendered_source_rows());
        column_widths(&rows, ColumnWidthMode::Max, self.column_gap)
    }

    fn rendered_source_rows(&self) -> Vec<Vec<String>> {
        self.rows
            .iter()
            .map(|row| {
                (0..self.source_column_count())
                    .map(|source_column| {
                        self.render_source_cell(
                            source_column,
                            row.get(source_column).map(String::as_str),
                        )
                    })
                    .collect()
            })
            .collect()
    }

    fn cached_rendered_value_width(&self, source_column: usize) -> usize {
        self.rows
            .iter()
            .map(|row| {
                self.render_source_cell(source_column, row.get(source_column).map(String::as_str))
            })
            .map(|value| UnicodeWidthStr::width(value.as_str()))
            .max()
            .unwrap_or(1)
            .max(1)
    }

    fn rendered_source_header_width(&self, source_column: usize) -> usize {
        self.rendered_source_header()
            .and_then(|header| {
                header
                    .get(source_column)
                    .map(|value| UnicodeWidthStr::width(value.as_str()))
            })
            .unwrap_or(1)
            .max(1)
    }
}

pub fn column_widths(rows: &[Vec<String>], mode: ColumnWidthMode, gap: usize) -> Vec<usize> {
    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    match mode {
        ColumnWidthMode::Fixed(width) => vec![width as usize; column_count],
        ColumnWidthMode::Max => (0..column_count)
            .map(|column| {
                rows.iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| UnicodeWidthStr::width(cell.as_str()))
                    .max()
                    .unwrap_or(1)
                    .clamp(1, 250)
            })
            .collect(),
        ColumnWidthMode::Mode => (0..column_count)
            .map(|column| mode_width(rows, column, gap))
            .collect(),
    }
}

fn mode_width(rows: &[Vec<String>], column: usize, _gap: usize) -> usize {
    rows.iter()
        .filter_map(|row| row.get(column))
        .map(|cell| UnicodeWidthStr::width(cell.as_str()))
        .max()
        .unwrap_or(1)
        .clamp(1, 250)
}
