use std::fmt;
use std::num::NonZeroUsize;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc, Arc, OnceLock,
};

use super::{CellValue, ColumnId, SourceGeneration, SourceQueryTask, SourceResult};

#[cfg(feature = "sqlite")]
pub const DEFAULT_SQLITE_SOURCE_LIMIT: usize = 1_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NullPlacement {
    First,
    #[default]
    Last,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueDomain {
    Raw,
    Rendered,
    RawOrRendered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    In,
    Out,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ViewFilterPredicate {
    Text {
        value: String,
        domain: ValueDomain,
    },
    Regex {
        pattern: String,
        domain: ValueDomain,
    },
    Numeric {
        operator: NumericOperator,
        operand: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericOperator {
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    Equal,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ViewFilter {
    pub column: ColumnId,
    pub mode: FilterMode,
    pub predicate: ViewFilterPredicate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewSortMode {
    Lexical,
    Natural,
    Numeric,
    Date,
    SemanticVersion,
    Ip,
    Boolean,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewSort {
    pub column: ColumnId,
    pub mode: ViewSortMode,
    pub direction: SortDirection,
    pub nulls: NullPlacement,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ViewTransform {
    pub generation: SourceGeneration,
    pub filters: Vec<ViewFilter>,
    pub order_by: Vec<ViewSort>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SourceOperand {
    Null,
    Boolean(bool),
    Integer(i64),
    Float(f64),
    Text(String),
    Binary(Vec<u8>),
}

impl From<SourceOperand> for CellValue {
    fn from(value: SourceOperand) -> Self {
        match value {
            SourceOperand::Null => Self::Null,
            SourceOperand::Boolean(value) => Self::Boolean(value),
            SourceOperand::Integer(value) => Self::Integer(value),
            SourceOperand::Float(value) => Self::Float(value),
            SourceOperand::Text(value) => Self::Text(value),
            SourceOperand::Binary(value) => Self::Binary(value),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceFilterOperator {
    Equal,
    NotEqual,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    Contains,
    Prefix,
    IsNull,
    IsNotNull,
}

impl SourceFilterOperator {
    pub fn requires_operand(self) -> bool {
        !matches!(self, Self::IsNull | Self::IsNotNull)
    }
}

impl fmt::Display for SourceFilterOperator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Equal => "equal",
            Self::NotEqual => "not equal",
            Self::LessThan => "less than",
            Self::LessThanOrEqual => "less than or equal",
            Self::GreaterThan => "greater than",
            Self::GreaterThanOrEqual => "greater than or equal",
            Self::Contains => "contains",
            Self::Prefix => "prefix",
            Self::IsNull => "is null",
            Self::IsNotNull => "is not null",
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SourceFilterScope {
    WholeRecord,
    Column(ColumnId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceFilter {
    pub scope: SourceFilterScope,
    pub operator: SourceFilterOperator,
    pub operand: Option<SourceOperand>,
}

impl SourceFilter {
    pub fn validate(&self) -> Result<(), SourceQueryValidationError> {
        let valid_operand_shape = if self.operator.requires_operand() {
            matches!(self.operand, Some(ref operand) if !matches!(operand, SourceOperand::Null))
        } else {
            self.operand.is_none()
        };
        if !valid_operand_shape {
            return Err(SourceQueryValidationError::OperandMismatch {
                operator: self.operator,
            });
        }
        if matches!(
            self.operand.as_ref(),
            Some(SourceOperand::Float(value)) if !value.is_finite()
        ) {
            return Err(SourceQueryValidationError::NonFiniteOperand);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceSort {
    pub column: ColumnId,
    pub direction: SortDirection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceQuery {
    pub generation: SourceGeneration,
    pub native_query: Option<String>,
    pub filters: Vec<SourceFilter>,
    pub order_by: Vec<SourceSort>,
    pub limit: NonZeroUsize,
}

impl SourceQuery {
    pub fn new(generation: SourceGeneration, limit: NonZeroUsize) -> Self {
        Self {
            generation,
            native_query: None,
            filters: Vec::new(),
            order_by: Vec::new(),
            limit,
        }
    }

    #[cfg(feature = "sqlite")]
    pub fn sqlite_default(generation: SourceGeneration) -> Self {
        Self::new(
            generation,
            NonZeroUsize::new(DEFAULT_SQLITE_SOURCE_LIMIT).expect("non-zero SQLite default"),
        )
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum SourceQueryValidationError {
    #[error("source query belongs to a different source generation")]
    StaleGeneration,
    #[error("source query references an unknown or stale column")]
    UnknownColumn,
    #[error("source filter '{operator}' has the wrong operand shape")]
    OperandMismatch { operator: SourceFilterOperator },
    #[error("source query contains a non-finite floating-point operand")]
    NonFiniteOperand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityStatus {
    Supported,
    Unavailable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceQueryProgress {
    Idle,
    Pending { revision: u64 },
    Failed { revision: u64, error: String },
}

pub enum SourceQueryCoordinatorEvent {
    Ready {
        revision: u64,
        result: Box<SourceResult>,
    },
    Failed {
        revision: u64,
        error: String,
    },
}

struct SourceQueryJobResult {
    revision: u64,
    result: anyhow::Result<SourceResult>,
}

struct SourceQueryJob {
    revision: u64,
    task: SourceQueryTask,
}

pub struct SourceQueryCoordinator {
    next_revision: u64,
    latest_requested: u64,
    progress: SourceQueryProgress,
    worker: Option<tokio::sync::mpsc::UnboundedSender<SourceQueryJob>>,
    latest_worker_revision: Arc<AtomicU64>,
    worker_handle: Option<tokio::task::JoinHandle<()>>,
    worker_done: mpsc::Receiver<()>,
    receiver: mpsc::Receiver<SourceQueryJobResult>,
}

impl fmt::Debug for SourceQueryCoordinator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceQueryCoordinator")
            .field("latest_requested", &self.latest_requested)
            .field("progress", &self.progress)
            .finish_non_exhaustive()
    }
}

impl Default for SourceQueryCoordinator {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        let (worker, worker_receiver) = tokio::sync::mpsc::unbounded_channel();
        let (worker_done_sender, worker_done) = mpsc::channel();
        let latest_worker_revision = Arc::new(AtomicU64::new(0));
        let worker_revision = latest_worker_revision.clone();
        let worker_handle = source_runtime_handle().spawn(source_query_worker(
            worker_receiver,
            worker_revision,
            sender,
            worker_done_sender,
        ));
        Self {
            next_revision: 1,
            latest_requested: 0,
            progress: SourceQueryProgress::Idle,
            worker: Some(worker),
            latest_worker_revision,
            worker_handle: Some(worker_handle),
            worker_done,
            receiver,
        }
    }
}

impl SourceQueryCoordinator {
    pub fn request(&mut self, task: SourceQueryTask) -> u64 {
        let revision = self.next_revision;
        self.next_revision = self.next_revision.saturating_add(1);
        self.latest_requested = revision;
        self.progress = SourceQueryProgress::Pending { revision };
        self.latest_worker_revision
            .store(revision, Ordering::Release);
        self.worker
            .as_ref()
            .expect("source query worker is active")
            .send(SourceQueryJob { revision, task })
            .expect("source query worker accepts jobs");
        revision
    }
    /// Invalidate every queued or running request before a synchronous reload.
    /// Blocking work keeps running but can no longer publish its result.
    pub fn supersede(&mut self) -> u64 {
        let revision = self.next_revision;
        self.next_revision = self.next_revision.saturating_add(1);
        self.latest_requested = revision;
        self.progress = SourceQueryProgress::Idle;
        self.latest_worker_revision
            .store(revision, Ordering::Release);
        revision
    }

    pub fn latest_revision(&self) -> u64 {
        self.latest_requested
    }

    pub fn progress(&self) -> &SourceQueryProgress {
        &self.progress
    }

    pub fn is_pending(&self) -> bool {
        matches!(self.progress, SourceQueryProgress::Pending { .. })
    }

    pub fn poll(&mut self) -> Option<SourceQueryCoordinatorEvent> {
        let mut latest = None;
        while let Ok(result) = self.receiver.try_recv() {
            if result.revision == self.latest_requested {
                latest = Some(result);
            }
        }
        let result = latest?;
        let revision = result.revision;
        match result.result {
            Ok(result) => {
                self.progress = SourceQueryProgress::Idle;
                Some(SourceQueryCoordinatorEvent::Ready {
                    revision,
                    result: Box::new(result),
                })
            }
            Err(error) => {
                let error = error.to_string();
                self.progress = SourceQueryProgress::Failed {
                    revision,
                    error: error.clone(),
                };
                Some(SourceQueryCoordinatorEvent::Failed { revision, error })
            }
        }
    }
}

impl Drop for SourceQueryCoordinator {
    fn drop(&mut self) {
        self.worker.take();
        let _ = self.worker_done.recv();
        self.worker_handle.take();
    }
}

fn source_runtime_handle() -> tokio::runtime::Handle {
    tokio::runtime::Handle::try_current().unwrap_or_else(|_| {
        static FALLBACK_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
        FALLBACK_RUNTIME
            .get_or_init(|| {
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .thread_name("tview-runtime")
                    .enable_all()
                    .build()
                    .expect("fallback Tokio runtime")
            })
            .handle()
            .clone()
    })
}

async fn source_query_worker(
    mut worker: tokio::sync::mpsc::UnboundedReceiver<SourceQueryJob>,
    latest_requested: Arc<AtomicU64>,
    sender: mpsc::Sender<SourceQueryJobResult>,
    worker_done: mpsc::Sender<()>,
) {
    while let Some(mut job) = worker.recv().await {
        'run: loop {
            while let Ok(replacement) = worker.try_recv() {
                job = replacement;
            }
            if job.revision != latest_requested.load(Ordering::Acquire) {
                break 'run;
            }
            let revision = job.revision;
            match job.task {
                SourceQueryTask::Blocking(task) => {
                    let result = run_blocking_source_query_task(task).await;
                    if revision == latest_requested.load(Ordering::Acquire)
                        && sender
                            .send(SourceQueryJobResult { revision, result })
                            .is_err()
                    {
                        let _ = worker_done.send(());
                        return;
                    }
                    break 'run;
                }
                SourceQueryTask::Async(mut running) => {
                    tokio::select! {
                        result = &mut running => {
                            if revision == latest_requested.load(Ordering::Acquire)
                                && sender
                                    .send(SourceQueryJobResult { revision, result })
                                    .is_err()
                            {
                                let _ = worker_done.send(());
                                return;
                            }
                            break 'run;
                        }
                        replacement = worker.recv() => {
                            let Some(mut replacement) = replacement else {
                                // Closing the lifecycle cancels an async request; blocking
                                // requests are still awaited by the other task branch.
                                let _ = worker_done.send(());
                                return;
                            };
                            while let Ok(newer) = worker.try_recv() {
                                replacement = newer;
                            }
                            job = replacement;
                        }
                    }
                }
            }
        }
    }
    let _ = worker_done.send(());
}

async fn run_blocking_source_query_task(
    task: Box<dyn FnOnce() -> anyhow::Result<SourceResult> + Send + 'static>,
) -> anyhow::Result<SourceResult> {
    match tokio::task::spawn_blocking(task).await {
        Ok(result) => result,
        Err(error) => Err(anyhow::anyhow!("source query task failed: {error}")),
    }
}

impl CapabilityStatus {
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::Supported)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceOperationCapabilities {
    pub filters: Vec<SourceFilterOperator>,
    pub sorting: CapabilityStatus,
    pub configurable_limit: bool,
}

impl SourceOperationCapabilities {
    pub fn supports_filter(&self, operator: SourceFilterOperator) -> bool {
        self.filters.contains(&operator)
    }
}

impl Default for SourceOperationCapabilities {
    fn default() -> Self {
        Self {
            filters: Vec::new(),
            sorting: CapabilityStatus::Unavailable {
                reason: "source-native sorting is unavailable for this source".to_owned(),
            },
            configurable_limit: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(generation: SourceGeneration, rows: Vec<Vec<String>>) -> super::super::SourceResult {
        let column_count = rows.first().map(Vec::len).unwrap_or(0);
        let definition = super::super::TableDefinition {
            generation,
            columns: (0..column_count)
                .map(|ordinal| super::super::ColumnDefinition {
                    id: ColumnId {
                        generation,
                        ordinal: ordinal as u32,
                    },
                    source_identity: super::super::ColumnSourceIdentity::Positional(ordinal),
                    display_name: format!("column {}", ordinal + 1),
                    source_declared_type: None,
                    source_type: super::super::LogicalType::Text,
                    type_origin: super::super::TypeOrigin::Inferred,
                })
                .collect(),
            schema_state: super::super::SchemaState::Complete,
            relation: super::super::RelationMetadata::implicit("test", true),
        };
        super::super::SourceResult::from_store(
            definition,
            Box::new(super::super::InMemoryTable::from_text_rows(
                generation, rows,
            )),
        )
    }

    #[test]
    fn operation_layers_reference_generation_scoped_columns() {
        let generation = SourceGeneration::new();
        let column = ColumnId {
            generation,
            ordinal: 1,
        };
        let view = ViewTransform {
            generation,
            filters: vec![ViewFilter {
                column,
                mode: FilterMode::In,
                predicate: ViewFilterPredicate::Text {
                    value: "ok".to_owned(),
                    domain: ValueDomain::RawOrRendered,
                },
            }],
            order_by: vec![ViewSort {
                column,
                mode: ViewSortMode::Natural,
                direction: SortDirection::Descending,
                nulls: NullPlacement::First,
            }],
        };
        let source = SourceQuery {
            generation,
            native_query: None,
            filters: vec![SourceFilter {
                scope: SourceFilterScope::Column(column),
                operator: SourceFilterOperator::Equal,
                operand: Some(SourceOperand::Text("ok".to_owned())),
            }],
            order_by: vec![SourceSort {
                column,
                direction: SortDirection::Ascending,
            }],
            limit: NonZeroUsize::new(1_000).unwrap(),
        };
        assert_eq!(view.order_by[0].nulls, NullPlacement::First);
        assert!(matches!(
            source.filters[0].scope,
            SourceFilterScope::Column(id) if id.generation == generation
        ));
    }

    #[test]
    fn null_tests_reject_operands_and_comparisons_require_them() {
        let generation = SourceGeneration::new();
        let column = ColumnId {
            generation,
            ordinal: 0,
        };
        assert!(SourceFilter {
            scope: SourceFilterScope::Column(column),
            operator: SourceFilterOperator::IsNull,
            operand: None,
        }
        .validate()
        .is_ok());
        assert!(SourceFilter {
            scope: SourceFilterScope::Column(column),
            operator: SourceFilterOperator::Equal,
            operand: None,
        }
        .validate()
        .is_err());
        assert_eq!(
            SourceFilter {
                scope: SourceFilterScope::Column(column),
                operator: SourceFilterOperator::Equal,
                operand: Some(SourceOperand::Null),
            }
            .validate(),
            Err(SourceQueryValidationError::OperandMismatch {
                operator: SourceFilterOperator::Equal,
            })
        );
    }

    #[test]
    fn source_filter_validation_borrows_operands_and_rejects_non_finite_floats() {
        let generation = SourceGeneration::new();
        let column = ColumnId {
            generation,
            ordinal: 0,
        };
        let text = SourceFilter {
            scope: SourceFilterScope::Column(column),
            operator: SourceFilterOperator::Equal,
            operand: Some(SourceOperand::Text("value".to_owned())),
        };
        assert!(text.validate().is_ok());
        assert_eq!(text.operand, Some(SourceOperand::Text("value".to_owned())));

        let non_finite = SourceFilter {
            scope: SourceFilterScope::Column(column),
            operator: SourceFilterOperator::Equal,
            operand: Some(SourceOperand::Float(f64::NAN)),
        };
        assert_eq!(
            non_finite.validate(),
            Err(SourceQueryValidationError::NonFiniteOperand)
        );
    }

    #[test]
    fn coordinator_publishes_only_latest_revision() {
        let generation = SourceGeneration::new();
        let mut coordinator = SourceQueryCoordinator::default();
        let (release, wait) = std::sync::mpsc::channel();
        let (started, first_started) = std::sync::mpsc::channel();
        let first_finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let finished = first_finished.clone();
        let superseded_ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        coordinator.request(SourceQueryTask::Blocking(Box::new(move || {
            started.send(()).unwrap();
            wait.recv().unwrap();
            finished.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(result(generation, vec![vec!["stale".to_owned()]]))
        })));
        first_started
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("first source query started");
        let ran = superseded_ran.clone();
        coordinator.request(SourceQueryTask::Blocking(Box::new(move || {
            ran.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(result(generation, vec![vec!["superseded".to_owned()]]))
        })));
        let latest = coordinator.request(SourceQueryTask::Blocking(Box::new(move || {
            Ok(result(generation, vec![vec!["latest".to_owned()]]))
        })));
        let non_cancellation_deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(50);
        while std::time::Instant::now() < non_cancellation_deadline {
            assert!(
                coordinator.poll().is_none(),
                "a newer query ran before the active blocking query completed"
            );
            std::thread::yield_now();
        }
        release.send(()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let event = loop {
            if let Some(event) = coordinator.poll() {
                break event;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "source query worker timed out"
            );
            std::thread::yield_now();
        };
        assert!(matches!(
            event,
            SourceQueryCoordinatorEvent::Ready { revision, .. } if revision == latest
        ));
        assert!(first_finished.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!superseded_ran.load(std::sync::atomic::Ordering::SeqCst));
        assert!(coordinator.poll().is_none());
    }

    #[test]
    fn superseded_work_never_overwrites_latest_success_or_failure() {
        let generation = SourceGeneration::new();
        for stale_fails in [true, false] {
            let mut coordinator = SourceQueryCoordinator::default();
            let (started, wait_for_start) = std::sync::mpsc::channel();
            let (release, wait_for_release) = std::sync::mpsc::channel();
            coordinator.request(SourceQueryTask::Blocking(Box::new(move || {
                started.send(()).expect("blocking worker started");
                wait_for_release.recv().expect("blocking worker released");
                if stale_fails {
                    anyhow::bail!("stale failure");
                }
                Ok(result(generation, vec![vec!["stale".to_owned()]]))
            })));
            wait_for_start
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("worker started");
            coordinator.supersede();
            let latest = coordinator.request(SourceQueryTask::Blocking(Box::new(move || {
                if stale_fails {
                    Ok(result(generation, vec![vec!["current".to_owned()]]))
                } else {
                    anyhow::bail!("current failure");
                }
            })));
            release.send(()).expect("release stale worker");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let event = loop {
                if let Some(event) = coordinator.poll() {
                    break event;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "latest worker timed out"
                );
                std::thread::yield_now();
            };
            if stale_fails {
                assert!(
                    matches!(event, SourceQueryCoordinatorEvent::Ready { revision, .. } if revision == latest)
                );
            } else {
                assert!(
                    matches!(event, SourceQueryCoordinatorEvent::Failed { revision, .. } if revision == latest)
                );
            }
            assert!(coordinator.poll().is_none());
        }
    }

    #[test]
    fn dropping_coordinator_joins_its_active_worker() {
        let generation = SourceGeneration::new();
        let mut coordinator = SourceQueryCoordinator::default();
        let (started, wait_for_start) = std::sync::mpsc::channel();
        let (release, wait_for_release) = std::sync::mpsc::channel();
        coordinator.request(SourceQueryTask::Blocking(Box::new(move || {
            started.send(()).expect("worker started");
            wait_for_release.recv().expect("release worker");
            Ok(result(generation, Vec::new()))
        })));
        wait_for_start
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("active worker");

        let (dropped, wait_for_drop) = std::sync::mpsc::channel();
        let drop_thread = std::thread::spawn(move || {
            drop(coordinator);
            dropped.send(()).expect("drop completed");
        });
        assert!(
            wait_for_drop
                .recv_timeout(std::time::Duration::from_millis(25))
                .is_err(),
            "coordinator detached its active worker"
        );

        release.send(()).expect("release active worker");
        wait_for_drop
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("worker joined");
        drop_thread.join().expect("drop thread");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn source_query_tasks_run_on_the_active_tokio_runtime() {
        let generation = SourceGeneration::new();
        let mut coordinator = SourceQueryCoordinator::default();
        coordinator.request(SourceQueryTask::Blocking(Box::new(move || {
            assert!(tokio::runtime::Handle::try_current().is_ok());
            Ok(result(generation, vec![vec!["tokio".to_owned()]]))
        })));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(event) = coordinator.poll() {
                assert!(matches!(event, SourceQueryCoordinatorEvent::Ready { .. }));
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Tokio source query task timed out"
            );
            tokio::task::yield_now().await;
        }
    }
}
