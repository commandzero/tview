use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use super::{TableView, Viewport};
use crate::ingest::{OpenOptions, OpenedTable};
use crate::ops::filter::{FilterKind, FilterMode};
use crate::table::{
    CellValue, ColumnDefinition, ColumnId, ColumnSourceIdentity, InMemoryTable, IndexProgress,
    LogicalType, RelationMetadata, Row, RowCount, RowId, RowIndex, ScanProgress, ScanRequest,
    SchemaState, SourceGeneration, SourceQuery, SourceQueryTask, SourceResult, StableRowIdentity,
    TableDefinition, TableStore, TypeOrigin,
};

#[derive(Clone, Copy)]
enum Failure {
    None,
    Loading,
    Reconstruction,
    IdentityRead,
    NoIdentity,
}

struct Step {
    failure: Failure,
    values: Vec<String>,
}

type Script = Arc<Mutex<VecDeque<Step>>>;

struct ControlledStore {
    inner: InMemoryTable,
    failure: Failure,
    active_query: Option<SourceQuery>,
    script: Option<Script>,
}

impl TableStore for ControlledStore {
    fn generation(&self) -> SourceGeneration {
        self.inner.generation()
    }

    fn row_count(&self) -> RowCount {
        self.inner.row_count()
    }

    fn column_count(&self) -> usize {
        self.inner.column_count()
    }

    fn row(&mut self, index: RowIndex) -> anyhow::Result<Option<Row>> {
        self.inner.row(index)
    }

    fn ensure_indexed_through(&mut self, index: RowIndex) -> anyhow::Result<IndexProgress> {
        if matches!(self.failure, Failure::Loading) {
            anyhow::bail!("controlled loading failure");
        }
        self.inner.ensure_indexed_through(index)
    }

    fn scan_rows(
        &mut self,
        request: ScanRequest,
        visitor: &mut dyn crate::table::RowVisitor,
    ) -> anyhow::Result<ScanProgress> {
        self.inner.scan_rows(request, visitor)
    }

    fn materialize(&mut self) -> anyhow::Result<InMemoryTable> {
        if matches!(self.failure, Failure::Reconstruction) {
            anyhow::bail!("controlled reconstruction failure");
        }
        self.inner.materialize()
    }

    fn active_source_query(&self) -> Option<&SourceQuery> {
        self.active_query.as_ref()
    }

    fn source_query_task(&mut self, query: SourceQuery) -> anyhow::Result<SourceQueryTask> {
        let script = self
            .script
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no replacement script"))?;
        let step = script
            .lock()
            .expect("script lock")
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("replacement script exhausted"))?;
        let script = script.clone();
        Ok(SourceQueryTask::Blocking(Box::new(move || {
            let values = step.values.iter().map(String::as_str).collect::<Vec<_>>();
            let next = opened_with_script(&values, step.failure, Some(script), Some(query.limit));
            Ok(SourceResult::from_store(next.definition, next.store))
        })))
    }

    fn stable_row_identity(
        &mut self,
        index: RowIndex,
    ) -> anyhow::Result<Option<StableRowIdentity>> {
        if matches!(self.failure, Failure::IdentityRead) {
            anyhow::bail!("controlled identity read failure");
        }
        if matches!(self.failure, Failure::NoIdentity) {
            return Ok(None);
        }
        Ok(self
            .inner
            .row(index)?
            .and_then(|row| row.cells.into_iter().next())
            .map(|value| StableRowIdentity::PrimaryKey(vec![value])))
    }
}

