use super::TableView;
use crate::table::{RowCount, TableStore};

impl TableView {
    pub fn active_source_query(&self) -> Option<&crate::table::SourceQuery> {
        self.active_source_query.as_ref()
    }

    pub fn source_capabilities(&self) -> &crate::table::SourceOperationCapabilities {
        &self.source_capabilities
    }

    pub fn source_column_name_for_id(&self, id: crate::table::ColumnId) -> Option<String> {
        let definition = self.table_definition.as_ref()?;
        let index = definition
            .columns
            .iter()
            .position(|column| column.id == id)?;
        definition.canonical_column_key(index).or_else(|| {
            match &definition.columns[index].source_identity {
                crate::table::ColumnSourceIdentity::Delimited {
                    name: Some(name), ..
                } => (definition
                    .columns
                    .iter()
                    .filter(|column| {
                        matches!(&column.source_identity,
                            crate::table::ColumnSourceIdentity::Delimited { name: Some(other), .. }
                            if other == name)
                    })
                    .count()
                    == 1)
                    .then(|| name.clone()),
                crate::table::ColumnSourceIdentity::Delimited {
                    ordinal,
                    name: None,
                }
                | crate::table::ColumnSourceIdentity::Positional(ordinal) => {
                    Some(format!("column_{}", ordinal + 1))
                }
                _ => None,
            }
        })
    }

    pub fn source_operations_for_column(&self, column: crate::table::ColumnId) -> Vec<String> {
        let Some(query) = &self.active_source_query else {
            return Vec::new();
        };
        let mut operations = query
            .filters
            .iter()
            .filter(|filter| {
                matches!(
                    filter.scope,
                    crate::table::SourceFilterScope::Column(id) if id == column
                )
            })
            .map(|filter| format!("source filter: {}", filter.operator))
            .collect::<Vec<_>>();
        operations.extend(
            query
                .order_by
                .iter()
                .filter(|sort| sort.column == column)
                .map(|sort| format!("source sort: {:?}", sort.direction)),
        );
        operations
    }

    pub fn source_result_extent(&self) -> Option<crate::table::ResultExtent> {
        self.incremental_store
            .as_ref()
            .and_then(|store| store.0.borrow().result_extent())
            .or(self.source_result_extent)
    }

    pub fn source_result_is_partial(&self) -> bool {
        self.incremental_store
            .as_ref()
            .is_some_and(|store| store.0.borrow().result_is_partial())
            || self.source_result_is_partial
    }

    pub fn source_result_warnings(&self) -> &[String] {
        &self.source_result_warnings
    }

    pub fn source_field_catalog(&self) -> std::sync::Arc<[crate::table::SourceFieldMetadata]> {
        self.incremental_store
            .as_ref()
            .map(|store| store.0.borrow().source_field_catalog())
            .unwrap_or_default()
    }

    pub fn source_query_provenance(&self) -> Option<&crate::table::NativeQueryArtifact> {
        self.source_query_provenance.as_ref()
    }

    pub fn fetched_source_row_count(&self) -> usize {
        match self
            .incremental_store
            .as_ref()
            .map(|store| store.0.borrow().row_count())
            .or_else(|| self.source_store.as_ref().map(TableStore::row_count))
        {
            Some(RowCount::Exact(count) | RowCount::AtLeast(count)) => count,
            Some(RowCount::Unknown) | None => self.rows.len(),
        }
    }

    pub fn object_mode_resolution(&self) -> Option<crate::ingest::ObjectModeResolution> {
        self.object_mode
    }

    #[cfg(feature = "saved-views")]
    pub(super) fn source_column_name(&self, source_column: usize) -> String {
        if let Some(key) = self
            .table_definition
            .as_ref()
            .and_then(|definition| definition.canonical_column_key(source_column))
        {
            return key;
        }
        self.header
            .as_ref()
            .and_then(|header| header.get(source_column))
            .cloned()
            .unwrap_or_else(|| format!("column_{}", source_column + 1))
    }
}
