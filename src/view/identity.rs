use super::TableView;
use crate::table::{
    ColumnDefinition, ColumnSourceIdentity, LogicalType, RowIndex, StableRowIdentity,
};

impl TableView {
    pub(super) fn current_stable_row_identity(&self) -> anyhow::Result<Option<StableRowIdentity>> {
        let row = self
            .visible_rows
            .get(self.cursor.row)
            .and_then(|row| self.row_ids.get(*row))
            .copied();
        self.stable_row_identity(row)
    }

    pub(super) fn mark_stable_row_identity(&self) -> anyhow::Result<Option<StableRowIdentity>> {
        self.stable_row_identity(self.mark_identity.map(|(row, _)| row))
    }

    fn stable_row_identity(
        &self,
        row: Option<crate::table::RowId>,
    ) -> anyhow::Result<Option<StableRowIdentity>> {
        let Some((row, store)) = row.zip(self.incremental_store.as_ref()) else {
            return Ok(None);
        };
        store
            .0
            .borrow_mut()
            .stable_row_identity(RowIndex(row.ordinal as usize))
    }

    pub(super) fn restore_stable_identities(
        &mut self,
        cursor_identity: Option<crate::table::StableRowIdentity>,
        mark_identity: Option<crate::table::StableRowIdentity>,
    ) -> anyhow::Result<()> {
        if cursor_identity.is_none() && mark_identity.is_none() {
            self.cursor.row = 0;
            self.mark = None;
            self.mark_identity = None;
            return Ok(());
        }
        let mut cursor_match = StableRowMatch::new(cursor_identity.as_ref());
        let mut mark_match = StableRowMatch::new(mark_identity.as_ref());
        self.scan_stable_rows(&mut cursor_match, &mut mark_match)?;
        self.cursor.row = cursor_match.unique_position().unwrap_or(0);
        self.restore_mark_row(mark_match.unique_position());
        self.keep_cursor_visible();
        Ok(())
    }

    fn scan_stable_rows(
        &mut self,
        cursor_match: &mut StableRowMatch<'_>,
        mark_match: &mut StableRowMatch<'_>,
    ) -> anyhow::Result<()> {
        let Some(shared) = self.incremental_store.clone() else {
            return Ok(());
        };
        let mut store = shared.0.borrow_mut();
        let transformed = self.view_transform_is_active();
        let previous_len = self.rows.len();
        let visible_positions = transformed.then(|| {
            self.row_ids
                .iter()
                .copied()
                .enumerate()
                .map(|(position, id)| (id, position))
                .collect::<std::collections::HashMap<_, _>>()
        });

        let mut index = 0;
        while let Some(source_row) = store.row(RowIndex(index))? {
            let identity = store.stable_row_identity(RowIndex(index))?;
            let position = match &visible_positions {
                Some(positions) => positions.get(&source_row.id).copied(),
                None => Some(index),
            };
            cursor_match.observe(identity.as_ref(), position);
            mark_match.observe(identity.as_ref(), position);
            if !transformed && index >= self.row_ids.len() {
                self.append_source_row(source_row);
            }
            index += 1;
        }
        if transformed {
            return Ok(());
        }
        self.visible_rows = (0..self.rows.len()).collect();
        if self.rows.len() != previous_len {
            self.refresh_resident_color_facts_on_append();
        }
        Ok(())
    }

    fn restore_mark_row(&mut self, row: Option<usize>) {
        let Some((row, (_, column_id))) = row.zip(self.mark_identity) else {
            self.mark = None;
            self.mark_identity = None;
            return;
        };
        self.mark_identity = Some((self.row_ids[row], column_id));
        if let Some(mark) = &mut self.mark {
            mark.row = row;
        }
    }
}

struct StableRowMatch<'a> {
    wanted: Option<&'a StableRowIdentity>,
    count: usize,
    position: Option<usize>,
}

impl<'a> StableRowMatch<'a> {
    fn new(wanted: Option<&'a StableRowIdentity>) -> Self {
        Self {
            wanted,
            count: 0,
            position: None,
        }
    }

    fn observe(&mut self, identity: Option<&StableRowIdentity>, position: Option<usize>) {
        if identity.is_some_and(|identity| self.wanted == Some(identity)) {
            self.count += 1;
            self.position = position;
        }
    }

    fn unique_position(&self) -> Option<usize> {
        (self.count == 1).then_some(self.position).flatten()
    }
}

pub(super) fn compatible_source_column(old: &ColumnDefinition, new: &ColumnDefinition) -> bool {
    let mismatched_declared_type = matches!(
        (&old.source_declared_type, &new.source_declared_type),
        (Some(left), Some(right)) if left != right
    );
    if mismatched_declared_type || !same_column_identity(&old.source_identity, &new.source_identity)
    {
        return false;
    }
    compatible_source_type(old.source_type, new.source_type)
}

fn same_column_identity(old: &ColumnSourceIdentity, new: &ColumnSourceIdentity) -> bool {
    match (old, new) {
        (
            ColumnSourceIdentity::RelationColumn {
                relation: left,
                name: left_name,
                ..
            },
            ColumnSourceIdentity::RelationColumn {
                relation: right,
                name: right_name,
                ..
            },
        ) => (left, left_name) == (right, right_name),
        (
            ColumnSourceIdentity::Delimited {
                ordinal: left,
                name: left_name,
            },
            ColumnSourceIdentity::Delimited {
                ordinal: right,
                name: right_name,
            },
        ) => (left, left_name) == (right, right_name),
        (left, right) => left == right,
    }
}

fn compatible_source_type(old: LogicalType, new: LogicalType) -> bool {
    use LogicalType::{Float, Integer, Null, Unknown};
    match (old, new) {
        (Unknown | Null, _) | (_, Unknown | Null) | (Integer, Float) | (Float, Integer) => true,
        (old, new) => old == new,
    }
}