fn opened_with_script(
    values: &[&str],
    failure: Failure,
    script: Option<Script>,
    limit: Option<NonZeroUsize>,
) -> OpenedTable {
    let generation = SourceGeneration::new();
    let rows = values
        .iter()
        .enumerate()
        .map(|(ordinal, value)| {
            Row::new(
                RowId {
                    generation,
                    ordinal: ordinal as u64,
                },
                vec![CellValue::Text((*value).to_owned())],
            )
        })
        .collect();
    let inner = InMemoryTable::from_rows(generation, rows).expect("in-memory source");
    let definition = TableDefinition {
        generation,
        columns: vec![ColumnDefinition {
            id: ColumnId {
                generation,
                ordinal: 0,
            },
            source_identity: ColumnSourceIdentity::RelationColumn {
                relation: "events".to_owned(),
                ordinal: 0,
                name: "name".to_owned(),
            },
            display_name: "name".to_owned(),
            source_declared_type: None,
            source_type: LogicalType::Text,
            type_origin: TypeOrigin::Declared,
        }],
        schema_state: SchemaState::Complete,
        relation: RelationMetadata::implicit("events", true),
    };
    OpenedTable {
        generation,
        definition,
        store: Box::new(ControlledStore {
            inner,
            failure,
            active_query: limit.map(|limit| SourceQuery::new(generation, limit)),
            script,
        }),
        object_mode: None,
        warnings: Vec::new(),
    }
}

fn active(step: Step) -> TableView {
    let mut view = TableView::from_opened_table(
        opened_with_script(
            &["before", "other"],
            Failure::None,
            Some(Arc::new(Mutex::new(VecDeque::from([step])))),
            None,
        ),
        Viewport::new(2, 1),
    )
    .expect("active source");
    view.initialize_source_configuration(OpenOptions::default())
        .expect("committed source");
    view
}

#[test]
fn proven_absence_resets_row_bound_state_without_activation_error() {
    let mut view = active(Step {
        failure: Failure::NoIdentity,
        values: vec!["other".to_owned(), "before".to_owned()],
    });
    view.set_mark();
    let query = SourceQuery::new(
        view.source_generation().expect("generation"),
        NonZeroUsize::new(2).unwrap(),
    );
    assert!(view.request_source_query(query, false));
    view.await_latest_source_query()
        .expect("absence is not a store error");
    assert_eq!(view.cursor().row, 0);
    assert!(view.mark().is_none());
}

#[test]
fn unique_stable_rows_follow_a_reordered_candidate() {
    let mut view = active(Step {
        failure: Failure::None,
        values: vec!["other".to_owned(), "before".to_owned()],
    });
    view.set_mark();
    let query = SourceQuery::new(
        view.source_generation().expect("generation"),
        NonZeroUsize::new(2).unwrap(),
    );
    assert!(view.request_source_query(query, false));
    view.await_latest_source_query()
        .expect("compatible candidate");
    assert_eq!(view.cursor().row, 1);
    assert_eq!(view.mark().expect("mark follows identity").row, 1);
}

#[test]
fn duplicate_stable_row_identity_resets_cursor_and_mark() {
    let mut view = active(Step {
        failure: Failure::None,
        values: vec!["before".to_owned(), "before".to_owned()],
    });
    view.set_mark();
    let query = SourceQuery::new(
        view.source_generation().expect("generation"),
        NonZeroUsize::new(2).unwrap(),
    );
    assert!(view.request_source_query(query, false));
    view.await_latest_source_query()
        .expect("candidate is valid but row identity is ambiguous");
    assert_eq!(view.cursor().row, 0);
    assert!(view.mark().is_none());
}

