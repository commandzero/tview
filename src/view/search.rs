use super::{Position, TableView};
use crate::ops::search::CaseInsensitiveQuery;
use crate::table::RowCount;

impl TableView {
    pub fn progressive_search(
        &mut self,
        query: &str,
        direction: crate::ops::search::SearchDirection,
    ) -> Option<Position> {
        let matcher = CaseInsensitiveQuery::new(query)?;
        use crate::ops::search::SearchDirection;
        match direction {
            SearchDirection::Forward => self.search_forward(&matcher),
            SearchDirection::Reverse => self.search_reverse(&matcher),
        }
    }

    fn search_forward(&mut self, matcher: &CaseInsensitiveQuery<'_>) -> Option<Position> {
        let start = self.cursor;
        let mut position = self.next_search_position(start)?;
        loop {
            match self.search_value_at(position) {
                Some(value) if matcher.matches(&value) => return Some(position),
                Some(_) => position = self.next_search_position(position)?,
                None if !self.index_more_for_search() => break,
                None => {}
            }
        }
        std::iter::successors(Some(Position::default()), |position| {
            self.next_search_position(*position)
        })
        .take_while(|position| *position != start)
        .find(|position| self.search_cell_matches(*position, matcher))
    }

    fn search_reverse(&mut self, matcher: &CaseInsensitiveQuery<'_>) -> Option<Position> {
        let start = self.cursor;
        let preceding = std::iter::successors(self.previous_search_position(start), |position| {
            self.previous_search_position(*position)
        });
        if let Some(position) = preceding
            .into_iter()
            .find(|position| self.search_cell_matches(*position, matcher))
        {
            return Some(position);
        }
        while !matches!(self.row_count_state(), RowCount::Exact(_)) {
            if !self.index_more_for_search() {
                break;
            }
        }
        std::iter::successors(self.last_search_position(), |position| {
            self.previous_search_position(*position)
        })
        .take_while(|position| *position != start)
        .find(|position| self.search_cell_matches(*position, matcher))
    }

    fn search_cell_matches(&self, position: Position, matcher: &CaseInsensitiveQuery<'_>) -> bool {
        self.search_value_at(position)
            .is_some_and(|value| matcher.matches(&value))
    }

    fn index_more_for_search(&mut self) -> bool {
        let previous = self.rows.len();
        let _ = self.ensure_source_indexed_through(previous.saturating_add(255));
        self.rows.len() != previous
    }

    pub fn progressive_skip_to_change(
        &mut self,
        axis: crate::ops::skip::Axis,
        direction: crate::ops::skip::Direction,
        count: usize,
    ) -> Position {
        let mut position = self.cursor;
        for _ in 0..count.max(1) {
            let Some(start_value) = self.search_value_at(position) else {
                break;
            };
            let mut candidate = position;
            while let Some(next) = self.advance_skip_position(candidate, axis, direction) {
                candidate = next;
                let Some(value) = self.search_value_at(candidate) else {
                    break;
                };
                if value != start_value {
                    position = candidate;
                    break;
                }
            }
        }
        position
    }

    fn advance_skip_position(
        &mut self,
        mut position: Position,
        axis: crate::ops::skip::Axis,
        direction: crate::ops::skip::Direction,
    ) -> Option<Position> {
        use crate::ops::skip::{Axis, Direction};
        match (axis, direction) {
            (Axis::Row, Direction::Forward) => {
                position.row = position.row.saturating_add(1);
                let _ = self.ensure_source_indexed_through(position.row);
            }
            (Axis::Row, Direction::Backward) => position.row = position.row.checked_sub(1)?,
            (Axis::Column, Direction::Forward) => {
                position.column = position.column.saturating_add(1)
            }
            (Axis::Column, Direction::Backward) => {
                position.column = position.column.checked_sub(1)?
            }
        }
        Some(position)
    }

    fn search_value_at(&self, position: Position) -> Option<String> {
        let row_index = *self.visible_rows.get(position.row)?;
        let row = self.rows.get(row_index)?;
        let source_column = self.source_column_for_visible(position.column)?;
        Some(self.render_source_cell(source_column, row.get(source_column).map(String::as_str)))
    }

    fn next_search_position(&self, position: Position) -> Option<Position> {
        if position.column + 1 < self.column_count() {
            Some(Position {
                row: position.row,
                column: position.column + 1,
            })
        } else {
            Some(Position {
                row: position.row.checked_add(1)?,
                column: 0,
            })
        }
    }

    fn previous_search_position(&self, position: Position) -> Option<Position> {
        if position.column > 0 {
            Some(Position {
                row: position.row,
                column: position.column - 1,
            })
        } else {
            let row = position.row.checked_sub(1)?;
            Some(Position {
                row,
                column: self.column_count().saturating_sub(1),
            })
        }
    }

    fn last_search_position(&self) -> Option<Position> {
        (!self.rows.is_empty() && self.column_count() > 0).then_some(Position {
            row: self.rows.len() - 1,
            column: self.column_count() - 1,
        })
    }

    pub fn search_rows_vec(&self) -> Vec<Vec<String>> {
        self.visible_rows()
            .map(|row| {
                self.visible_source_columns()
                    .into_iter()
                    .map(|source_column| {
                        let raw = row
                            .get(source_column)
                            .map(String::as_str)
                            .unwrap_or_default();
                        let rendered = self.render_source_cell(source_column, Some(raw));
                        if rendered == raw {
                            raw.to_owned()
                        } else {
                            format!("{raw}\n{rendered}")
                        }
                    })
                    .collect()
            })
            .collect()
    }

    pub fn current_cell_matches(&self, query: &str) -> bool {
        let Some(source_column) = self.source_column_for_visible(self.cursor.column) else {
            return false;
        };
        let Some(raw) = self
            .visible_row(self.cursor.row)
            .and_then(|row| row.get(source_column).map(String::as_str))
        else {
            return false;
        };
        let Some(query) = CaseInsensitiveQuery::new(query) else {
            return false;
        };
        if query.matches(raw) {
            return true;
        }
        let rendered = self.render_source_cell(source_column, Some(raw));
        self.search_matches_cell(raw, &rendered, Some(&query))
    }

    #[cfg(all(test, feature = "saved-views"))]
    pub(super) fn visible_cell_matches_search_query(
        &self,
        row: usize,
        visible_column: usize,
        query: &str,
    ) -> bool {
        let Some(rendered_row) = self.rendered_visible_row(row) else {
            return false;
        };
        let Some(rendered) = rendered_row.get(visible_column) else {
            return false;
        };
        let Some(query) = CaseInsensitiveQuery::new(query) else {
            return false;
        };
        self.visible_cell_style_context(
            Position {
                row,
                column: visible_column,
            },
            rendered,
            Some(&query),
        )
        .search_match
    }

    pub(super) fn search_matches_cell(
        &self,
        raw: &str,
        rendered: &str,
        query: Option<&CaseInsensitiveQuery<'_>>,
    ) -> bool {
        let Some(query) = query else {
            return false;
        };
        if rendered == raw {
            query.matches(raw)
        } else {
            query.matches(raw) || query.matches(rendered)
        }
    }
}
