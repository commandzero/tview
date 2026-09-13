use super::*;

/// Applies file source filters and limits on demand for direct previews.
pub(crate) struct PreviewSourceStore {
    base: Box<dyn TableStore>,
    definition: TableDefinition,
    query: SourceQuery,
    requests: Vec<crate::ingest::SourceFilterRequest>,
    rows: Vec<Row>,
    next: usize,
    complete: bool,
}

impl PreviewSourceStore {
    pub(crate) fn new(
        base: Box<dyn TableStore>,
        definition: TableDefinition,
        mut query: SourceQuery,
        requests: Vec<crate::ingest::SourceFilterRequest>,
    ) -> anyhow::Result<Self> {
        query.filters.retain(|filter| !matches!(filter.scope, SourceFilterScope::Column(column) if column.ordinal == u32::MAX));
        validate_source_query(&definition, &query)?;
        for request in &requests {
            SourceFilter {
                scope: SourceFilterScope::WholeRecord,
                operator: request.operator,
                operand: request.operand.clone(),
            }
            .validate()?;
        }
        if !query.order_by.is_empty() {
            anyhow::bail!(
                "source sorting is unavailable for streaming delimited and structured files"
            );
        }
        Ok(Self {
            base,
            definition,
            query,
            requests,
            rows: Vec::new(),
            next: 0,
            complete: false,
        })
    }
}

impl TableStore for PreviewSourceStore {
    fn present_columns(&self, row: RowId) -> Option<Vec<usize>> {
        self.base.present_columns(row)
    }

    fn generation(&self) -> SourceGeneration {
        self.definition.generation
    }
    fn column_count(&self) -> usize {
        self.definition.columns.len()
    }
    fn row_count(&self) -> RowCount {
        if self.complete {
            RowCount::Exact(self.rows.len())
        } else if self.requests.is_empty() {
            match self.base.row_count() {
                RowCount::Exact(count) => RowCount::Exact(count.min(self.query.limit.get())),
                _ => RowCount::AtLeast(self.rows.len()),
            }
        } else {
            RowCount::AtLeast(self.rows.len())
        }
    }
    fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
        self.ensure_indexed_through(index)?;
        Ok(self.rows.get(index.0).cloned())
    }
    fn ensure_indexed_through(&mut self, index: RowIndex) -> anyhow::Result<IndexProgress> {
        let mut delta = SchemaDelta::default();
        let mut bytes_scanned = 0;
        while !self.complete && self.rows.len() <= index.0 {
            if self.rows.len() == self.query.limit.get() {
                self.complete = true;
                break;
            }
            let progress = self.base.ensure_indexed_through(RowIndex(self.next))?;
            self.definition.apply_delta(progress.schema_delta.clone())?;
            delta
                .added_columns
                .extend(progress.schema_delta.added_columns);
            delta
                .widened_types
                .extend(progress.schema_delta.widened_types);
            bytes_scanned += progress.bytes_scanned;
            let Some(row) = self.base.row(RowIndex(self.next))? else {
                for request in &self.requests {
                    anyhow::ensure!(
                        request.column == "*"
                            || self
                                .definition
                                .columns
                                .iter()
                                .enumerate()
                                .any(|(index, column)| {
                                    column.display_name == request.column
                                        || self.definition.canonical_column_key(index).as_deref()
                                            == Some(&request.column)
                                }),
                        "source operation column '{}' was not found",
                        request.column
                    );
                }
                self.complete = true;
                break;
            };
            self.next += 1;
            let mut matches = true;
            for request in &self.requests {
                let scope = if request.column == "*" {
                    SourceFilterScope::WholeRecord
                } else {
                    let canonical = self
                        .definition
                        .columns
                        .iter()
                        .enumerate()
                        .find(|(index, _)| {
                            self.definition.canonical_column_key(*index).as_deref()
                                == Some(&request.column)
                        })
                        .map(|(_, column)| column.id);
                    let column = if let Some(column) = canonical {
                        column
                    } else {
                        let columns = self
                            .definition
                            .columns
                            .iter()
                            .filter(|column| column.display_name == request.column)
                            .collect::<Vec<_>>();
                        anyhow::ensure!(
                            columns.len() <= 1,
                            "source operation column '{}' is ambiguous",
                            request.column
                        );
                        columns.first().map_or(
                            ColumnId {
                                generation: self.generation(),
                                ordinal: u32::MAX,
                            },
                            |column| column.id,
                        )
                    };
                    SourceFilterScope::Column(column)
                };
                let filter = SourceFilter {
                    scope,
                    operator: request.operator,
                    operand: request.operand.clone(),
                };
                matches &= file_source_filter_matches(&filter, &row);
            }
            if matches {
                self.rows.push(row);
            }
        }
        delta.completed = self.complete;
        Ok(IndexProgress {
            row_count: self.row_count(),
            schema_delta: delta,
            bytes_scanned,
        })
    }
    fn scan_rows(
        &mut self,
        request: ScanRequest,
        visitor: &mut dyn RowVisitor,
    ) -> anyhow::Result<ScanProgress> {
        let mut next = Some(request.start);
        let mut visited = 0;
        while visited < request.max_rows {
            let Some(index) = next else {
                break;
            };
            let Some(row) = self.row(index)? else {
                next = None;
                break;
            };
            visited += 1;
            next = match request.direction {
                ScanDirection::Forward => index.0.checked_add(1).map(RowIndex),
                ScanDirection::Reverse => index.0.checked_sub(1).map(RowIndex),
            };
            if visitor.visit(index, &row).is_break() {
                break;
            }
        }
        Ok(ScanProgress {
            visited,
            next,
            reached_end: next.is_none(),
        })
    }
    fn materialize(&mut self) -> anyhow::Result<InMemoryTable> {
        self.ensure_indexed_through(RowIndex(usize::MAX))?;
        InMemoryTable::from_rows(self.generation(), self.rows.clone())
    }
    fn active_source_query(&self) -> Option<&SourceQuery> {
        Some(&self.query)
    }
}