#[test]
fn failed_reload_retains_committed_result_and_latest_failure_until_success() {
    let dir = tempfile::tempdir().expect("source directory");
    let path = dir.path().join("rows.csv");
    std::fs::write(&path, "name\nbefore\n").expect("source");
    let source = crate::ingest::source::InputSource::Path(path.clone());
    let options = OpenOptions::default();
    let opened = crate::ingest::open_source(source.clone(), &options)
        .expect("open source")
        .into_implicit_table()
        .expect("table");
    let mut view =
        TableView::from_opened_table(opened, Viewport::new(4, 1)).expect("active result");
    view.initialize_source_configuration(options)
        .expect("committed source");
    let committed = view.committed_open_options().cloned();
    let generation = view.source_generation();
    let rows = view.visible_rows_vec();

    std::fs::remove_file(&path).expect("remove source");
    view.reload_committed_source(&source)
        .expect_err("reload opening fails");
    assert_eq!(view.committed_open_options(), committed.as_ref());
    assert_eq!(view.source_generation(), generation);
    assert_eq!(view.visible_rows_vec(), rows);
    let _ = view.take_source_status();
    assert!(
        view.await_latest_source_query().is_err(),
        "latest failure survives status consumption"
    );

    std::fs::write(&path, "name\nafter\n").expect("restore source");
    view.reload_committed_source(&source)
        .expect("new reload succeeds");
    assert_ne!(view.source_generation(), generation);
    assert!(view.await_latest_source_query().is_ok());
    assert_eq!(view.visible_rows_vec(), vec![vec!["after".to_owned()]]);
}

#[test]
fn latest_candidate_failures_survive_status_consumption_until_a_new_revision_activates() {
    for (failure, diagnostic) in [
        (Failure::Loading, "controlled loading failure"),
        (Failure::Reconstruction, "controlled reconstruction failure"),
        (Failure::IdentityRead, "controlled identity read failure"),
    ] {
        let script = Arc::new(Mutex::new(VecDeque::from([
            Step {
                failure,
                values: vec!["before".to_owned(), "changed".to_owned()],
            },
            Step {
                failure: Failure::None,
                values: vec!["before".to_owned(), "changed".to_owned()],
            },
        ])));
        let opened = opened_with_script(
            &["before", "other"],
            Failure::None,
            Some(script),
            NonZeroUsize::new(10),
        );
        let mut view = TableView::from_opened_table(opened, Viewport::new(2, 1))
            .expect("active scripted source");
        view.initialize_source_configuration(OpenOptions::default())
            .expect("committed configuration");
        if matches!(failure, Failure::Reconstruction) {
            view.apply_filter(0, FilterMode::In, FilterKind::Text, "before".to_owned())
                .expect("local filter");
        }
        if matches!(failure, Failure::IdentityRead) {
            view.set_mark();
        }
        let rows = view.visible_rows_vec();
        let generation = view.source_generation();
        let committed_options = view.committed_open_options().cloned();
        #[cfg(feature = "saved-views")]
        let yaml = view.to_saved_view_yaml("controlled", "fixture.csv", None);
        #[cfg(feature = "saved-views")]
        assert!(yaml.contains("limit: 10"));

        let mut failed_query = view.active_source_query().expect("active query").clone();
        failed_query.limit = NonZeroUsize::new(3).expect("nonzero");
        assert!(view.request_source_query(failed_query, false));
        assert_eq!(view.committed_open_options(), committed_options.as_ref());
        #[cfg(feature = "saved-views")]
        assert_eq!(
            view.to_saved_view_yaml("controlled", "fixture.csv", None),
            yaml
        );
        let error = view
            .await_latest_source_query()
            .expect_err("candidate activation fails");
        assert!(error.to_string().contains(diagnostic), "{error}");
        assert!(view
            .take_source_status()
            .expect("TUI diagnostic")
            .contains(diagnostic));
        assert!(
            view.await_latest_source_query().is_err(),
            "status consumption cannot clear latest outcome"
        );
        assert_eq!(view.visible_rows_vec(), rows);
        assert_eq!(view.source_generation(), generation);
        assert_eq!(view.committed_open_options(), committed_options.as_ref());
        #[cfg(feature = "saved-views")]
        assert_eq!(
            view.to_saved_view_yaml("controlled", "fixture.csv", None),
            yaml
        );

        let mut succeeding_query = view
            .active_source_query()
            .expect("prior query retained")
            .clone();
        succeeding_query.limit = NonZeroUsize::new(5).expect("nonzero");
        assert!(view.request_source_query(succeeding_query, false));
        view.await_latest_source_query()
            .expect("newer candidate activates");
        assert_ne!(view.source_generation(), generation);
        let expected = if matches!(failure, Failure::Reconstruction) {
            vec![vec!["before".to_owned()]]
        } else {
            vec![vec!["before".to_owned()], vec!["changed".to_owned()]]
        };
        assert_eq!(
            view.visible_rows_vec(),
            expected,
            "successful activation publishes new rows and retained local filters"
        );
        assert_eq!(
            view.active_source_query()
                .expect("committed query")
                .limit
                .get(),
            5
        );
        #[cfg(feature = "saved-views")]
        {
            let committed = view.to_saved_view_yaml("controlled", "fixture.csv", None);
            assert!(committed.contains("limit: 5"), "{committed}");
            assert!(!committed.contains("limit: 3"));
        }
        assert!(view.await_latest_source_query().is_ok());
    }
}

