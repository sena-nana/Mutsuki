//! The SQLite actor: one connection thread, its bounded job queue and metrics, connection
//! setup, and the repository handle's inherent API.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use mutsuki_bot_management::in_blocking_section;
use mutsuki_bot_protocol::{BotDeliveryAttempt, BotDeliveryReceipt, BotInteractionSession};
use rusqlite::Connection;
use tokio::sync::mpsc;

use crate::codec::bounded_page_limit;
use crate::delivery::delivery_page;
use crate::error::BotStateDbError;
use crate::inspect::{
    BotDatabaseSnapshot, BotDatabaseTablePage, inspect_rows, inspect_snapshot_tables,
};
use crate::interaction::interaction_page;
use crate::management::{
    BotManagementAuditRecord, BotManagementOperationReservation, begin_management_operation,
    commit_management_audit, complete_management_operation, management_audits, management_revision,
};
use crate::migrations::migrate_schema;

const DEFAULT_QUEUE_CAPACITY: usize = 256;
pub(crate) const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct BotStateDbRepository {
    inner: Arc<RepositoryInner>,
}

struct RepositoryInner {
    path: PathBuf,
    jobs: mpsc::Sender<DbJob>,
    metrics: Arc<ActorMetrics>,
    /// Effective `journal_mode` after open (`wal`, or a non-WAL fallback such as `delete`).
    journal_mode: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BotStateDbMetrics {
    pub queue_depth: usize,
    pub peak_queue_depth: usize,
    pub transaction_count: u64,
    pub transaction_latency_total_ns: u64,
    pub transaction_latency_max_ns: u64,
    pub busy_count: u64,
    pub connection_open_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BotStatePage<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Default)]
struct ActorMetrics {
    queue_depth: AtomicUsize,
    peak_queue_depth: AtomicUsize,
    transaction_count: AtomicU64,
    transaction_latency_total_ns: AtomicU64,
    transaction_latency_max_ns: AtomicU64,
    busy_count: AtomicU64,
    connection_open_count: AtomicU64,
}

impl ActorMetrics {
    fn snapshot(&self) -> BotStateDbMetrics {
        BotStateDbMetrics {
            queue_depth: self.queue_depth.load(Ordering::Relaxed),
            peak_queue_depth: self.peak_queue_depth.load(Ordering::Relaxed),
            transaction_count: self.transaction_count.load(Ordering::Relaxed),
            transaction_latency_total_ns: self.transaction_latency_total_ns.load(Ordering::Relaxed),
            transaction_latency_max_ns: self.transaction_latency_max_ns.load(Ordering::Relaxed),
            busy_count: self.busy_count.load(Ordering::Relaxed),
            connection_open_count: self.connection_open_count.load(Ordering::Relaxed),
        }
    }

    fn queued(&self) {
        let depth = self.queue_depth.fetch_add(1, Ordering::Relaxed) + 1;
        self.peak_queue_depth.fetch_max(depth, Ordering::Relaxed);
    }

    fn dequeued(&self) {
        self.queue_depth.fetch_sub(1, Ordering::Relaxed);
    }

    fn transaction_finished(&self, elapsed: Duration) {
        let nanos = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        self.transaction_count.fetch_add(1, Ordering::Relaxed);
        self.transaction_latency_total_ns
            .fetch_add(nanos, Ordering::Relaxed);
        self.transaction_latency_max_ns
            .fetch_max(nanos, Ordering::Relaxed);
    }

