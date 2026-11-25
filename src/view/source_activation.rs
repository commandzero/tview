use super::{CandidateRestoration, PendingSourceResult, Position, TableView};
use crate::ingest::OpenedTable;

impl TableView {
    /// Transfer validated opening settings only after the initial result and view bind.
    pub fn initialize_source_configuration(
        &mut self,
        options: crate::ingest::OpenOptions,
    ) -> anyhow::Result<()> {
        let committed = self.resolve_source_configuration(options)?;
        self.committed_source = Some(committed);
        Ok(())
    }

    pub fn committed_open_options(&self) -> Option<&crate::ingest::OpenOptions> {
        self.committed_source.as_ref()
    }

    fn resolve_source_configuration(
        &self,
        mut options: crate::ingest::OpenOptions,
    ) -> anyhow::Result<crate::ingest::OpenOptions> {
        if let Some(query) = self.active_source_query.as_ref() {
            options.limit = (query.limit != std::num::NonZeroUsize::MAX).then_some(query.limit);
            options.source_filters = query
                .filters
                .iter()
                .map(|filter| {
                    let column = match filter.scope {
                        crate::table::SourceFilterScope::WholeRecord => "*".to_owned(),
                        crate::table::SourceFilterScope::Column(id) => {
                            self.source_column_name_for_id(id).ok_or_else(|| {
                                anyhow::anyhow!("source filter column has no durable key")
                            })?
                        }
                    };
                    Ok(crate::ingest::SourceFilterRequest {
                        column,
                        operator: filter.operator,
                        operand: filter.operand.clone(),
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            options.source_sort = query
                .order_by
                .iter()
                .map(|sort| {
                    Ok(crate::ingest::SourceSortRequest {
                        column: self.source_column_name_for_id(sort.column).ok_or_else(|| {
                            anyhow::anyhow!("source sort column has no durable key")
                        })?,
                        direction: sort.direction,
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let has_explicit_source = options.native_query.is_some() || options.table.is_some();
            if !has_explicit_source && self.source_query_provenance.is_some() {
                options.table = self
                    .table_definition
                    .as_ref()
                    .map(|definition| definition.relation.name.clone());
            }
        }
        Ok(options)
    }

    pub fn request_source_query(
        &mut self,
        query: crate::table::SourceQuery,
        native_query_changed: bool,
    ) -> bool {
        let Some(definition) = self.table_definition.as_ref() else {
            self.source_status = Some("Source queries require a store-backed table".to_owned());
            return false;
        };
        if let Err(error) = crate::table::validate_source_query(definition, &query) {
            self.source_status = Some(format!("Source query validation failed: {error}"));
            return false;
        }
        let Some(source) = self.incremental_store.as_ref() else {
            self.source_status = Some("Source query replacement is unavailable".to_owned());
            return false;
        };
        let task = match source.0.borrow_mut().source_query_task(query.clone()) {
            Ok(task) => task,
            Err(error) => {
                self.source_status = Some(error.to_string());
                return false;
            }
        };
        let (cursor_identity, mark_identity) = match (
            self.current_stable_row_identity(),
            self.mark_stable_row_identity(),
        ) {
            (Ok(cursor), Ok(mark)) => (cursor, mark),
            (Err(error), _) | (_, Err(error)) => {
                self.source_status = Some(format!("Source identity read failed: {error}"));
                return false;
            }
        };
        let mut options = self.committed_source.clone().unwrap_or_default();
        if native_query_changed {
            options.native_query = query.native_query.clone();
            options.table = None;
        }
        let revision = self.source_query_coordinator.borrow_mut().request(task);
        self.pending_source = Some(PendingSourceResult {
            revision,
            options,
            cursor_identity,
            mark_identity,
        });
        self.latest_activation_error = None;
        self.source_status = Some(format!("Source query revision {revision} is running"));
        true
    }

    pub fn source_query_is_pending(&self) -> bool {
        self.source_query_coordinator.borrow().is_pending()
    }

    pub fn poll_source_query(&mut self) -> bool {
        let event = self.source_query_coordinator.borrow_mut().poll();
        match event {
            Some(crate::table::SourceQueryCoordinatorEvent::Ready { revision, result }) => {
                let Some(pending) = self
                    .pending_source
                    .take()
                    .filter(|pending| pending.revision == revision)
                else {
                    return false;
                };
                match self.activate_source_replacement(*result, pending) {
                    Ok(()) => {
                        self.source_status =
                            Some(format!("Source query revision {revision} completed"));
                        true
                    }
                    Err(error) => {
                        self.record_activation_failure(error.to_string());
                        false
                    }
                }
            }
            Some(crate::table::SourceQueryCoordinatorEvent::Failed { revision, error }) => {
                if self
                    .pending_source
                    .as_ref()
                    .is_some_and(|pending| pending.revision == revision)
                {
                    self.pending_source = None;
                    self.record_activation_failure(error);
                }
                false
            }
            None => false,
        }
    }

    fn record_activation_failure(&mut self, error: String) {
        self.source_status = Some(format!(
            "Source query failed; prior result retained: {error}"
        ));
        self.latest_activation_error = Some(error);
    }

    pub fn await_latest_source_query(&mut self) -> anyhow::Result<()> {
        while self.source_query_is_pending() {
            self.poll_source_query();
            if self.source_query_is_pending() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        if let Some(error) = &self.latest_activation_error {
            anyhow::bail!("latest source activation failed: {error}");
        }
        Ok(())
    }

    pub fn reload_committed_source(
        &mut self,
        source: &crate::ingest::source::InputSource,
    ) -> anyhow::Result<()> {
        if source.is_stdin() {
            return Ok(());
        }
        let options = self
            .committed_source
            .clone()
            .ok_or_else(|| anyhow::anyhow!("source configuration is not committed"))?;
        let revision = self.source_query_coordinator.borrow_mut().supersede();
        self.pending_source = None;
        let cursor_row = self.cursor.row;
        let result = (|| {
            let opened =
                crate::ingest::open_source(source.clone(), &options)?.into_implicit_table()?;
            let next = self.prepare_opened_candidate(
                opened,
                options,
                CandidateRestoration::Reload { cursor_row },
            )?;
            self.commit_candidate(next, revision)
        })();
        if let Err(error) = &result {
            self.record_activation_failure(error.to_string());
        }
        result
    }

    fn activate_source_replacement(
        &mut self,
        replacement: crate::table::SourceResult,
        pending: PendingSourceResult,
    ) -> anyhow::Result<()> {
        let opened = crate::ingest::OpenedTable {
            generation: replacement.definition.generation,
            definition: replacement.definition,
            store: replacement.store,
            object_mode: self.object_mode,
            warnings: replacement.metadata.warnings,
        };
        let next = self.prepare_opened_candidate(
            opened,
            pending.options,
            CandidateRestoration::Stable {
                cursor_identity: pending.cursor_identity,
                mark_identity: pending.mark_identity,
            },
        )?;
        self.commit_candidate(next, pending.revision)
    }

    fn prepare_opened_candidate(
        &self,
        opened: OpenedTable,
        options: crate::ingest::OpenOptions,
        restoration: CandidateRestoration,
    ) -> anyhow::Result<Self> {
        let mut next = Self::from_opened_table(opened, self.viewport)?;
        next.restore_view_settings_from(self)?;
        match restoration {
            CandidateRestoration::Stable {
                cursor_identity,
                mark_identity,
            } => {
                next.remap_mark_column(self);
                next.restore_stable_identities(cursor_identity, mark_identity)?;
            }
            CandidateRestoration::Reload { cursor_row } => {
                next.goto(cursor_row, next.cursor.column);
            }
        }
        next.initialize_source_configuration(options)?;
        Ok(next)
    }

    fn remap_mark_column(&mut self, previous: &Self) {
        let Some((visible_column, new_column)) = self.remapped_mark_column(previous) else {
            return;
        };
        self.mark_identity = self.row_ids.first().map(|id| (*id, new_column));
        self.mark = self.mark_identity.map(|_| Position {
            row: 0,
            column: visible_column,
        });
    }

    fn remapped_mark_column(&self, previous: &Self) -> Option<(usize, crate::table::ColumnId)> {
        let (_, column_id) = previous.mark_identity?;
        let old = previous.table_definition.as_ref()?;
        let new = self.table_definition.as_ref()?;
        let old_index = old
            .columns
            .iter()
            .position(|column| column.id == column_id)?;
        let new_index = super::restore::matching_column(old, new, old_index)?;
        let visible = self.visible_column_for_source(new_index)?;
        Some((visible, new.columns[new_index].id))
    }

    fn commit_candidate(&mut self, mut next: Self, revision: u64) -> anyhow::Result<()> {
        if self.source_query_coordinator.borrow().latest_revision() != revision {
            anyhow::bail!("source replacement was superseded during preparation");
        }
        next.source_query_coordinator = self.source_query_coordinator.clone();
        next.latest_activation_error = None;
        *self = next;
        Ok(())
    }
}