#[test]
fn reload_retains_identical_positional_column_settings() {
    for (filename, contents) in [
        ("headerless.csv", "1,10\n2,2\n3,12\n"),
        ("positional.json", "[[1,10],[2,2],[3,12]]"),
    ] {
        let directory = tempfile::tempdir().expect("source directory");
        let path = directory.path().join(filename);
        std::fs::write(&path, contents).expect("source");
        let source = crate::ingest::source::InputSource::Path(path);
        let options = OpenOptions::default();
        let opened = crate::ingest::open_source(source.clone(), &options)
            .expect("open source")
            .into_implicit_table()
            .expect("table");
        let mut view = TableView::from_opened_table(opened, Viewport::new(8, 80)).expect("view");
        view.initialize_source_configuration(options)
            .expect("committed source");
        view.apply_filter(0, FilterMode::In, FilterKind::Numeric, ">1".to_owned())
            .expect("numeric filter");
        view.goto(0, 1);
        view.sort_current_column(
            crate::ops::sort::SortMode::Numeric,
            crate::ops::sort::SortDirection::Descending,
        );
        view.set_current_column_width(17);
        view.goto(0, 0);
        view.hide_current_column();
        let expected = vec![vec!["12".to_owned()], vec!["2".to_owned()]];
        assert_eq!(view.visible_rows_vec(), expected);
        assert_eq!(view.effective_column_widths(), vec![17]);

        view.reload_committed_source(&source).expect("reload");

        assert_eq!(
            view.visible_rows_vec(),
            expected,
            "{filename}: hidden columns, filters and sort survive exact positional identity"
        );
        assert_eq!(
            view.effective_column_widths(),
            vec![17],
            "{filename}: explicit width survives"
        );
    }
}

#[cfg(feature = "saved-views")]
#[test]
fn source_bound_text_filters_match_masked_presentation() {
    let directory = tempfile::tempdir().expect("source directory");
    let path = directory.path().join("amounts.csv");
    std::fs::write(&path, "amount\n1000\n2000\n").expect("source");
    let source = crate::ingest::source::InputSource::Path(path);
    let options = OpenOptions::default();
    let opened = crate::ingest::open_source(source, &options)
        .expect("source")
        .into_implicit_table()
        .expect("table");
    let mut view = TableView::from_opened_table(opened, Viewport::new(8, 80)).expect("view");
    let saved = crate::saved_views::parse_saved_view_yaml(
        "name: masked\nfilenames: []\nsource: {}\nview:\n  locale: en_US\n  columns:\n    amount: {type: number, format: mask, mask: '#,##0'}\n",
    ).expect("saved view");
    view.install_saved_binding(saved.view.view, true);
    assert_eq!(
        view.visible_rows_vec(),
        vec![vec!["1,000".to_owned()], vec!["2,000".to_owned()]]
    );

    view.apply_filter(0, FilterMode::In, FilterKind::Text, "1,000".to_owned())
        .expect("text filter");

    assert_eq!(view.visible_rows_vec(), vec![vec!["1,000".to_owned()]]);
    assert_eq!(view.visible_raw_rows_vec(), vec![vec!["1000".to_owned()]]);
}