    fn observe_error(&self, error: &BotStateDbError) {
        if let BotStateDbError::Database(rusqlite::Error::SqliteFailure(code, _)) = error
            && matches!(
                code.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            )
        {
            self.busy_count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl BotStateDbRepository {
    /// Opens a single-connection database actor with the default bounded queue.
    ///
    /// # Errors
    ///
    /// Returns an error when the actor thread, `SQLite` connection, or schema setup fails.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, BotStateDbError> {
        Self::open_with_capacity(path, DEFAULT_QUEUE_CAPACITY)
    }

    /// Opens a single-connection database actor with an explicit queue capacity.
    ///
    /// # Errors
    ///
    /// Returns an error for zero capacity or when actor and database initialization fails.
    pub fn open_with_capacity(
        path: impl Into<PathBuf>,
        queue_capacity: usize,
    ) -> Result<Self, BotStateDbError> {
        if queue_capacity == 0 {
            return Err(BotStateDbError::InvalidConfiguration(
                "queue_capacity must be greater than zero".into(),
            ));
        }
        let path = path.into();
        let (jobs, receiver) = mpsc::channel(queue_capacity);
        let metrics = Arc::new(ActorMetrics::default());
        let actor_path = path.clone();
        let actor_metrics = metrics.clone();
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("mutsuki-bot-state-db".into())
            .spawn(move || {
                let connection = open_connection(&actor_path, &actor_metrics);
                match connection {
                    Ok((connection, journal_mode)) => {
                        let _ = started_tx.send(Ok(journal_mode));
                        actor_loop(connection, receiver, &actor_metrics);
                    }
                    Err(error) => {
                        let _ = started_tx.send(Err(error.to_string()));
                    }
                }
            })
            .map_err(|error| BotStateDbError::ActorStart(error.to_string()))?;
        let journal_mode = started_rx
            .recv()
            .map_err(|_| BotStateDbError::ActorStopped)?
            .map_err(BotStateDbError::ActorStart)?;
        Ok(Self {
            inner: Arc::new(RepositoryInner {
                path,
                jobs,
                metrics,
                journal_mode,
            }),
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// Effective SQLite `journal_mode` after open (prefer `wal`; may be a fallback mode).
    #[must_use]
    pub fn journal_mode(&self) -> &str {
        &self.inner.journal_mode
    }

    #[must_use]
    pub fn metrics(&self) -> BotStateDbMetrics {
        self.inner.metrics.snapshot()
    }

    /// Reads the live catalog: user tables, columns, and row counts.
    pub fn inspect_snapshot(&self) -> Result<BotDatabaseSnapshot, BotStateDbError> {
        let tables =
            self.call_sync(false, move |connection| inspect_snapshot_tables(connection))?;
        Ok(BotDatabaseSnapshot {
            path: self.path().display().to_string(),
            journal_mode: self.journal_mode().to_string(),
            size_bytes: std::fs::metadata(self.path()).ok().map(|meta| meta.len()),
            tables,
        })
    }

    /// Pages rows from a live user table. `after` is the last `rowid` cursor.
    pub fn inspect_rows(
        &self,
        table: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<BotDatabaseTablePage, BotStateDbError> {
        let table = table.to_owned();
        let after = after.unwrap_or_default().to_owned();
        {
            let limit = bounded_page_limit(limit);
            self.call_sync(false, move |connection| {
                inspect_rows(connection, &table, &after, limit)
            })
        }
    }

    /// Lists durable delivery receipts and attempts in stable delivery-id order.
    pub fn delivery_page(
        &self,
        after: Option<&str>,
        limit: u32,
    ) -> Result<BotStatePage<(BotDeliveryReceipt, Vec<BotDeliveryAttempt>)>, BotStateDbError> {
        let after = after.unwrap_or_default().to_owned();
        {
            let limit = bounded_page_limit(limit);
            self.call_sync(false, move |connection| {
                delivery_page(connection, &after, limit)
            })
        }
    }

    /// Lists durable interaction sessions in stable session-id order.
    pub fn interaction_page(
        &self,
        after: Option<&str>,
        limit: u32,
    ) -> Result<BotStatePage<BotInteractionSession>, BotStateDbError> {
        let after = after.unwrap_or_default().to_owned();
        {
            let limit = bounded_page_limit(limit);
            self.call_sync(false, move |connection| {
                interaction_page(connection, &after, limit)
            })
        }
    }

    pub fn management_revision(&self) -> Result<u64, BotStateDbError> {
        self.call_sync(false, move |connection| management_revision(connection))
    }

    pub fn management_audits(
        &self,
        limit: u32,
    ) -> Result<Vec<BotManagementAuditRecord>, BotStateDbError> {
        {
            let limit = bounded_page_limit(limit);
            self.call_sync(false, move |connection| {
                management_audits(connection, limit)
            })
        }
    }

    pub fn begin_management_operation(
        &self,
        operation_id: &str,
        expected_revision: u64,
        actor_id: &str,
        action: &str,
        created_at_unix_ms: u64,
    ) -> Result<BotManagementOperationReservation, BotStateDbError> {
        {
            let operation_id = operation_id.to_owned();
            let actor_id = actor_id.to_owned();
            let action = action.to_owned();
            self.call_sync(true, move |connection| {
                begin_management_operation(
                    connection,
                    &operation_id,
                    expected_revision,
                    &actor_id,
                    &action,
                    created_at_unix_ms,
                )
            })
        }
    }

    pub fn complete_management_operation(
        &self,
        operation_id: &str,
        action: &str,
        result: serde_json::Value,
        created_at_unix_ms: u64,
    ) -> Result<BotManagementAuditRecord, BotStateDbError> {
        {
            let operation_id = operation_id.to_owned();
            let action = action.to_owned();
            self.call_sync(true, move |connection| {
                complete_management_operation(
                    connection,
                    &operation_id,
                    &action,
                    result,
                    created_at_unix_ms,
                )
            })
        }
    }

    /// Commits one revision-fenced management audit entry atomically.
    pub fn commit_management_audit(
        &self,
        expected_revision: u64,
        actor_id: &str,
        action: &str,
        result: serde_json::Value,
        created_at_unix_ms: u64,
    ) -> Result<Option<BotManagementAuditRecord>, BotStateDbError> {
        {
            let actor_id = actor_id.to_owned();
            let action = action.to_owned();
            self.call_sync(true, move |connection| {
                commit_management_audit(
                    connection,
                    expected_revision,
                    &actor_id,
                    &action,
                    result,
                    created_at_unix_ms,
                )
            })
        }
    }

    /// Blocks the caller until the SQLite actor answers.
    ///
    /// Runner-facing callers must stay synchronous, so this cannot become async. The Web Console
    /// reaches the same repository from an async executor; the wait is announced as a blocking
    /// section so the scheduler can relocate the console's other sockets first.
    ///
    /// `transactional` only marks the work for the latency metric; each repository
    /// function opens its own transaction when it needs one.
    pub(crate) fn call_sync<T, F>(&self, transactional: bool, work: F) -> Result<T, BotStateDbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, BotStateDbError> + Send + 'static,
    {
        let (reply, response) = std::sync::mpsc::sync_channel(1);
        self.inner.metrics.queued();
        let job = DbJob {
            transactional,
            run: Box::new(move |connection, metrics| {
                send_reply(reply, work(connection), metrics);
            }),
        };
        match self.inner.jobs.try_send(job) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.inner.metrics.dequeued();
                return Err(BotStateDbError::QueueFull);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.inner.metrics.dequeued();
                return Err(BotStateDbError::ActorStopped);
            }
        }
        in_blocking_section(|| response.recv().map_err(|_| BotStateDbError::ActorStopped))?
    }
}

/// One unit of work for the SQLite actor.
///
/// This used to be a 40-plus variant enum: adding an operation meant a variant, an
/// entry in a `transactional` list, a match arm that only unpacked the variant and
/// forwarded it, and the repository method itself. The enum carried no information
/// the closure cannot, because every arm did exactly `send_reply(reply, f(..),
/// metrics)`. Boxing the call collapses those four edits into one.
struct DbJob {
    /// Marks the work for the transaction-latency metric only.
    transactional: bool,
    run: Box<dyn FnOnce(&mut Connection, &ActorMetrics) + Send>,
}

fn actor_loop(mut connection: Connection, mut jobs: mpsc::Receiver<DbJob>, metrics: &ActorMetrics) {
    while let Some(job) = jobs.blocking_recv() {
        metrics.dequeued();
        let transactional = job.transactional;
        let started = Instant::now();
        (job.run)(&mut connection, metrics);
        if transactional {
            metrics.transaction_finished(started.elapsed());
        }
    }
}

fn send_reply<T>(
    reply: std::sync::mpsc::SyncSender<Result<T, BotStateDbError>>,
    result: Result<T, BotStateDbError>,
    metrics: &ActorMetrics,
) {
    if let Err(error) = &result {
        metrics.observe_error(error);
    }
    // The caller may have gone away; the actor keeps serving either way.
    let _ = reply.send(result);
}

fn open_connection(
    path: &Path,
    metrics: &ActorMetrics,
) -> Result<(Connection, String), BotStateDbError> {
    let connection = Connection::open(path)?;
    metrics
        .connection_open_count
        .fetch_add(1, Ordering::Relaxed);
    let journal_mode = configure_connection(&connection)?;
    migrate_schema(&connection)?;
    Ok((connection, journal_mode))
}

/// Prepared statements the actor reuses across calls.
///
/// The actor owns one long-lived connection, so every repository query can be served
/// from the statement cache instead of being recompiled per call. The capacity is
/// sized above the number of distinct cached statements (repository queries plus the
/// sandbox history queries that share this connection) so the hot set does not evict
/// itself; rusqlite's default of 16 would thrash.
const STATEMENT_CACHE_CAPACITY: usize = 64;

/// Single-connection factory: busy timeout, foreign keys, prefer WAL, then synchronous=NORMAL.
/// When WAL is unavailable, SQLite keeps another mode and open still succeeds.
fn configure_connection(connection: &Connection) -> Result<String, BotStateDbError> {
    connection.set_prepared_statement_cache_capacity(STATEMENT_CACHE_CAPACITY);
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.pragma_update(None, "foreign_keys", true)?;
    let journal_mode: String =
        connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(journal_mode.to_ascii_lowercase())
}
