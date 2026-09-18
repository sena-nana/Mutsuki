// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::doc_markdown,
    clippy::ignored_unit_patterns,
    clippy::implicit_clone,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::needless_borrow,
    clippy::needless_pass_by_value,
    clippy::semicolon_if_nothing_returned,
    clippy::too_many_lines,
    clippy::type_complexity
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use mutsuki_bot_conversation::PersonaStore;
use mutsuki_bot_conversation::{
    AgentEventClaim, ConversationContextStore, ConversationError, ConversationRepository,
};
use mutsuki_bot_delivery::{
    DELIVERY_SEND_LEASE_MS, DeliveryError, DeliveryRepository, ReplyDeliveryRepository,
    reply_part_request,
};
use mutsuki_bot_interaction::{InteractionError, InteractionRepository};
use mutsuki_bot_management::in_blocking_section;
use mutsuki_bot_protocol::{
    AgentSessionBinding, BotActiveDeliveryRequest, BotDeliveryAttempt, BotDeliveryReceipt,
    BotInteractionSession, BotPersona, BotReplyDeliveryReceipt, BotReplyDeliveryRequest,
    ConversationIclEntry, DeliveryStatus, InteractionStatus,
};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use thiserror::Error;
use tokio::sync::mpsc;

mod sandbox_history;

const DEFAULT_QUEUE_CAPACITY: usize = 256;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

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

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BotDatabaseSnapshot {
    pub path: String,
    pub journal_mode: String,
    pub size_bytes: Option<u64>,
    pub tables: Vec<BotDatabaseTableInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BotDatabaseTableInfo {
    pub name: String,
    pub row_count: u64,
    pub columns: Vec<BotDatabaseColumnInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BotDatabaseColumnInfo {
    pub name: String,
    pub decl_type: String,
    pub primary_key: bool,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BotDatabaseTablePage {
    pub table: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BotManagementAuditRecord {
    pub audit_id: String,
    pub revision: u64,
    pub actor_id: String,
    pub action: String,
    pub result: serde_json::Value,
    pub created_at_unix_ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BotManagementOperationReservation {
    Reserved,
    Resume,
    Completed(BotManagementAuditRecord),
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
    fn call_sync<T, F>(&self, transactional: bool, work: F) -> Result<T, BotStateDbError>
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

fn migrate_schema(connection: &Connection) -> Result<(), BotStateDbError> {
    let user_version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let sql = format!(
        "BEGIN IMMEDIATE;
         CREATE TABLE IF NOT EXISTS bot_conversation_policy(
             rule_id TEXT PRIMARY KEY,
             body TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS bot_conversation_policy_meta(
             singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
             revision INTEGER NOT NULL
         );
         INSERT OR IGNORE INTO bot_conversation_policy_meta(singleton, revision) VALUES (1, 0);
         CREATE TABLE IF NOT EXISTS bot_conversation_policy_audit(
             revision INTEGER PRIMARY KEY,
             audit_id TEXT NOT NULL UNIQUE,
             body TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS bot_agent_binding(
             binding_key TEXT PRIMARY KEY,
             generation INTEGER NOT NULL,
             body TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS bot_agent_event(
             binding_key TEXT NOT NULL,
             event_id TEXT NOT NULL,
             turn_id TEXT NOT NULL,
             status TEXT NOT NULL CHECK(status IN ('pending', 'completed')),
             PRIMARY KEY(binding_key, event_id)
         );
         CREATE TABLE IF NOT EXISTS bot_delivery_request(
             delivery_id TEXT PRIMARY KEY,
             idempotency_key TEXT NOT NULL UNIQUE,
             body TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS bot_delivery_attempt(
             delivery_id TEXT NOT NULL,
             attempt INTEGER NOT NULL,
             status TEXT NOT NULL,
             retry_at INTEGER,
             body TEXT NOT NULL,
             PRIMARY KEY(delivery_id, attempt, status),
             FOREIGN KEY(delivery_id) REFERENCES bot_delivery_request(delivery_id)
         );
         CREATE TABLE IF NOT EXISTS bot_delivery_receipt(
             delivery_id TEXT PRIMARY KEY,
             idempotency_key TEXT NOT NULL UNIQUE,
             status TEXT NOT NULL DEFAULT 'pending',
             body TEXT NOT NULL,
             FOREIGN KEY(delivery_id) REFERENCES bot_delivery_request(delivery_id)
         );
         CREATE TABLE IF NOT EXISTS bot_reply_delivery(
             reply_id TEXT PRIMARY KEY,
             idempotency_key TEXT NOT NULL UNIQUE,
             body TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS bot_reply_delivery_part(
             reply_id TEXT NOT NULL,
             delivery_id TEXT NOT NULL UNIQUE,
             part_index INTEGER NOT NULL,
             PRIMARY KEY(reply_id, part_index),
             FOREIGN KEY(reply_id) REFERENCES bot_reply_delivery(reply_id),
             FOREIGN KEY(delivery_id) REFERENCES bot_delivery_request(delivery_id)
         );
         CREATE TABLE IF NOT EXISTS bot_interaction(
             session_id TEXT PRIMARY KEY,
             origin_key TEXT NOT NULL,
             status TEXT NOT NULL,
             version INTEGER NOT NULL,
             body TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS bot_management_meta(
             singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
             revision INTEGER NOT NULL
         );
         INSERT OR IGNORE INTO bot_management_meta(singleton, revision) VALUES (1, 0);
         CREATE TABLE IF NOT EXISTS bot_management_audit(
             audit_id INTEGER PRIMARY KEY AUTOINCREMENT,
             revision INTEGER NOT NULL UNIQUE,
             actor_id TEXT NOT NULL,
             action TEXT NOT NULL,
             result TEXT NOT NULL,
             created_at_unix_ms INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS bot_management_operation(
             operation_id TEXT PRIMARY KEY,
             expected_revision INTEGER NOT NULL,
             actor_id TEXT NOT NULL,
             action TEXT NOT NULL,
             audit_revision INTEGER,
             created_at_unix_ms INTEGER NOT NULL,
             FOREIGN KEY(audit_revision) REFERENCES bot_management_audit(revision)
         );
         CREATE INDEX IF NOT EXISTS bot_delivery_attempt_due
             ON bot_delivery_attempt(status, retry_at, delivery_id, attempt);
         CREATE INDEX IF NOT EXISTS bot_interaction_active
             ON bot_interaction(origin_key, status, session_id);
         CREATE TABLE IF NOT EXISTS bot_conversation_icl(
             origin_key TEXT NOT NULL,
             seq INTEGER NOT NULL,
             body TEXT NOT NULL,
             PRIMARY KEY(origin_key, seq)
         );
         CREATE TABLE IF NOT EXISTS bot_persona(
             persona_id TEXT PRIMARY KEY,
             body TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS bot_persona_binding(
             origin_key TEXT PRIMARY KEY,
             persona_id TEXT NOT NULL
         );
         {}
         COMMIT;",
        sandbox_history::SANDBOX_SCHEMA_SQL
    );
    connection.execute_batch(&sql)?;
    sandbox_history::migrate_sandbox_v9(connection)?;
    sandbox_history::migrate_sandbox_v10(connection)?;
    if user_version < 11 {
        sandbox_history::migrate_sandbox_v11(connection)?;
    }
    sandbox_history::migrate_sandbox_v12(connection)?;

    let has_receipt_status = {
        let mut statement = connection.prepare("PRAGMA table_info(bot_delivery_receipt)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        columns.iter().any(|column| column == "status")
    };
    if !has_receipt_status {
        connection.execute(
            "ALTER TABLE bot_delivery_receipt ADD COLUMN status TEXT NOT NULL DEFAULT 'pending'",
            [],
        )?;
        let mut statement =
            connection.prepare("SELECT delivery_id, body FROM bot_delivery_receipt")?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (delivery_id, body) in rows {
            let receipt: BotDeliveryReceipt = decode(&body)?;
            connection.execute(
                "UPDATE bot_delivery_receipt SET status=?2 WHERE delivery_id=?1",
                params![delivery_id, delivery_status_name(receipt.status)],
            )?;
        }
    }
    connection.execute(
        "CREATE INDEX IF NOT EXISTS bot_delivery_receipt_status
         ON bot_delivery_receipt(status, delivery_id)",
        [],
    )?;
    connection.pragma_update(None, "user_version", 12)?;
    Ok(())
}

fn immediate(connection: &mut Connection) -> Result<Transaction<'_>, BotStateDbError> {
    Ok(connection.transaction_with_behavior(TransactionBehavior::Immediate)?)
}

fn session_binding(
    connection: &Connection,
    binding_key: &str,
) -> Result<Option<AgentSessionBinding>, BotStateDbError> {
    connection
        .query_row(
            "SELECT body FROM bot_agent_binding WHERE binding_key=?1",
            params![binding_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|body| decode(&body))
        .transpose()
}

fn compare_and_set_session_binding(
    connection: &mut Connection,
    binding_key: &str,
    expected_generation: Option<u64>,
    binding: &AgentSessionBinding,
) -> Result<bool, BotStateDbError> {
    let transaction = immediate(connection)?;
    let body = encode(binding)?;
    let generation = sqlite_integer(binding.generation)?;
    let changed = match expected_generation {
        None => transaction.execute(
            "INSERT OR IGNORE INTO bot_agent_binding(binding_key, generation, body)
             VALUES (?1, ?2, ?3)",
            params![binding_key, generation, body],
        )?,
        Some(expected) => {
            let expected = sqlite_integer(expected)?;
            transaction.execute(
                "UPDATE bot_agent_binding SET generation=?2, body=?3
                 WHERE binding_key=?1 AND generation=?4",
                params![binding_key, generation, body, expected],
            )?
        }
    };
    transaction.commit()?;
    Ok(changed == 1)
}

fn begin_agent_event(
    connection: &mut Connection,
    binding_key: &str,
    event_id: &str,
    turn_id: &str,
) -> Result<AgentEventClaim, BotStateDbError> {
    let transaction = immediate(connection)?;
    let changed = transaction.execute(
        "INSERT OR IGNORE INTO bot_agent_event(binding_key, event_id, turn_id, status)
         VALUES (?1, ?2, ?3, 'pending')",
        params![binding_key, event_id, turn_id],
    )?;
    let claim = if changed == 1 {
        AgentEventClaim::New
    } else {
        let status = transaction.query_row(
            "SELECT status FROM bot_agent_event WHERE binding_key=?1 AND event_id=?2",
            params![binding_key, event_id],
            |row| row.get::<_, String>(0),
        )?;
        if status == "completed" {
            AgentEventClaim::Completed
        } else {
            AgentEventClaim::ResumePending
        }
    };
    transaction.commit()?;
    Ok(claim)
}

fn complete_agent_event(
    connection: &Connection,
    binding_key: &str,
    event_id: &str,
) -> Result<bool, BotStateDbError> {
    Ok(connection.execute(
        "UPDATE bot_agent_event SET status='completed' WHERE binding_key=?1 AND event_id=?2",
        params![binding_key, event_id],
    )? == 1)
}

#[derive(Debug)]
enum DeliveryReservation {
    Reserved,
    Existing(BotDeliveryReceipt),
    Conflict,
}

fn pending_receipt(request: &BotActiveDeliveryRequest) -> BotDeliveryReceipt {
    BotDeliveryReceipt {
        delivery_id: request.delivery_id.clone(),
        idempotency_key: request.idempotency_key.clone(),
        status: DeliveryStatus::Pending,
        attempt_count: 0,
        platform_message_ids: Vec::new(),
        part_receipts: Vec::new(),
        delivered_at_unix_ms: None,
        error_code: None,
        generation: 0,
        lease_expires_at_unix_ms: None,
    }
}

fn reserve_delivery(
    connection: &mut Connection,
    request: &BotActiveDeliveryRequest,
) -> Result<DeliveryReservation, BotStateDbError> {
    let transaction = immediate(connection)?;
    let reservation = reserve_delivery_in_transaction(&transaction, request)?;
    transaction.commit()?;
    Ok(reservation)
}

fn reserve_delivery_in_transaction(
    transaction: &Transaction<'_>,
    request: &BotActiveDeliveryRequest,
) -> Result<DeliveryReservation, BotStateDbError> {
    let changed = transaction.execute(
        "INSERT OR IGNORE INTO bot_delivery_request(delivery_id, idempotency_key, body)
         VALUES (?1, ?2, ?3)",
        params![
            request.delivery_id,
            request.idempotency_key,
            encode(request)?
        ],
    )?;
    if changed == 1 {
        upsert_delivery_receipt(&transaction, &pending_receipt(request))?;
        return Ok(DeliveryReservation::Reserved);
    }
    if let Some(body) = transaction
        .query_row(
            "SELECT body FROM bot_delivery_receipt WHERE idempotency_key=?1",
            params![request.idempotency_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        let receipt = decode(&body)?;
        return Ok(DeliveryReservation::Existing(receipt));
    }
    if let Some(body) = transaction
        .query_row(
            "SELECT body FROM bot_delivery_request WHERE idempotency_key=?1",
            params![request.idempotency_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        let owner: BotActiveDeliveryRequest = decode(&body)?;
        let receipt = pending_receipt(&owner);
        upsert_delivery_receipt(&transaction, &receipt)?;
        return Ok(DeliveryReservation::Existing(receipt));
    }
    let delivery_exists = transaction
        .query_row(
            "SELECT 1 FROM bot_delivery_request WHERE delivery_id=?1",
            params![request.delivery_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    Ok(if delivery_exists {
        DeliveryReservation::Conflict
    } else {
        return Err(BotStateDbError::Invariant(
            "delivery reservation disappeared after conflict".into(),
        ));
    })
}

#[derive(Debug)]
enum ReplyDeliveryReservation {
    Reserved,
    Existing(BotReplyDeliveryReceipt),
    Conflict,
}

fn reserve_reply_delivery(
    connection: &mut Connection,
    request: &BotReplyDeliveryRequest,
) -> Result<ReplyDeliveryReservation, BotStateDbError> {
    let transaction = immediate(connection)?;
    if let Some(body) = transaction
        .query_row(
            "SELECT body FROM bot_reply_delivery WHERE reply_id=?1 OR idempotency_key=?2",
            params![request.reply_id, request.idempotency_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        let existing: BotReplyDeliveryRequest = decode(&body)?;
        if existing.reply_id != request.reply_id
            || existing.idempotency_key != request.idempotency_key
        {
            return Ok(ReplyDeliveryReservation::Conflict);
        }
        if existing == *request {
            complete_reply_source_event(&transaction, request)?;
            let receipt = reply_delivery_receipt(&transaction, &existing)?;
            transaction.commit()?;
            return Ok(ReplyDeliveryReservation::Existing(receipt));
        }
        if !reply_parts_all_pending(&transaction, &existing)? {
            return Ok(ReplyDeliveryReservation::Conflict);
        }
        replace_pending_reply_parts(&transaction, &existing, request)?;
        complete_reply_source_event(&transaction, request)?;
        transaction.commit()?;
        return Ok(ReplyDeliveryReservation::Reserved);
    }
    transaction.execute(
        "INSERT INTO bot_reply_delivery(reply_id, idempotency_key, body) VALUES (?1, ?2, ?3)",
        params![request.reply_id, request.idempotency_key, encode(request)?],
    )?;
    for (index, part) in request.parts.iter().enumerate() {
        let delivery = reply_part_request(request, part);
        if !matches!(
            reserve_delivery_in_transaction(&transaction, &delivery)?,
            DeliveryReservation::Reserved
        ) {
            return Ok(ReplyDeliveryReservation::Conflict);
        }
        transaction.execute(
            "INSERT INTO bot_reply_delivery_part(reply_id, delivery_id, part_index)
             VALUES (?1, ?2, ?3)",
            params![
                request.reply_id,
                part.part_id,
                i64::try_from(index).map_err(|_| {
                    BotStateDbError::Invariant("reply part index exceeds SQLite integer".into())
                })?
            ],
        )?;
    }
    complete_reply_source_event(&transaction, request)?;
    transaction.commit()?;
    Ok(ReplyDeliveryReservation::Reserved)
}

fn reply_parts_all_pending(
    connection: &Connection,
    request: &BotReplyDeliveryRequest,
) -> Result<bool, BotStateDbError> {
    for part in &request.parts {
        let Some(receipt) = delivery_receipt(connection, &part.part_id)? else {
            return Ok(false);
        };
        if receipt.status != DeliveryStatus::Pending {
            return Ok(false);
        }
    }
    Ok(true)
}

fn replace_pending_reply_parts(
    transaction: &Transaction<'_>,
    existing: &BotReplyDeliveryRequest,
    request: &BotReplyDeliveryRequest,
) -> Result<(), BotStateDbError> {
    for part in &existing.parts {
        transaction.execute(
            "DELETE FROM bot_delivery_attempt WHERE delivery_id=?1",
            params![part.part_id],
        )?;
        transaction.execute(
            "DELETE FROM bot_delivery_receipt WHERE delivery_id=?1",
            params![part.part_id],
        )?;
        transaction.execute(
            "DELETE FROM bot_reply_delivery_part WHERE delivery_id=?1",
            params![part.part_id],
        )?;
        transaction.execute(
            "DELETE FROM bot_delivery_request WHERE delivery_id=?1",
            params![part.part_id],
        )?;
    }
    transaction.execute(
        "UPDATE bot_reply_delivery SET idempotency_key=?2, body=?3 WHERE reply_id=?1",
        params![request.reply_id, request.idempotency_key, encode(request)?],
    )?;
    for (index, part) in request.parts.iter().enumerate() {
        let delivery = reply_part_request(request, part);
        if !matches!(
            reserve_delivery_in_transaction(transaction, &delivery)?,
            DeliveryReservation::Reserved
        ) {
            return Err(BotStateDbError::Invariant(
                "pending reply part replacement collided with another delivery".into(),
            ));
        }
        transaction.execute(
            "INSERT INTO bot_reply_delivery_part(reply_id, delivery_id, part_index)
             VALUES (?1, ?2, ?3)",
            params![
                request.reply_id,
                part.part_id,
                i64::try_from(index).map_err(|_| {
                    BotStateDbError::Invariant("reply part index exceeds SQLite integer".into())
                })?
            ],
        )?;
    }
    Ok(())
}

fn complete_reply_source_event(
    transaction: &Transaction<'_>,
    request: &BotReplyDeliveryRequest,
) -> Result<(), BotStateDbError> {
    if let Some(binding_key) = request.source_binding_key.as_deref() {
        transaction.execute(
            "UPDATE bot_agent_event SET status='completed'
             WHERE binding_key=?1 AND event_id=?2 AND status='pending'",
            params![binding_key, request.source_event_id],
        )?;
    }
    Ok(())
}

fn reply_delivery_receipt_by_id(
    connection: &Connection,
    reply_id: &str,
) -> Result<Option<BotReplyDeliveryReceipt>, BotStateDbError> {
    let request = optional_body::<BotReplyDeliveryRequest>(
        connection,
        "SELECT body FROM bot_reply_delivery WHERE reply_id=?1",
        reply_id,
    )?;
    request
        .map(|request| reply_delivery_receipt(connection, &request))
        .transpose()
}

fn reply_delivery_receipt(
    connection: &Connection,
    request: &BotReplyDeliveryRequest,
) -> Result<BotReplyDeliveryReceipt, BotStateDbError> {
    let part_receipts = request
        .parts
        .iter()
        .map(|part| {
            delivery_receipt(connection, &part.part_id)?.ok_or_else(|| {
                BotStateDbError::Invariant(format!("reply part receipt missing: {}", part.part_id))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(BotReplyDeliveryReceipt {
        reply_id: request.reply_id.clone(),
        idempotency_key: request.idempotency_key.clone(),
        part_receipts,
    })
}

fn delivery_request(
    connection: &Connection,
    delivery_id: &str,
) -> Result<Option<BotActiveDeliveryRequest>, BotStateDbError> {
    optional_body(
        connection,
        "SELECT body FROM bot_delivery_request WHERE delivery_id=?1",
        delivery_id,
    )
}

fn delivery_receipt(
    connection: &Connection,
    delivery_id: &str,
) -> Result<Option<BotDeliveryReceipt>, BotStateDbError> {
    optional_body(
        connection,
        "SELECT body FROM bot_delivery_receipt WHERE delivery_id=?1",
        delivery_id,
    )
}

fn optional_body<T: serde::de::DeserializeOwned>(
    connection: &Connection,
    sql: &str,
    key: &str,
) -> Result<Option<T>, BotStateDbError> {
    connection
        .query_row(sql, params![key], |row| row.get::<_, String>(0))
        .optional()?
        .map(|body| decode(&body))
        .transpose()
}

fn delivery_attempts(
    connection: &Connection,
    delivery_id: &str,
) -> Result<Vec<BotDeliveryAttempt>, BotStateDbError> {
    let mut statement = connection.prepare_cached(
        "SELECT body FROM bot_delivery_attempt
         WHERE delivery_id=?1 ORDER BY attempt, status",
    )?;
    statement
        .query_map(params![delivery_id], |row| row.get::<_, String>(0))?
        .map(|body| decode(&body?))
        .collect()
}

fn delivery_page(
    connection: &Connection,
    after: &str,
    limit: u32,
) -> Result<BotStatePage<(BotDeliveryReceipt, Vec<BotDeliveryAttempt>)>, BotStateDbError> {
    let fetch = i64::from(limit.saturating_add(1));
    let mut statement = connection.prepare_cached(
        "SELECT delivery_id FROM bot_delivery_receipt
         WHERE delivery_id > ?1 ORDER BY delivery_id LIMIT ?2",
    )?;
    let mut delivery_ids = statement
        .query_map(params![after, fetch], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let has_more = delivery_ids.len() > limit as usize;
    delivery_ids.truncate(limit as usize);
    let next_cursor = has_more.then(|| delivery_ids.last().cloned()).flatten();
    let items = delivery_ids
        .into_iter()
        .map(|delivery_id| {
            let receipt = delivery_receipt(connection, &delivery_id)?.ok_or_else(|| {
                BotStateDbError::Invariant(format!(
                    "listed delivery receipt disappeared: {delivery_id}"
                ))
            })?;
            let attempts = delivery_attempts(connection, &delivery_id)?;
            Ok((receipt, attempts))
        })
        .collect::<Result<Vec<_>, BotStateDbError>>()?;
    Ok(BotStatePage { items, next_cursor })
}

fn insert_delivery_attempt(
    connection: &Connection,
    attempt: &BotDeliveryAttempt,
) -> Result<usize, BotStateDbError> {
    let attempt_number = i64::from(attempt.attempt);
    let retry_at = attempt.retry_at_unix_ms.map(sqlite_integer).transpose()?;
    Ok(connection.execute(
        "INSERT OR REPLACE INTO bot_delivery_attempt(delivery_id, attempt, status, retry_at, body)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            attempt.delivery_id,
            attempt_number,
            delivery_status_name(attempt.status),
            retry_at,
            encode(attempt)?,
        ],
    )?)
}

fn save_delivery_outcome(
    connection: &mut Connection,
    attempt: &BotDeliveryAttempt,
    receipt: &BotDeliveryReceipt,
) -> Result<(), BotStateDbError> {
    let transaction = immediate(connection)?;
    insert_delivery_attempt(&transaction, attempt)?;
    upsert_delivery_receipt(&transaction, receipt)?;
    transaction.commit()?;
    Ok(())
}

fn save_delivery_receipt(
    connection: &Connection,
    receipt: &BotDeliveryReceipt,
) -> Result<(), BotStateDbError> {
    upsert_delivery_receipt(connection, receipt)?;
    Ok(())
}

fn upsert_delivery_receipt(
    connection: &Connection,
    receipt: &BotDeliveryReceipt,
) -> Result<usize, BotStateDbError> {
    Ok(connection.execute(
        "INSERT INTO bot_delivery_receipt(delivery_id, idempotency_key, status, body)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(delivery_id) DO UPDATE SET
             idempotency_key=excluded.idempotency_key,
             status=excluded.status,
             body=excluded.body",
        params![
            receipt.delivery_id,
            receipt.idempotency_key,
            delivery_status_name(receipt.status),
            encode(receipt)?,
        ],
    )?)
}

fn claim_send_lease_on_receipt(receipt: &mut BotDeliveryReceipt, now_unix_ms: u64, lease_ms: u64) {
    receipt.status = DeliveryStatus::Sending;
    receipt.generation = receipt.generation.saturating_add(1);
    receipt.lease_expires_at_unix_ms = Some(now_unix_ms.saturating_add(lease_ms));
    receipt.error_code = None;
}

fn claim_due_deliveries(
    connection: &mut Connection,
    now_unix_ms: u64,
) -> Result<Vec<String>, BotStateDbError> {
    claim_due_deliveries_by_kind(connection, now_unix_ms, false)
}

fn claim_due_reply_parts(
    connection: &mut Connection,
    now_unix_ms: u64,
) -> Result<Vec<String>, BotStateDbError> {
    claim_due_deliveries_by_kind(connection, now_unix_ms, true)
}

const CLAIM_DUE_BATCH_LIMIT: i64 = 64;

fn claim_due_deliveries_by_kind(
    connection: &mut Connection,
    now_unix_ms: u64,
    reply_parts: bool,
) -> Result<Vec<String>, BotStateDbError> {
    let transaction = immediate(connection)?;
    let now = sqlite_integer(now_unix_ms)?;
    let candidates = {
        let query = if reply_parts {
            "SELECT r.delivery_id, r.status, r.body, d.body
             FROM bot_delivery_receipt r
             JOIN bot_reply_delivery_part p ON p.delivery_id=r.delivery_id
             JOIN bot_reply_delivery d ON d.reply_id=p.reply_id
             WHERE r.status IN ('pending', 'retry_scheduled', 'sending')
               AND NOT EXISTS (
                   SELECT 1
                   FROM bot_reply_delivery_part earlier
                   JOIN bot_delivery_receipt earlier_receipt
                     ON earlier_receipt.delivery_id=earlier.delivery_id
                   WHERE earlier.reply_id=p.reply_id
                     AND earlier.part_index<p.part_index
                     AND earlier_receipt.status IN (
                         'pending', 'retry_scheduled', 'sending', 'reconcile_required'
                     )
               )
               AND (
                   r.status = 'pending'
                   OR (
                       r.status = 'retry_scheduled'
                       AND EXISTS (
                           SELECT 1 FROM bot_delivery_attempt a
                           WHERE a.delivery_id=r.delivery_id
                             AND a.status='retry_scheduled'
                             AND a.retry_at<=?1
                             AND a.attempt=(
                                 SELECT MAX(b.attempt) FROM bot_delivery_attempt b
                                 WHERE b.delivery_id=r.delivery_id
                             )
                       )
                   )
                   OR (
                       r.status = 'sending'
                       AND (
                           json_extract(r.body, '$.lease_expires_at_unix_ms') IS NULL
                           OR json_extract(r.body, '$.lease_expires_at_unix_ms') <= ?1
                       )
                   )
               )
             ORDER BY p.reply_id, p.part_index
             LIMIT ?2"
        } else {
            "SELECT r.delivery_id, r.status, r.body, NULL
             FROM bot_delivery_receipt r
             WHERE r.status IN ('pending', 'retry_scheduled', 'sending')
               AND NOT EXISTS (
                   SELECT 1 FROM bot_reply_delivery_part p WHERE p.delivery_id=r.delivery_id
               )
               AND (
                   r.status = 'pending'
                   OR (
                       r.status = 'retry_scheduled'
                       AND EXISTS (
                           SELECT 1 FROM bot_delivery_attempt a
                           WHERE a.delivery_id=r.delivery_id
                             AND a.status='retry_scheduled'
                             AND a.retry_at<=?1
                             AND a.attempt=(
                                 SELECT MAX(b.attempt) FROM bot_delivery_attempt b
                                 WHERE b.delivery_id=r.delivery_id
                             )
                       )
                   )
                   OR (
                       r.status = 'sending'
                       AND (
                           json_extract(r.body, '$.lease_expires_at_unix_ms') IS NULL
                           OR json_extract(r.body, '$.lease_expires_at_unix_ms') <= ?1
                       )
                   )
               )
             ORDER BY r.delivery_id
             LIMIT ?2"
        };
        let mut statement = transaction.prepare_cached(query)?;
        statement
            .query_map(params![now, CLAIM_DUE_BATCH_LIMIT], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut claimed = Vec::new();
    for (delivery_id, status, body, reply_body) in candidates {
        if reply_body
            .as_deref()
            .and_then(|body| decode::<BotReplyDeliveryRequest>(body).ok())
            .is_some_and(|request| request.occupancy_only)
        {
            continue;
        }
        let mut receipt: BotDeliveryReceipt = decode(&body)?;
        match status.as_str() {
            "sending" => {
                let expired = receipt
                    .lease_expires_at_unix_ms
                    .is_none_or(|expires| expires <= now_unix_ms);
                if expired {
                    receipt.status = DeliveryStatus::ReconcileRequired;
                    receipt.lease_expires_at_unix_ms = None;
                    receipt.error_code = Some("delivery.reconcile_required".into());
                    transaction.execute(
                        "UPDATE bot_delivery_receipt SET status='reconcile_required', body=?2
                         WHERE delivery_id=?1 AND status='sending'",
                        params![delivery_id, encode(&receipt)?],
                    )?;
                }
            }
            "pending" => {
                claim_send_lease_on_receipt(&mut receipt, now_unix_ms, DELIVERY_SEND_LEASE_MS);
                let changed = transaction.execute(
                    "UPDATE bot_delivery_receipt SET status='sending', body=?2
                     WHERE delivery_id=?1 AND status='pending'",
                    params![delivery_id, encode(&receipt)?],
                )?;
                if changed == 1 {
                    claimed.push(delivery_id);
                    if reply_parts {
                        break;
                    }
                }
            }
            "retry_scheduled" => {
                // Due-ness was already decided by the `EXISTS` on
                // `bot_delivery_attempt` in the candidate query above, with the same
                // `now` and the same max-attempt predicate. Re-asking per candidate
                // compiled a statement and ran the subquery again up to
                // `CLAIM_DUE_BATCH_LIMIT` times per tick. Nothing in this loop writes
                // `bot_delivery_attempt`, so the answer cannot change underneath us.
                claim_send_lease_on_receipt(&mut receipt, now_unix_ms, DELIVERY_SEND_LEASE_MS);
                let changed = transaction.execute(
                    "UPDATE bot_delivery_receipt SET status='sending', body=?2
                     WHERE delivery_id=?1 AND status='retry_scheduled'",
                    params![delivery_id, encode(&receipt)?],
                )?;
                if changed == 1 {
                    claimed.push(delivery_id);
                    if reply_parts {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    transaction.commit()?;
    Ok(claimed)
}

fn is_reply_part(connection: &Connection, delivery_id: &str) -> Result<bool, BotStateDbError> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM bot_reply_delivery_part WHERE delivery_id=?1",
            params![delivery_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn begin_send_delivery(
    connection: &mut Connection,
    delivery_id: &str,
    attempt: BotDeliveryAttempt,
    now_unix_ms: u64,
    lease_ms: u64,
) -> Result<BotDeliveryReceipt, BotStateDbError> {
    let transaction = immediate(connection)?;
    let body = transaction
        .query_row(
            "SELECT body FROM bot_delivery_receipt WHERE delivery_id=?1",
            params![delivery_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| {
            BotStateDbError::Invariant(format!("delivery receipt missing: {delivery_id}"))
        })?;
    let mut receipt: BotDeliveryReceipt = decode(&body)?;
    let claimable = match receipt.status {
        DeliveryStatus::Pending
        | DeliveryStatus::RetryScheduled
        | DeliveryStatus::PermanentlyFailed
        | DeliveryStatus::ReconcileRequired => true,
        DeliveryStatus::Sending => receipt
            .lease_expires_at_unix_ms
            .is_some_and(|expires| expires > now_unix_ms),
        _ => false,
    };
    if !claimable {
        return Err(BotStateDbError::Invariant(format!(
            "delivery {delivery_id} is not claimable in status {:?}",
            receipt.status
        )));
    }
    if !matches!(receipt.status, DeliveryStatus::Sending)
        || receipt
            .lease_expires_at_unix_ms
            .is_none_or(|expires| expires <= now_unix_ms)
    {
        claim_send_lease_on_receipt(&mut receipt, now_unix_ms, lease_ms);
    }
    upsert_delivery_receipt(&transaction, &receipt)?;
    insert_delivery_attempt(&transaction, &attempt)?;
    transaction.commit()?;
    Ok(receipt)
}

fn create_interaction(
    connection: &mut Connection,
    session: &BotInteractionSession,
) -> Result<bool, BotStateDbError> {
    let transaction = immediate(connection)?;
    let version = sqlite_integer(session.version)?;
    let changed = transaction.execute(
        "INSERT OR IGNORE INTO bot_interaction(session_id, origin_key, status, version, body)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            session.session_id,
            session.conversation.origin_key(),
            interaction_status_name(session.status),
            version,
            encode(session)?,
        ],
    )?;
    transaction.commit()?;
    Ok(changed == 1)
}

fn active_interactions(
    connection: &Connection,
    origin_key: &str,
) -> Result<Vec<BotInteractionSession>, BotStateDbError> {
    let mut statement = connection.prepare_cached(
        "SELECT body FROM bot_interaction
         WHERE origin_key=?1 AND status='waiting' ORDER BY session_id",
    )?;
    statement
        .query_map(params![origin_key], |row| row.get::<_, String>(0))?
        .map(|body| decode(&body?))
        .collect()
}

fn compare_and_set_interaction(
    connection: &mut Connection,
    expected_version: u64,
    session: &BotInteractionSession,
) -> Result<bool, BotStateDbError> {
    let transaction = immediate(connection)?;
    let version = sqlite_integer(session.version)?;
    let expected_version = sqlite_integer(expected_version)?;
    let changed = transaction.execute(
        "UPDATE bot_interaction SET status=?2, version=?3, body=?4
         WHERE session_id=?1 AND version=?5",
        params![
            session.session_id,
            interaction_status_name(session.status),
            version,
            encode(session)?,
            expected_version,
        ],
    )?;
    transaction.commit()?;
    Ok(changed == 1)
}

fn recover_waiting_interactions(
    connection: &Connection,
) -> Result<Vec<BotInteractionSession>, BotStateDbError> {
    let mut statement = connection.prepare_cached(
        "SELECT body FROM bot_interaction WHERE status='waiting' ORDER BY session_id",
    )?;
    statement
        .query_map([], |row| row.get::<_, String>(0))?
        .map(|body| decode(&body?))
        .collect()
}

fn interaction_page(
    connection: &Connection,
    after: &str,
    limit: u32,
) -> Result<BotStatePage<BotInteractionSession>, BotStateDbError> {
    let fetch = i64::from(limit.saturating_add(1));
    let mut statement = connection.prepare_cached(
        "SELECT session_id, body FROM bot_interaction
         WHERE session_id > ?1 ORDER BY session_id LIMIT ?2",
    )?;
    let mut rows = statement
        .query_map(params![after, fetch], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = has_more
        .then(|| rows.last().map(|(session_id, _)| session_id.clone()))
        .flatten();
    let items = rows
        .into_iter()
        .map(|(_, body)| decode(&body))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(BotStatePage { items, next_cursor })
}

fn management_revision(connection: &Connection) -> Result<u64, BotStateDbError> {
    let revision = connection.query_row(
        "SELECT revision FROM bot_management_meta WHERE singleton=1",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    sqlite_unsigned(revision, "management revision")
}

fn management_audits(
    connection: &Connection,
    limit: u32,
) -> Result<Vec<BotManagementAuditRecord>, BotStateDbError> {
    let mut statement = connection.prepare_cached(
        "SELECT audit_id, revision, actor_id, action, result, created_at_unix_ms
         FROM bot_management_audit ORDER BY revision DESC LIMIT ?1",
    )?;
    statement
        .query_map(params![i64::from(limit)], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?
        .map(|row| {
            let (audit_id, revision, actor_id, action, result, created_at_unix_ms) = row?;
            Ok(BotManagementAuditRecord {
                audit_id: format!("audit-{audit_id}"),
                revision: sqlite_unsigned(revision, "management audit revision")?,
                actor_id,
                action,
                result: decode(&result)?,
                created_at_unix_ms: sqlite_unsigned(
                    created_at_unix_ms,
                    "management audit timestamp",
                )?,
            })
        })
        .collect()
}

fn management_audit_by_revision(
    connection: &Connection,
    revision: u64,
) -> Result<BotManagementAuditRecord, BotStateDbError> {
    let (audit_id, actor_id, action, result, created_at_unix_ms) = connection.query_row(
        "SELECT audit_id, actor_id, action, result, created_at_unix_ms
         FROM bot_management_audit WHERE revision=?1",
        params![sqlite_integer(revision)?],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        },
    )?;
    Ok(BotManagementAuditRecord {
        audit_id: format!("audit-{audit_id}"),
        revision,
        actor_id,
        action: action.to_owned(),
        result: decode(&result)?,
        created_at_unix_ms: sqlite_unsigned(created_at_unix_ms, "management audit timestamp")?,
    })
}

fn begin_management_operation(
    connection: &mut Connection,
    operation_id: &str,
    expected_revision: u64,
    actor_id: &str,
    action: &str,
    created_at_unix_ms: u64,
) -> Result<BotManagementOperationReservation, BotStateDbError> {
    let transaction = immediate(connection)?;
    let existing = transaction
        .query_row(
            "SELECT expected_revision, actor_id, action, audit_revision
             FROM bot_management_operation WHERE operation_id=?1",
            params![operation_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .optional()?;
    if let Some((stored_revision, stored_actor, stored_action, audit_revision)) = existing {
        if sqlite_unsigned(stored_revision, "management expected revision")? != expected_revision
            || stored_actor != actor_id
            || stored_action != action
        {
            return Err(BotStateDbError::Conflict);
        }
        return audit_revision.map_or_else(
            || Ok(BotManagementOperationReservation::Resume),
            |revision| {
                management_audit_by_revision(
                    &transaction,
                    sqlite_unsigned(revision, "management audit revision")?,
                )
                .map(BotManagementOperationReservation::Completed)
            },
        );
    }
    if management_revision(&transaction)? != expected_revision {
        return Err(BotStateDbError::Conflict);
    }
    let pending_exists = transaction.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM bot_management_operation
             WHERE expected_revision=?1 AND audit_revision IS NULL
         )",
        params![sqlite_integer(expected_revision)?],
        |row| row.get::<_, bool>(0),
    )?;
    if pending_exists {
        return Err(BotStateDbError::Conflict);
    }
    transaction.execute(
        "INSERT INTO bot_management_operation(
             operation_id, expected_revision, actor_id, action, audit_revision, created_at_unix_ms
         ) VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
        params![
            operation_id,
            sqlite_integer(expected_revision)?,
            actor_id,
            action,
            sqlite_integer(created_at_unix_ms)?,
        ],
    )?;
    transaction.commit()?;
    Ok(BotManagementOperationReservation::Reserved)
}

fn complete_management_operation(
    connection: &mut Connection,
    operation_id: &str,
    action: &str,
    result: serde_json::Value,
    created_at_unix_ms: u64,
) -> Result<BotManagementAuditRecord, BotStateDbError> {
    let transaction = immediate(connection)?;
    let (expected_revision, actor_id, _operation_fingerprint, audit_revision) = transaction
        .query_row(
            "SELECT expected_revision, actor_id, action, audit_revision
         FROM bot_management_operation WHERE operation_id=?1",
            params![operation_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )?;
    if let Some(revision) = audit_revision {
        return management_audit_by_revision(
            &transaction,
            sqlite_unsigned(revision, "management audit revision")?,
        );
    }
    let expected_revision = sqlite_unsigned(expected_revision, "management expected revision")?;
    if management_revision(&transaction)? != expected_revision {
        return Err(BotStateDbError::Conflict);
    }
    let revision = expected_revision.saturating_add(1);
    transaction.execute(
        "UPDATE bot_management_meta SET revision=?1 WHERE singleton=1 AND revision=?2",
        params![
            sqlite_integer(revision)?,
            sqlite_integer(expected_revision)?
        ],
    )?;
    transaction.execute(
        "INSERT INTO bot_management_audit(
             revision, actor_id, action, result, created_at_unix_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            sqlite_integer(revision)?,
            actor_id,
            action,
            encode(&result)?,
            sqlite_integer(created_at_unix_ms)?,
        ],
    )?;
    let audit_id = format!("audit-{}", transaction.last_insert_rowid());
    transaction.execute(
        "UPDATE bot_management_operation SET audit_revision=?2 WHERE operation_id=?1",
        params![operation_id, sqlite_integer(revision)?],
    )?;
    transaction.commit()?;
    Ok(BotManagementAuditRecord {
        audit_id,
        revision,
        actor_id,
        action: action.to_owned(),
        result,
        created_at_unix_ms,
    })
}

fn commit_management_audit(
    connection: &mut Connection,
    expected_revision: u64,
    actor_id: &str,
    action: &str,
    result: serde_json::Value,
    created_at_unix_ms: u64,
) -> Result<Option<BotManagementAuditRecord>, BotStateDbError> {
    let transaction = immediate(connection)?;
    let current = management_revision(&transaction)?;
    if current != expected_revision {
        return Ok(None);
    }
    let revision = current.saturating_add(1);
    let changed = transaction.execute(
        "UPDATE bot_management_meta SET revision=?1
         WHERE singleton=1 AND revision=?2",
        params![sqlite_integer(revision)?, sqlite_integer(current)?],
    )?;
    if changed != 1 {
        return Ok(None);
    }
    transaction.execute(
        "INSERT INTO bot_management_audit(
             revision, actor_id, action, result, created_at_unix_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            sqlite_integer(revision)?,
            actor_id,
            action,
            encode(&result)?,
            sqlite_integer(created_at_unix_ms)?,
        ],
    )?;
    let audit_id = format!("audit-{}", transaction.last_insert_rowid());
    transaction.commit()?;
    Ok(Some(BotManagementAuditRecord {
        audit_id,
        revision,
        actor_id: actor_id.to_owned(),
        action: action.to_owned(),
        result,
        created_at_unix_ms,
    }))
}

fn bounded_page_limit(limit: u32) -> u32 {
    limit.clamp(1, 100)
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn user_table_names(connection: &Connection) -> Result<Vec<String>, BotStateDbError> {
    let mut statement = connection.prepare_cached(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    statement
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(BotStateDbError::from)
}

fn inspect_columns(
    connection: &Connection,
    table: &str,
) -> Result<Vec<BotDatabaseColumnInfo>, BotStateDbError> {
    let mut statement =
        connection.prepare(&format!("PRAGMA table_info({})", quote_ident(table)))?;
    statement
        .query_map([], |row| {
            Ok(BotDatabaseColumnInfo {
                name: row.get(1)?,
                decl_type: row.get::<_, String>(2).unwrap_or_default(),
                primary_key: row.get::<_, i64>(5)? != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(BotStateDbError::from)
}

fn inspect_row_count(connection: &Connection, table: &str) -> Result<u64, BotStateDbError> {
    let count: i64 = connection.query_row(
        &format!("SELECT COUNT(*) FROM {}", quote_ident(table)),
        [],
        |row| row.get(0),
    )?;
    sqlite_unsigned(count, "row_count")
}

fn inspect_snapshot_tables(
    connection: &Connection,
) -> Result<Vec<BotDatabaseTableInfo>, BotStateDbError> {
    user_table_names(connection)?
        .into_iter()
        .map(|name| {
            Ok(BotDatabaseTableInfo {
                row_count: inspect_row_count(connection, &name)?,
                columns: inspect_columns(connection, &name)?,
                name,
            })
        })
        .collect()
}

fn inspect_value(value: ValueRef<'_>) -> serde_json::Value {
    match value {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Integer(value) => serde_json::Value::from(value),
        ValueRef::Real(value) => serde_json::json!(value),
        ValueRef::Text(value) => {
            serde_json::Value::String(String::from_utf8_lossy(value).into_owned())
        }
        ValueRef::Blob(value) => serde_json::json!({
            "$type": "blob",
            "bytes": value.len(),
        }),
    }
}

fn inspect_rows(
    connection: &Connection,
    table: &str,
    after: &str,
    limit: u32,
) -> Result<BotDatabaseTablePage, BotStateDbError> {
    if !user_table_names(connection)?
        .iter()
        .any(|name| name == table)
    {
        return Err(BotStateDbError::InvalidConfiguration(format!(
            "unknown table: {table}"
        )));
    }
    let columns = inspect_columns(connection, table)?;
    let column_names = columns
        .into_iter()
        .map(|column| column.name)
        .collect::<Vec<_>>();
    let select_list = column_names
        .iter()
        .map(|name| quote_ident(name))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT rowid, {select_list} FROM {} WHERE rowid > ?1 ORDER BY rowid LIMIT ?2",
        quote_ident(table)
    );
    let after_rowid = if after.is_empty() {
        0
    } else {
        after.parse::<i64>().map_err(|_| {
            BotStateDbError::InvalidConfiguration(format!("invalid cursor: {after}"))
        })?
    };
    let mut statement = connection.prepare(&sql)?;
    let mut query = statement.query(params![after_rowid, i64::from(limit)])?;
    let mut rows = Vec::new();
    let mut last_rowid = None;
    while let Some(row) = query.next()? {
        last_rowid = Some(row.get(0)?);
        let mut values = Vec::with_capacity(column_names.len());
        for index in 0..column_names.len() {
            values.push(inspect_value(row.get_ref(index + 1)?));
        }
        rows.push(values);
    }
    let next_cursor = if rows.len() == usize::try_from(limit).unwrap_or(usize::MAX) {
        last_rowid.map(|rowid: i64| rowid.to_string())
    } else {
        None
    };
    Ok(BotDatabaseTablePage {
        table: table.to_owned(),
        columns: column_names,
        rows,
        next_cursor,
    })
}

fn encode(value: &impl serde::Serialize) -> Result<String, BotStateDbError> {
    serde_json::to_string(value).map_err(|error| BotStateDbError::Serialization(error.to_string()))
}

fn decode<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, BotStateDbError> {
    serde_json::from_str(body).map_err(|error| BotStateDbError::Serialization(error.to_string()))
}

fn sqlite_integer(value: u64) -> Result<i64, BotStateDbError> {
    i64::try_from(value)
        .map_err(|_| BotStateDbError::Invariant(format!("integer {value} exceeds SQLite range")))
}

fn sqlite_unsigned(value: i64, name: &str) -> Result<u64, BotStateDbError> {
    u64::try_from(value)
        .map_err(|_| BotStateDbError::Invariant(format!("negative {name}: {value}")))
}

fn delivery_status_name(status: DeliveryStatus) -> &'static str {
    match status {
        DeliveryStatus::Pending => "pending",
        DeliveryStatus::Sending => "sending",
        DeliveryStatus::Succeeded => "succeeded",
        DeliveryStatus::RetryScheduled => "retry_scheduled",
        DeliveryStatus::PermanentlyFailed => "permanently_failed",
        DeliveryStatus::Cancelled => "cancelled",
        DeliveryStatus::Previewed => "previewed",
        DeliveryStatus::ReconcileRequired => "reconcile_required",
    }
}

fn interaction_status_name(status: InteractionStatus) -> &'static str {
    match status {
        InteractionStatus::Waiting => "waiting",
        InteractionStatus::Completed => "completed",
        InteractionStatus::TimedOut => "timed_out",
        InteractionStatus::Cancelled => "cancelled",
        InteractionStatus::Failed => "failed",
    }
}

fn record_icl(
    connection: &Connection,
    origin_key: &str,
    entry: &ConversationIclEntry,
    max_count: usize,
) -> Result<(), BotStateDbError> {
    let next_seq: i64 = connection.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM bot_conversation_icl WHERE origin_key = ?1",
        params![origin_key],
        |row| row.get(0),
    )?;
    connection.execute(
        "INSERT INTO bot_conversation_icl(origin_key, seq, body) VALUES(?1, ?2, ?3)",
        params![origin_key, next_seq, encode(entry)?],
    )?;
    if max_count > 0 {
        let keep = i64::try_from(max_count).unwrap_or(i64::MAX);
        if next_seq > keep {
            connection.execute(
                "DELETE FROM bot_conversation_icl WHERE origin_key = ?1 AND seq <= ?2",
                params![origin_key, next_seq - keep],
            )?;
        }
    }
    Ok(())
}

fn load_icl(
    connection: &Connection,
    origin_key: &str,
    max_count: usize,
) -> Result<Vec<ConversationIclEntry>, BotStateDbError> {
    // Only the newest `max_count` entries survive, so the limit belongs in SQL.
    // Reading the whole history and trimming afterwards decoded JSON for rows that
    // were discarded on the next line, on every message that attaches ICL.
    // `max_count == 0` keeps its "no limit" meaning.
    let limit: i64 = if max_count == 0 {
        -1
    } else {
        i64::try_from(max_count).unwrap_or(i64::MAX)
    };
    let mut statement = connection.prepare_cached(
        "SELECT body FROM bot_conversation_icl WHERE origin_key = ?1 ORDER BY seq DESC LIMIT ?2",
    )?;
    let rows = statement
        .query_map(params![origin_key, limit], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    // `DESC` gives the newest first; callers expect chronological order.
    let mut entries = rows
        .into_iter()
        .rev()
        .map(|body| decode(&body))
        .collect::<Result<Vec<ConversationIclEntry>, _>>()?;
    entries.shrink_to_fit();
    Ok(entries)
}

fn upsert_persona(connection: &Connection, persona: &BotPersona) -> Result<(), BotStateDbError> {
    connection.execute(
        "INSERT INTO bot_persona(persona_id, body) VALUES(?1, ?2)
         ON CONFLICT(persona_id) DO UPDATE SET body = excluded.body",
        params![persona.persona_id, encode(persona)?],
    )?;
    Ok(())
}

fn list_personas(connection: &Connection) -> Result<Vec<BotPersona>, BotStateDbError> {
    let mut statement =
        connection.prepare_cached("SELECT body FROM bot_persona ORDER BY persona_id")?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|body| decode(&body))
        .collect::<Result<Vec<_>, _>>()
}

fn get_persona(
    connection: &Connection,
    persona_id: &str,
) -> Result<Option<BotPersona>, BotStateDbError> {
    connection
        .query_row(
            "SELECT body FROM bot_persona WHERE persona_id = ?1",
            params![persona_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|body| decode(&body))
        .transpose()
}

fn bind_conversation_persona(
    connection: &Connection,
    origin_key: &str,
    persona_id: &str,
) -> Result<(), BotStateDbError> {
    if get_persona(connection, persona_id)?.is_none() {
        return Err(BotStateDbError::Invariant(format!(
            "unknown persona {persona_id}"
        )));
    }
    connection.execute(
        "INSERT INTO bot_persona_binding(origin_key, persona_id) VALUES(?1, ?2)
         ON CONFLICT(origin_key) DO UPDATE SET persona_id = excluded.persona_id",
        params![origin_key, persona_id],
    )?;
    Ok(())
}

fn conversation_persona(
    connection: &Connection,
    origin_key: &str,
) -> Result<Option<String>, BotStateDbError> {
    connection
        .query_row(
            "SELECT persona_id FROM bot_persona_binding WHERE origin_key = ?1",
            params![origin_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(BotStateDbError::from)
}

/// These methods are `async` by trait signature but answer through the blocking channel on
/// purpose.
///
/// Their callers are bot Runners driven by `TaskAwaitRunnerAdapter`, which polls the runner future
/// on a Host sync worker with a noop waker: a future that returns `Pending` for anything other
/// than a Core task await is rejected as `runner.awaitable_unsupported`. Awaiting the DB actor
/// here would therefore fail the batch rather than yield. Callers that really are on an async
/// executor (the Web Console) must announce the blocking section at their own boundary.
#[async_trait]
impl ConversationRepository for BotStateDbRepository {
    async fn session_binding(
        &self,
        binding_key: &str,
    ) -> Result<Option<AgentSessionBinding>, ConversationError> {
        let binding_key = binding_key.to_owned();
        self.call_sync(false, move |connection| {
            session_binding(connection, &binding_key)
        })
        .map_err(conversation_error)
    }

    async fn compare_and_set_session_binding(
        &self,
        binding_key: &str,
        expected_generation: Option<u64>,
        binding: AgentSessionBinding,
    ) -> Result<(), ConversationError> {
        let binding_key = binding_key.to_owned();
        let changed = self
            .call_sync(true, move |connection| {
                compare_and_set_session_binding(
                    connection,
                    &binding_key,
                    expected_generation,
                    &binding,
                )
            })
            .map_err(conversation_error)?;
        changed
            .then_some(())
            .ok_or(ConversationError::GenerationConflict)
    }

    async fn begin_agent_event(
        &self,
        binding_key: &str,
        event_id: &str,
        turn_id: &str,
    ) -> Result<AgentEventClaim, ConversationError> {
        let binding_key = binding_key.to_owned();
        let event_id = event_id.to_owned();
        let turn_id = turn_id.to_owned();
        self.call_sync(true, move |connection| {
            begin_agent_event(connection, &binding_key, &event_id, &turn_id)
        })
        .map_err(conversation_error)
    }

    async fn complete_agent_event(
        &self,
        binding_key: &str,
        event_id: &str,
    ) -> Result<(), ConversationError> {
        let binding_key = binding_key.to_owned();
        let event_id = event_id.to_owned();
        let changed = self
            .call_sync(false, move |connection| {
                complete_agent_event(connection, &binding_key, &event_id)
            })
            .map_err(conversation_error)?;
        changed
            .then_some(())
            .ok_or_else(|| ConversationError::Repository("agent event claim was not found".into()))
    }
}

#[async_trait]
impl DeliveryRepository for BotStateDbRepository {
    async fn reserve(
        &self,
        request: &BotActiveDeliveryRequest,
    ) -> Result<Option<BotDeliveryReceipt>, DeliveryError> {
        let request = request.clone();
        match self
            .call_sync(true, move |connection| {
                reserve_delivery(connection, &request)
            })
            .map_err(delivery_error)?
        {
            DeliveryReservation::Reserved => Ok(None),
            DeliveryReservation::Existing(receipt) => Ok(Some(receipt)),
            DeliveryReservation::Conflict => Err(DeliveryError::Conflict),
        }
    }

    async fn request(&self, delivery_id: &str) -> Result<BotActiveDeliveryRequest, DeliveryError> {
        let delivery_id = delivery_id.to_owned();
        self.call_sync(false, move |connection| {
            delivery_request(connection, &delivery_id)
        })
        .map_err(delivery_error)?
        .ok_or(DeliveryError::NotFound)
    }

    async fn receipt(&self, delivery_id: &str) -> Result<BotDeliveryReceipt, DeliveryError> {
        let delivery_id = delivery_id.to_owned();
        self.call_sync(false, move |connection| {
            delivery_receipt(connection, &delivery_id)
        })
        .map_err(delivery_error)?
        .ok_or(DeliveryError::NotFound)
    }

    async fn attempts(&self, delivery_id: &str) -> Result<Vec<BotDeliveryAttempt>, DeliveryError> {
        let delivery_id = delivery_id.to_owned();
        self.call_sync(false, move |connection| {
            delivery_attempts(connection, &delivery_id)
        })
        .map_err(delivery_error)
    }

    /// One actor round trip for the whole claimed batch instead of two per id.
    /// The rows are read on the database thread with the same helpers, so the
    /// missing-row behaviour stays `NotFound` for the batch.
    async fn requests_and_receipts(
        &self,
        delivery_ids: &[String],
    ) -> Result<Vec<(BotActiveDeliveryRequest, BotDeliveryReceipt)>, DeliveryError> {
        let delivery_ids = delivery_ids.to_vec();
        self.call_sync(false, move |connection| {
            let mut rows = Vec::with_capacity(delivery_ids.len());
            for delivery_id in &delivery_ids {
                let (Some(request), Some(receipt)) = (
                    delivery_request(connection, delivery_id)?,
                    delivery_receipt(connection, delivery_id)?,
                ) else {
                    return Ok(None);
                };
                rows.push((request, receipt));
            }
            Ok(Some(rows))
        })
        .map_err(delivery_error)?
        .ok_or(DeliveryError::NotFound)
    }

    /// One actor round trip for every part of a reply bundle.
    async fn receipts(
        &self,
        delivery_ids: &[String],
    ) -> Result<Vec<BotDeliveryReceipt>, DeliveryError> {
        let delivery_ids = delivery_ids.to_vec();
        self.call_sync(false, move |connection| {
            let mut receipts = Vec::with_capacity(delivery_ids.len());
            for delivery_id in &delivery_ids {
                let Some(receipt) = delivery_receipt(connection, delivery_id)? else {
                    return Ok(None);
                };
                receipts.push(receipt);
            }
            Ok(Some(receipts))
        })
        .map_err(delivery_error)?
        .ok_or(DeliveryError::NotFound)
    }

    async fn save_outcome(
        &self,
        attempt: BotDeliveryAttempt,
        receipt: BotDeliveryReceipt,
    ) -> Result<(), DeliveryError> {
        self.call_sync(true, move |connection| {
            save_delivery_outcome(connection, &attempt, &receipt)
        })
        .map_err(delivery_error)
    }

    async fn save_receipt(&self, receipt: BotDeliveryReceipt) -> Result<(), DeliveryError> {
        self.call_sync(false, move |connection| {
            save_delivery_receipt(connection, &receipt)
        })
        .map_err(delivery_error)
    }

    async fn claim_due_delivery_ids(&self, now_unix_ms: u64) -> Result<Vec<String>, DeliveryError> {
        self.call_sync(true, move |connection| {
            claim_due_deliveries(connection, now_unix_ms)
        })
        .map_err(delivery_error)
    }

    async fn begin_send(
        &self,
        delivery_id: &str,
        attempt: BotDeliveryAttempt,
        now_unix_ms: u64,
        lease_ms: u64,
    ) -> Result<BotDeliveryReceipt, DeliveryError> {
        let delivery_id = delivery_id.to_owned();
        self.call_sync(true, move |connection| {
            begin_send_delivery(connection, &delivery_id, attempt, now_unix_ms, lease_ms)
        })
        .map_err(delivery_error)
    }
}

#[async_trait]
impl ReplyDeliveryRepository for BotStateDbRepository {
    async fn reserve_reply(
        &self,
        request: &BotReplyDeliveryRequest,
    ) -> Result<Option<BotReplyDeliveryReceipt>, DeliveryError> {
        let request = request.clone();
        match self
            .call_sync(true, move |connection| {
                reserve_reply_delivery(connection, &request)
            })
            .map_err(delivery_error)?
        {
            ReplyDeliveryReservation::Reserved => Ok(None),
            ReplyDeliveryReservation::Existing(receipt) => Ok(Some(receipt)),
            ReplyDeliveryReservation::Conflict => Err(DeliveryError::Conflict),
        }
    }

    async fn reply_receipt(
        &self,
        reply_id: &str,
    ) -> Result<BotReplyDeliveryReceipt, DeliveryError> {
        let reply_id = reply_id.to_owned();
        self.call_sync(false, move |connection| {
            reply_delivery_receipt_by_id(connection, &reply_id)
        })
        .map_err(delivery_error)?
        .ok_or(DeliveryError::NotFound)
    }

    async fn claim_due_reply_part_id(
        &self,
        now_unix_ms: u64,
    ) -> Result<Option<String>, DeliveryError> {
        self.call_sync(true, move |connection| {
            claim_due_reply_parts(connection, now_unix_ms)
        })
        .map(|ids| ids.into_iter().next())
        .map_err(delivery_error)
    }

    async fn is_reply_part(&self, delivery_id: &str) -> Result<bool, DeliveryError> {
        let delivery_id = delivery_id.to_owned();
        self.call_sync(false, move |connection| {
            is_reply_part(connection, &delivery_id)
        })
        .map_err(delivery_error)
    }
}

impl InteractionRepository for BotStateDbRepository {
    fn create(&self, session: BotInteractionSession) -> Result<(), InteractionError> {
        let changed = self
            .call_sync(true, move |connection| {
                create_interaction(connection, &session)
            })
            .map_err(interaction_error)?;
        changed
            .then_some(())
            .ok_or(InteractionError::WaiterConflict)
    }

    fn active_for_origin(
        &self,
        origin_key: &str,
    ) -> Result<Vec<BotInteractionSession>, InteractionError> {
        let origin_key = origin_key.to_owned();
        self.call_sync(false, move |connection| {
            active_interactions(connection, &origin_key)
        })
        .map_err(interaction_error)
    }

    fn compare_and_set(
        &self,
        expected_version: u64,
        session: BotInteractionSession,
    ) -> Result<(), InteractionError> {
        let changed = self
            .call_sync(true, move |connection| {
                compare_and_set_interaction(connection, expected_version, &session)
            })
            .map_err(interaction_error)?;
        changed
            .then_some(())
            .ok_or(InteractionError::GenerationConflict)
    }

    /// One round trip and one transaction for the whole expired set.
    ///
    /// The rows keep their own `body`, so this stays a compare-and-set per
    /// session rather than a single blanket `UPDATE`; what it removes is the
    /// per-session handoff to the database actor, not the per-row write.
    fn time_out_expired(&self, expired: &[BotInteractionSession]) -> Result<(), InteractionError> {
        let expired = expired
            .iter()
            .map(|session| {
                let mut session = session.clone();
                session.status = InteractionStatus::TimedOut;
                session.version += 1;
                session
            })
            .collect::<Vec<_>>();
        let committed = self
            .call_sync(true, move |connection| {
                for session in &expired {
                    if !compare_and_set_interaction(connection, session.version - 1, session)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            })
            .map_err(interaction_error)?;
        committed
            .then_some(())
            .ok_or(InteractionError::GenerationConflict)
    }

    fn recover_waiting(&self) -> Result<Vec<BotInteractionSession>, InteractionError> {
        self.call_sync(false, move |connection| {
            recover_waiting_interactions(connection)
        })
        .map_err(interaction_error)
    }
}

fn conversation_error(error: BotStateDbError) -> ConversationError {
    ConversationError::Repository(state_db_error_message(error))
}

fn delivery_error(error: BotStateDbError) -> DeliveryError {
    DeliveryError::Repository(state_db_error_message(error))
}

fn interaction_error(error: BotStateDbError) -> InteractionError {
    InteractionError::Repository(state_db_error_message(error))
}

impl ConversationContextStore for BotStateDbRepository {
    fn record_icl(
        &self,
        origin_key: &str,
        entry: ConversationIclEntry,
        max_count: usize,
    ) -> Result<(), String> {
        let origin_key = origin_key.to_owned();
        self.call_sync(true, move |connection| {
            record_icl(connection, &origin_key, &entry, max_count)
        })
        .map_err(state_db_error_message)
    }

    fn load_icl(
        &self,
        origin_key: &str,
        max_count: usize,
    ) -> Result<Vec<ConversationIclEntry>, String> {
        let origin_key = origin_key.to_owned();
        self.call_sync(false, move |connection| {
            load_icl(connection, &origin_key, max_count)
        })
        .map_err(state_db_error_message)
    }
}

impl PersonaStore for BotStateDbRepository {
    fn upsert(&self, persona: BotPersona) -> Result<(), String> {
        self.call_sync(true, move |connection| upsert_persona(connection, &persona))
            .map_err(state_db_error_message)
    }

    fn list(&self) -> Result<Vec<BotPersona>, String> {
        self.call_sync(false, move |connection| list_personas(connection))
            .map_err(state_db_error_message)
    }

    fn get(&self, persona_id: &str) -> Result<Option<BotPersona>, String> {
        let persona_id = persona_id.to_owned();
        self.call_sync(false, move |connection| {
            get_persona(connection, &persona_id)
        })
        .map_err(state_db_error_message)
    }

    fn bind_conversation(&self, origin_key: &str, persona_id: &str) -> Result<(), String> {
        let origin_key = origin_key.to_owned();
        let persona_id = persona_id.to_owned();
        self.call_sync(true, move |connection| {
            bind_conversation_persona(connection, &origin_key, &persona_id)
        })
        .map_err(state_db_error_message)
    }

    fn conversation_persona(&self, origin_key: &str) -> Result<Option<String>, String> {
        let origin_key = origin_key.to_owned();
        self.call_sync(false, move |connection| {
            conversation_persona(connection, &origin_key)
        })
        .map_err(state_db_error_message)
    }
}

fn state_db_error_message(error: BotStateDbError) -> String {
    let message = error.to_string();
    drop(error);
    message
}

#[derive(Debug, Error)]
pub enum BotStateDbError {
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error("serialization failed: {0}")]
    Serialization(String),
    #[error("database invariant failed: {0}")]
    Invariant(String),
    #[error("database actor failed to start: {0}")]
    ActorStart(String),
    #[error("database actor stopped")]
    ActorStopped,
    #[error("database actor queue is full")]
    QueueFull,
    #[error("invalid database actor configuration: {0}")]
    InvalidConfiguration(String),
    #[error("state revision or idempotency reservation conflicts with the request")]
    Conflict,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::AtomicUsize;

    use mutsuki_bot_protocol::{
        BotConversationKind, BotDeliveryContent, BotDeliveryReceipt, BotInteractionSession,
        BotReplyDeliveryPart, DeliveryPolicy, DeliveryStatus, InteractionScope,
        InteractionWaitSpec, MessageSegment, QQ_CONVERSATION_REF_VERSION, QqConversationRef,
    };
    use tokio::task::JoinSet;

    use super::*;

    #[test]
    fn delivery_receipt_status_column_and_index_migrate_from_legacy_schema() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("legacy.db");
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "PRAGMA foreign_keys=ON;
                     CREATE TABLE bot_delivery_request(
                         delivery_id TEXT PRIMARY KEY,
                         idempotency_key TEXT NOT NULL UNIQUE,
                         body TEXT NOT NULL
                     );
                     CREATE TABLE bot_delivery_receipt(
                         delivery_id TEXT PRIMARY KEY,
                         idempotency_key TEXT NOT NULL UNIQUE,
                         body TEXT NOT NULL,
                         FOREIGN KEY(delivery_id) REFERENCES bot_delivery_request(delivery_id)
                     );",
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO bot_delivery_request(delivery_id, idempotency_key, body)
                     VALUES (?1, ?2, ?3)",
                    rusqlite::params!["d1", "k1", "{}"],
                )
                .unwrap();
            let body = serde_json::to_string(&BotDeliveryReceipt {
                delivery_id: "d1".into(),
                idempotency_key: "k1".into(),
                status: DeliveryStatus::RetryScheduled,
                attempt_count: 1,
                platform_message_ids: vec![],
                part_receipts: vec![],
                delivered_at_unix_ms: None,
                error_code: None,
                generation: 0,
                lease_expires_at_unix_ms: None,
            })
            .unwrap();
            connection
                .execute(
                    "INSERT INTO bot_delivery_receipt(delivery_id, idempotency_key, body)
                     VALUES (?1, ?2, ?3)",
                    rusqlite::params!["d1", "k1", body],
                )
                .unwrap();
        }

        let _repository = BotStateDbRepository::open(&path).unwrap();
        let connection = rusqlite::Connection::open(&path).unwrap();
        let columns = connection
            .prepare("PRAGMA table_info(bot_delivery_receipt)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(columns.iter().any(|column| column == "status"));
        let status: String = connection
            .query_row(
                "SELECT status FROM bot_delivery_receipt WHERE delivery_id='d1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "retry_scheduled");
        let index_sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='index' AND name='bot_delivery_receipt_status'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(index_sql.contains("status"));
        assert!(index_sql.contains("delivery_id"));
    }

    #[test]
    fn load_icl_keeps_the_newest_entries_in_chronological_order() {
        let root = tempfile::tempdir().unwrap();
        let repository = BotStateDbRepository::open(root.path().join("state.db")).unwrap();
        // Retention is applied on write with a generous cap so the read side is what
        // this exercises.
        for index in 0..6_u64 {
            repository
                .record_icl(
                    "group:g1",
                    ConversationIclEntry {
                        actor_id: "u1".into(),
                        display_name: None,
                        text: format!("m{index}"),
                        time_ms: index.cast_signed(),
                    },
                    100,
                )
                .unwrap();
        }

        // The window is the *newest* entries, still oldest-first: an ICL prompt built
        // from a reversed window would feed the model the conversation backwards.
        let window = repository.load_icl("group:g1", 3).unwrap();
        let texts: Vec<_> = window.iter().map(|entry| entry.text.as_str()).collect();
        assert_eq!(texts, vec!["m3", "m4", "m5"]);

        // 0 means "no limit" and must not be turned into an empty result.
        let all = repository.load_icl("group:g1", 0).unwrap();
        let all_texts: Vec<_> = all.iter().map(|entry| entry.text.as_str()).collect();
        assert_eq!(all_texts, vec!["m0", "m1", "m2", "m3", "m4", "m5"]);

        // A window wider than the history returns everything, unpadded.
        assert_eq!(repository.load_icl("group:g1", 50).unwrap().len(), 6);
    }

    #[test]
    fn icl_and_persona_persist_on_shared_repository() {
        let root = tempfile::tempdir().unwrap();
        let repository = BotStateDbRepository::open(root.path().join("state.db")).unwrap();
        repository
            .record_icl(
                "group:g1",
                ConversationIclEntry {
                    actor_id: "u1".into(),
                    display_name: Some("Alice".into()),
                    text: "hello".into(),
                    time_ms: 1,
                },
                20,
            )
            .unwrap();
        let entries = repository.load_icl("group:g1", 20).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text, "hello");

        repository
            .upsert(BotPersona {
                persona_id: "guide".into(),
                name: "向导".into(),
                system_prompt: "hi".into(),
                begin_dialogs: Vec::new(),
                allowed_tools: None,
                agent_runtime_profile_id: "qq-guide".into(),
            })
            .unwrap();
        repository.bind_conversation("group:g1", "guide").unwrap();
        assert_eq!(
            repository
                .conversation_persona("group:g1")
                .unwrap()
                .as_deref(),
            Some("guide")
        );
    }

    #[tokio::test]
    async fn claim_due_drains_beyond_batch_limit_without_starving_on_sending() {
        let root = tempfile::tempdir().unwrap();
        let repository =
            Arc::new(BotStateDbRepository::open(root.path().join("state.db")).unwrap());
        let conversation = conversation();
        let count = 80_usize;
        for index in 0..count {
            let request = delivery(
                &conversation,
                &format!("batch-{index:03}"),
                &format!("batch-key-{index:03}"),
            );
            assert!(repository.reserve(&request).await.unwrap().is_none());
        }

        let mut claimed = Vec::new();
        loop {
            let batch = repository.claim_due_delivery_ids(10).await.unwrap();
            if batch.is_empty() {
                break;
            }
            assert!(batch.len() <= usize::try_from(super::CLAIM_DUE_BATCH_LIMIT).unwrap());
            claimed.extend(batch);
        }
        assert_eq!(claimed.len(), count);
        assert_eq!(claimed[0], "batch-000");
    }

    #[tokio::test]
    async fn pending_after_reserve_and_expired_sending_are_recoverable() {
        let root = tempfile::tempdir().unwrap();
        let repository =
            Arc::new(BotStateDbRepository::open(root.path().join("state.db")).unwrap());
        let conversation = conversation();
        let request = delivery(&conversation, "crash-pending", "crash-key");
        assert!(repository.reserve(&request).await.unwrap().is_none());

        assert_eq!(
            repository.claim_due_delivery_ids(10).await.unwrap(),
            vec!["crash-pending"]
        );
        let sending = repository.receipt("crash-pending").await.unwrap();
        assert_eq!(sending.status, DeliveryStatus::Sending);
        assert!(sending.lease_expires_at_unix_ms.is_some());

        let mut expired = sending.clone();
        expired.lease_expires_at_unix_ms = Some(5);
        repository.save_receipt(expired).await.unwrap();
        assert!(
            repository
                .claim_due_delivery_ids(10)
                .await
                .unwrap()
                .is_empty()
        );
        let reconciled = repository.receipt("crash-pending").await.unwrap();
        assert_eq!(reconciled.status, DeliveryStatus::ReconcileRequired);

        let retry_request = delivery(&conversation, "race", "race-key");
        assert!(repository.reserve(&retry_request).await.unwrap().is_none());
        repository
            .save_outcome(retry_attempt("race"), retry_receipt("race", "race-key"))
            .await
            .unwrap();
        let mut claims = JoinSet::new();
        for _ in 0..32 {
            let repository = repository.clone();
            claims.spawn(async move { repository.claim_due_delivery_ids(150).await.unwrap() });
        }
        let mut winners = 0;
        while let Some(result) = claims.join_next().await {
            if result.unwrap() == vec!["race".to_owned()] {
                winners += 1;
            }
        }
        assert_eq!(winners, 1);
    }

    #[tokio::test]
    async fn reply_bundle_reservation_is_atomic_idempotent_and_claims_one_part_at_a_time() {
        let root = tempfile::tempdir().unwrap();
        let repository = BotStateDbRepository::open(root.path().join("state.db")).unwrap();
        let conversation = conversation();
        let request = reply_delivery(&conversation, "reply", &["reply:part:2", "reply:part:10"]);

        assert!(repository.reserve_reply(&request).await.unwrap().is_none());
        let duplicate = repository.reserve_reply(&request).await.unwrap().unwrap();
        assert_eq!(duplicate.part_receipts.len(), 2);
        assert!(
            duplicate
                .part_receipts
                .iter()
                .all(|receipt| receipt.status == DeliveryStatus::Pending)
        );
        assert!(
            repository
                .claim_due_delivery_ids(10)
                .await
                .unwrap()
                .is_empty()
        );

        assert_eq!(
            repository.claim_due_reply_part_id(10).await.unwrap(),
            Some("reply:part:2".into())
        );
        assert_eq!(
            repository.receipt("reply:part:2").await.unwrap().status,
            DeliveryStatus::Sending
        );
        assert_eq!(
            repository.receipt("reply:part:10").await.unwrap().status,
            DeliveryStatus::Pending
        );
        assert!(
            repository
                .claim_due_reply_part_id(10)
                .await
                .unwrap()
                .is_none()
        );
        let mut first_succeeded = repository.receipt("reply:part:2").await.unwrap();
        first_succeeded.status = DeliveryStatus::Succeeded;
        first_succeeded.lease_expires_at_unix_ms = None;
        repository.save_receipt(first_succeeded).await.unwrap();
        assert_eq!(
            repository.claim_due_reply_part_id(10).await.unwrap(),
            Some("reply:part:10".into())
        );

        let occupied = delivery(&conversation, "occupied", "occupied-key");
        assert!(repository.reserve(&occupied).await.unwrap().is_none());
        let conflicting = reply_delivery(
            &conversation,
            "conflicting-reply",
            &["new-part", "occupied"],
        );
        assert_eq!(
            repository.reserve_reply(&conflicting).await.unwrap_err(),
            DeliveryError::Conflict
        );
        assert_eq!(
            repository
                .reply_receipt("conflicting-reply")
                .await
                .unwrap_err(),
            DeliveryError::NotFound
        );
        assert_eq!(
            repository.request("new-part").await.unwrap_err(),
            DeliveryError::NotFound
        );
    }

    #[tokio::test]
    async fn unsent_reply_bundle_can_replace_parts_until_a_part_succeeds() {
        let root = tempfile::tempdir().unwrap();
        let repository = BotStateDbRepository::open(root.path().join("state.db")).unwrap();
        let conversation = conversation();
        let request = reply_delivery(&conversation, "replace-reply", &["replace:part:0"]);
        assert!(repository.reserve_reply(&request).await.unwrap().is_none());

        let mut upgraded = request.clone();
        upgraded.parts[0].content.reply_to = Some("quoted-message".into());
        upgraded.parts.push(BotReplyDeliveryPart {
            part_id: "replace:part:1".into(),
            content: BotDeliveryContent {
                segments: vec![MessageSegment::text("second")],
                summary: None,
                reply_to: Some("quoted-message".into()),
            },
            not_before_unix_ms: None,
        });
        assert!(repository.reserve_reply(&upgraded).await.unwrap().is_none());
        let receipt = repository.reply_receipt("replace-reply").await.unwrap();
        assert_eq!(receipt.part_receipts.len(), 2);
        assert!(
            receipt
                .part_receipts
                .iter()
                .all(|part| part.status == DeliveryStatus::Pending)
        );
        assert_eq!(
            repository
                .request("replace:part:1")
                .await
                .unwrap()
                .content
                .reply_to
                .as_deref(),
            Some("quoted-message")
        );

        let mut first = repository.receipt("replace:part:0").await.unwrap();
        first.status = DeliveryStatus::Succeeded;
        first.lease_expires_at_unix_ms = None;
        repository.save_receipt(first).await.unwrap();
        let mut after_send = upgraded.clone();
        after_send.parts[0].content.reply_to = Some("rewrite-succeeded".into());
        assert_eq!(
            repository.reserve_reply(&after_send).await.unwrap_err(),
            DeliveryError::Conflict
        );
        assert_eq!(
            repository
                .request("replace:part:0")
                .await
                .unwrap()
                .content
                .reply_to
                .as_deref(),
            Some("quoted-message")
        );
    }

    #[tokio::test]
    async fn occupancy_only_reply_is_not_due_until_submit_clears_hold() {
        let root = tempfile::tempdir().unwrap();
        let repository = BotStateDbRepository::open(root.path().join("state.db")).unwrap();
        let conversation = conversation();
        let mut draft = reply_delivery(&conversation, "hold-reply", &["hold:part:0"]);
        draft.occupancy_only = true;
        assert!(repository.reserve_reply(&draft).await.unwrap().is_none());
        assert!(
            repository
                .claim_due_reply_part_id(10)
                .await
                .unwrap()
                .is_none()
        );

        let mut finalized = draft.clone();
        finalized.occupancy_only = false;
        finalized.parts[0].content.reply_to = Some("quoted".into());
        assert!(
            repository
                .reserve_reply(&finalized)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            repository.claim_due_reply_part_id(10).await.unwrap(),
            Some("hold:part:0".into())
        );
        assert_eq!(
            repository
                .request("hold:part:0")
                .await
                .unwrap()
                .content
                .reply_to
                .as_deref(),
            Some("quoted")
        );
    }

    #[tokio::test]
    async fn state_recovers_after_reopen_and_uses_one_actor_connection() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("bot-state.db");
        let repository = BotStateDbRepository::open(&path).unwrap();
        let conversation = conversation();
        repository
            .compare_and_set_session_binding(
                &conversation.origin_key(),
                None,
                binding(&conversation, 1),
            )
            .await
            .unwrap();
        assert_eq!(
            repository
                .begin_agent_event(&conversation.origin_key(), "event", "turn")
                .await
                .unwrap(),
            AgentEventClaim::New
        );
        repository
            .complete_agent_event(&conversation.origin_key(), "event")
            .await
            .unwrap();
        let request = delivery(&conversation, "delivery", "key");
        assert!(repository.reserve(&request).await.unwrap().is_none());
        repository
            .save_outcome(retry_attempt("delivery"), retry_receipt("delivery", "key"))
            .await
            .unwrap();
        repository.create(interaction(&conversation)).unwrap();
        assert_eq!(repository.metrics().connection_open_count, 1);
        assert_eq!(repository.journal_mode(), "wal");
        drop(repository);

        let reopened = BotStateDbRepository::open(&path).unwrap();
        assert_eq!(reopened.journal_mode(), "wal");
        assert_eq!(
            reopened
                .session_binding(&conversation.origin_key())
                .await
                .unwrap()
                .unwrap()
                .session_id,
            "session"
        );
        assert_eq!(
            reopened
                .begin_agent_event(&conversation.origin_key(), "event", "turn")
                .await
                .unwrap(),
            AgentEventClaim::Completed
        );
        assert_eq!(
            reopened.claim_due_delivery_ids(150).await.unwrap(),
            vec!["delivery"]
        );
        assert_eq!(reopened.recover_waiting().unwrap().len(), 1);
        assert_eq!(reopened.metrics().connection_open_count, 1);
    }

    #[tokio::test]
    async fn management_revision_audit_and_stable_pages_survive_restart() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("management.db");
        let repository = BotStateDbRepository::open(&path).unwrap();
        let conversation = conversation();
        for (delivery_id, key) in [("delivery-b", "key-b"), ("delivery-a", "key-a")] {
            assert!(
                repository
                    .reserve(&delivery(&conversation, delivery_id, key))
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        let mut second_interaction = interaction(&conversation);
        second_interaction.session_id = "interaction-b".into();
        repository.create(second_interaction).unwrap();
        let mut first_interaction = interaction(&conversation);
        first_interaction.session_id = "interaction-a".into();
        repository.create(first_interaction).unwrap();

        let deliveries = repository.delivery_page(None, 1).unwrap();
        assert_eq!(deliveries.items[0].0.delivery_id, "delivery-a");
        assert_eq!(deliveries.next_cursor.as_deref(), Some("delivery-a"));
        let interactions = repository.interaction_page(None, 1).unwrap();
        assert_eq!(interactions.items[0].session_id, "interaction-a");
        assert_eq!(interactions.next_cursor.as_deref(), Some("interaction-a"));

        let audit = repository
            .commit_management_audit(
                0,
                "local-web-console",
                "delivery_preview",
                serde_json::json!({"delivery_id": "delivery-a"}),
                100,
            )
            .unwrap()
            .unwrap();
        assert_eq!(audit.revision, 1);
        assert!(
            repository
                .commit_management_audit(0, "forged", "retry", serde_json::Value::Null, 101)
                .unwrap()
                .is_none()
        );
        drop(repository);

        let reopened = BotStateDbRepository::open(path).unwrap();
        assert_eq!(reopened.management_revision().unwrap(), 1);
        let audits = reopened.management_audits(10).unwrap();
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].actor_id, "local-web-console");
        assert_eq!(audits[0].result["delivery_id"], "delivery-a");
    }

    #[test]
    fn management_operation_reservation_is_durable_and_replayable() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("management-operation.db");
        let repository = BotStateDbRepository::open(&path).unwrap();
        assert_eq!(
            repository
                .begin_management_operation("op-1", 0, "console", "send_test", 10)
                .unwrap(),
            BotManagementOperationReservation::Reserved
        );
        assert!(matches!(
            repository.begin_management_operation("op-2", 0, "console", "send_test", 10),
            Err(BotStateDbError::Conflict)
        ));
        drop(repository);

        let reopened = BotStateDbRepository::open(&path).unwrap();
        assert_eq!(
            reopened
                .begin_management_operation("op-1", 0, "console", "send_test", 11)
                .unwrap(),
            BotManagementOperationReservation::Resume
        );
        let audit = reopened
            .complete_management_operation(
                "op-1",
                "send_test",
                serde_json::json!({"message_id": "m1"}),
                12,
            )
            .unwrap();
        assert_eq!(audit.revision, 1);
        assert_eq!(
            reopened
                .begin_management_operation("op-1", 0, "console", "send_test", 13)
                .unwrap(),
            BotManagementOperationReservation::Completed(audit)
        );
        assert!(matches!(
            reopened.begin_management_operation("op-1", 0, "forged", "send_test", 14),
            Err(BotStateDbError::Conflict)
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn sixty_four_way_reserve_claim_and_cas_are_atomic() {
        let root = tempfile::tempdir().unwrap();
        let repository =
            Arc::new(BotStateDbRepository::open(root.path().join("state.db")).unwrap());
        let conversation = conversation();

        let mut reservations = JoinSet::new();
        for index in 0_u32..64 {
            let repository = repository.clone();
            let request = delivery(&conversation, &format!("delivery-{index}"), "shared-key");
            reservations.spawn(async move { repository.reserve(&request).await.unwrap() });
        }
        let mut reserved = 0;
        let mut existing = 0;
        while let Some(result) = reservations.join_next().await {
            if result.unwrap().is_none() {
                reserved += 1;
            } else {
                existing += 1;
            }
        }
        assert_eq!((reserved, existing), (1, 63));

        let binding_key = conversation.origin_key();
        repository
            .compare_and_set_session_binding(&binding_key, None, binding(&conversation, 1))
            .await
            .unwrap();
        let mut cas = JoinSet::new();
        for index in 0..64 {
            let repository = repository.clone();
            let binding_key = binding_key.clone();
            let mut next = binding(&conversation, 2);
            next.session_version = index;
            cas.spawn(async move {
                repository
                    .compare_and_set_session_binding(&binding_key, Some(1), next)
                    .await
            });
        }
        let mut cas_successes = 0;
        while let Some(result) = cas.join_next().await {
            if result.unwrap().is_ok() {
                cas_successes += 1;
            }
        }
        assert_eq!(cas_successes, 1);

        let waiting = interaction(&conversation);
        repository.create(waiting.clone()).unwrap();
        let mut interaction_cas = JoinSet::new();
        for index in 0..64 {
            let repository = repository.clone();
            let mut next = waiting.clone();
            next.version = 2;
            next.retries_remaining = index + 1;
            interaction_cas.spawn(async move { repository.compare_and_set(1, next) });
        }
        let mut interaction_cas_successes = 0;
        while let Some(result) = interaction_cas.join_next().await {
            if result.unwrap().is_ok() {
                interaction_cas_successes += 1;
            }
        }
        assert_eq!(interaction_cas_successes, 1);

        let mut claims = JoinSet::new();
        for _ in 0..64 {
            let repository = repository.clone();
            let binding_key = binding_key.clone();
            claims.spawn(async move {
                repository
                    .begin_agent_event(&binding_key, "event-64", "turn")
                    .await
                    .unwrap()
            });
        }
        let mut claim_counts = BTreeMap::new();
        while let Some(result) = claims.join_next().await {
            *claim_counts
                .entry(format!("{:?}", result.unwrap()))
                .or_insert(0) += 1;
        }
        assert_eq!(claim_counts.get("New"), Some(&1));
        assert_eq!(claim_counts.get("ResumePending"), Some(&63));
        assert_eq!(repository.metrics().connection_open_count, 1);
        assert_eq!(repository.metrics().busy_count, 0);
        assert!(repository.metrics().transaction_count >= 194);
    }

    #[tokio::test]
    async fn failed_delivery_outcome_rolls_back_attempt_and_receipt_together() {
        let root = tempfile::tempdir().unwrap();
        let repository = BotStateDbRepository::open(root.path().join("state.db")).unwrap();
        let conversation = conversation();
        let first = delivery(&conversation, "first", "key-first");
        let second = delivery(&conversation, "second", "key-second");
        assert!(repository.reserve(&first).await.unwrap().is_none());
        assert!(repository.reserve(&second).await.unwrap().is_none());
        repository
            .save_receipt(retry_receipt("first", "key-first"))
            .await
            .unwrap();

        let error = repository
            .save_outcome(
                retry_attempt("second"),
                retry_receipt("second", "key-first"),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, DeliveryError::Repository(_)));
        assert!(repository.attempts("second").await.unwrap().is_empty());
        let second_receipt = repository.receipt("second").await.unwrap();
        assert_eq!(second_receipt.status, DeliveryStatus::Pending);
        assert_eq!(second_receipt.idempotency_key, "key-second");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn database_wait_does_not_block_async_runtime_worker() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.db");
        let repository = BotStateDbRepository::open(&path).unwrap();
        let conversation = conversation();

        let lock_path = path.clone();
        let holder = std::thread::spawn(move || {
            let connection = Connection::open(lock_path).unwrap();
            connection.busy_timeout(Duration::from_millis(1)).unwrap();
            connection.execute_batch("BEGIN IMMEDIATE").unwrap();
            std::thread::sleep(Duration::from_millis(250));
            connection.execute_batch("COMMIT").unwrap();
        });
        std::thread::sleep(Duration::from_millis(30));

        let progress = Arc::new(AtomicUsize::new(0));
        let ticker_progress = progress.clone();
        let ticker = async {
            let deadline = Instant::now() + Duration::from_millis(200);
            while Instant::now() < deadline {
                ticker_progress.fetch_add(1, Ordering::Relaxed);
                tokio::task::yield_now().await;
            }
        };

        let binding_key = conversation.origin_key();
        let (write_result, _) = tokio::join!(
            repository.compare_and_set_session_binding(
                &binding_key,
                None,
                binding(&conversation, 1)
            ),
            ticker
        );
        write_result.unwrap();
        holder.join().unwrap();
        assert!(
            progress.load(Ordering::Relaxed) > 0,
            "async runtime worker must keep scheduling while the DB actor waits on SQLITE_BUSY"
        );
        assert_eq!(repository.metrics().connection_open_count, 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn external_writer_contention_stays_correct() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.db");
        let repository = Arc::new(BotStateDbRepository::open(&path).unwrap());
        let conversation = conversation();
        let binding_key = conversation.origin_key();
        repository
            .compare_and_set_session_binding(&binding_key, None, binding(&conversation, 1))
            .await
            .unwrap();
        let waiting = interaction(&conversation);
        repository.create(waiting.clone()).unwrap();

        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let lock_path = path.clone();
        let stop_flag = stop.clone();
        let contender = std::thread::spawn(move || {
            let connection = Connection::open(lock_path).unwrap();
            connection.busy_timeout(BUSY_TIMEOUT).unwrap();
            while !stop_flag.load(Ordering::Relaxed) {
                connection.execute_batch("BEGIN IMMEDIATE").unwrap();
                std::thread::sleep(Duration::from_millis(5));
                connection.execute_batch("COMMIT").unwrap();
                std::thread::sleep(Duration::from_millis(1));
            }
        });

        let mut jobs = JoinSet::new();
        let iterations = 256_u32;
        for index in 0..iterations {
            let repository = repository.clone();
            let conversation = conversation.clone();
            let binding_key = binding_key.clone();
            let waiting = waiting.clone();
            jobs.spawn(async move {
                let started = Instant::now();
                match index % 4 {
                    0 => {
                        let request = delivery(
                            &conversation,
                            &format!("delivery-{index}"),
                            &format!("key-{index}"),
                        );
                        repository.reserve(&request).await.unwrap();
                    }
                    1 => {
                        let _ = repository
                            .begin_agent_event(&binding_key, &format!("event-{index}"), "turn")
                            .await
                            .unwrap();
                    }
                    2 => {
                        let mut next = waiting.clone();
                        next.version = u64::from(index) + 2;
                        next.retries_remaining = index;
                        let _ = repository.compare_and_set(1, next);
                    }
                    _ => {
                        let mut next = binding(&conversation, 2);
                        next.session_version = u64::from(index);
                        let _ = repository
                            .compare_and_set_session_binding(&binding_key, Some(1), next)
                            .await;
                    }
                }
                started.elapsed()
            });
        }

        let mut latencies = Vec::with_capacity(iterations as usize);
        while let Some(result) = jobs.join_next().await {
            latencies.push(result.unwrap());
        }
        stop.store(true, Ordering::Relaxed);
        contender.join().unwrap();

        latencies.sort_unstable();
        let p95 = latencies[((latencies.len() * 95) / 100).saturating_sub(1)];
        let p99 = latencies[((latencies.len() * 99) / 100).saturating_sub(1)];
        eprintln!(
            "issue-164 contention: n={} p95={p95:?} p99={p99:?} open={} busy={}",
            latencies.len(),
            repository.metrics().connection_open_count,
            repository.metrics().busy_count
        );

        assert_eq!(repository.metrics().connection_open_count, 1);
        assert_eq!(
            repository.metrics().busy_count,
            0,
            "actor busy_timeout must absorb SQLITE_BUSY instead of surfacing it"
        );
        assert!(
            p95 < Duration::from_secs(2),
            "p95 under injected contention should stay under 2s, got {p95:?}"
        );
        assert!(
            p99 < Duration::from_secs(5),
            "p99 under injected contention should stay under busy_timeout, got {p99:?}"
        );
    }

    #[test]
    fn inspect_reads_live_schema_and_rejects_unknown_tables() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.db");
        let repository = BotStateDbRepository::open(&path).unwrap();
        let snapshot = repository.inspect_snapshot().unwrap();
        assert_eq!(snapshot.path, path.display().to_string());
        assert_eq!(snapshot.journal_mode, "wal");
        assert!(snapshot.size_bytes.unwrap_or(0) > 0);
        let meta = snapshot
            .tables
            .iter()
            .find(|table| table.name == "bot_management_meta")
            .expect("schema table");
        assert_eq!(meta.row_count, 1);
        assert!(meta.columns.iter().any(|column| column.name == "revision"));
        assert!(
            !snapshot
                .tables
                .iter()
                .any(|table| table.name.starts_with("sqlite_"))
        );

        let page = repository
            .inspect_rows("bot_management_meta", None, 10)
            .unwrap();
        assert_eq!(page.table, "bot_management_meta");
        assert!(page.columns.contains(&"revision".to_string()));
        assert_eq!(page.rows.len(), 1);
        assert!(page.next_cursor.is_none());
        assert!(repository.inspect_rows("sqlite_master", None, 10).is_err());
        assert!(repository.inspect_rows("not_a_table", None, 10).is_err());
        assert!(
            repository
                .inspect_rows("bot_management_meta;drop", None, 10)
                .is_err()
        );
    }

    #[tokio::test]
    async fn sandbox_history_roundtrip_survives_reopen() {
        use mutsuki_bot_sandbox::{
            SandboxAction, SandboxApi, SandboxHistoryKind, SandboxHistoryStore, SandboxMode,
            SandboxService, sandbox_user_id,
        };

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.db");
        let repository = Arc::new(BotStateDbRepository::open(&path).unwrap());
        let snapshot = repository.inspect_snapshot().unwrap();
        assert!(
            snapshot
                .tables
                .iter()
                .any(|table| table.name == "bot_sandbox_conversation")
        );
        assert!(
            snapshot
                .tables
                .iter()
                .any(|table| table.name == "bot_sandbox_sticker")
        );
        assert!(
            snapshot
                .tables
                .iter()
                .any(|table| table.name == "bot_sandbox_face")
        );

        let first = SandboxService::with_history("qq-main", repository.clone()).unwrap();
        first.set_runtime(Arc::new(NoopSandboxRuntime));
        let initial = first.snapshot("").await.unwrap();
        let group_id = initial
            .conversations
            .iter()
            .find(|item| item.kind == BotConversationKind::Group)
            .expect("seed group")
            .conversation_id
            .clone();
        first
            .write(
                "tester",
                mutsuki_bot_sandbox::SandboxWriteRequest {
                    operation_id: "op-hist".into(),
                    expected_revision: initial.revision,
                    action: SandboxAction::IngestAsUser {
                        conversation_id: group_id.clone(),
                        user_id: sandbox_user_id("Alice"),
                        text: "persisted".into(),
                        segments: vec![],
                        reply_to: None,
                    },
                },
            )
            .await
            .unwrap();
        first
            .upload_sticker("pack.png", "image/png", b"sticker-bytes".to_vec())
            .await
            .unwrap();
        let after = first.snapshot("").await.unwrap();
        first
            .write(
                "tester",
                mutsuki_bot_sandbox::SandboxWriteRequest {
                    operation_id: "op-face".into(),
                    expected_revision: after.revision,
                    action: SandboxAction::IngestAsUser {
                        conversation_id: group_id.clone(),
                        user_id: sandbox_user_id("Alice"),
                        text: String::new(),
                        segments: vec![MessageSegment::PlatformSpecific {
                            platform: "qqbot".into(),
                            kind: "face".into(),
                            payload: serde_json::json!({ "face_type": "6", "face_id": "0" }),
                        }],
                        reply_to: None,
                    },
                },
            )
            .await
            .unwrap();
        drop(first);
        drop(repository);

        let reopened = Arc::new(BotStateDbRepository::open(&path).unwrap());
        let restored = SandboxService::with_history("qq-main", reopened.clone()).unwrap();
        let messages = restored.messages(&group_id).await.unwrap();
        assert!(messages.iter().any(|item| item.text == "persisted"));
        assert_eq!(
            restored.snapshot("").await.unwrap().mode,
            SandboxMode::Simulate
        );
        let loaded = SandboxHistoryStore::load(reopened.as_ref()).unwrap();
        assert!(loaded.simulate.iter().any(|item| {
            item.messages
                .iter()
                .any(|message| message.text == "persisted")
        }));
        assert!(
            reopened
                .sandbox_conversations(SandboxHistoryKind::Simulate)
                .unwrap()
                .iter()
                .any(|item| item.conversation_id == group_id)
        );
        assert!(
            reopened
                .sandbox_messages(SandboxHistoryKind::Simulate, &group_id)
                .unwrap()
                .iter()
                .any(|item| item.text == "persisted")
        );
        assert!(loaded.stickers.iter().any(|item| item.name == "pack.png"));
        let sticker_id = loaded
            .stickers
            .iter()
            .find(|item| item.name == "pack.png")
            .expect("sticker")
            .content_hash
            .clone();
        assert_eq!(
            restored.sticker_blob(&sticker_id).await.unwrap().bytes,
            b"sticker-bytes"
        );
        assert!(
            loaded
                .faces
                .iter()
                .any(|item| item.face_key == "qq:6:0" && item.face_type == "6")
        );
    }

    #[tokio::test]
    async fn sandbox_conversation_digest_changes_for_anything_persist_writes() {
        use mutsuki_bot_sandbox::{
            SandboxAction, SandboxApi, SandboxHistoryStore, SandboxService, sandbox_user_id,
        };

        let root = tempfile::tempdir().unwrap();
        let repository =
            Arc::new(BotStateDbRepository::open(root.path().join("state.db")).unwrap());
        let service = SandboxService::with_history("qq-main", repository.clone()).unwrap();
        service.set_runtime(Arc::new(NoopSandboxRuntime));
        let initial = service.snapshot("").await.unwrap();
        let group_id = initial
            .conversations
            .iter()
            .find(|item| item.kind == BotConversationKind::Group)
            .expect("seed group")
            .conversation_id
            .clone();
        service
            .write(
                "tester",
                mutsuki_bot_sandbox::SandboxWriteRequest {
                    operation_id: "op-digest".into(),
                    expected_revision: initial.revision,
                    action: SandboxAction::IngestAsUser {
                        conversation_id: group_id.clone(),
                        user_id: sandbox_user_id("Alice"),
                        text: "first".into(),
                        segments: vec![],
                        reply_to: None,
                    },
                },
            )
            .await
            .unwrap();

        let loaded = SandboxHistoryStore::load(repository.as_ref()).unwrap();
        let conversation = loaded
            .simulate
            .iter()
            .find(|item| item.view.conversation_id == group_id)
            .expect("persisted conversation");
        let baseline = sandbox_history::conversation_digest(conversation).unwrap();

        // Skipping an unchanged conversation is only safe if the digest is stable for
        // identical content...
        assert_eq!(
            baseline,
            sandbox_history::conversation_digest(&conversation.clone()).unwrap()
        );

        // ...and differs for every part the upserts write. A collision here would
        // silently drop a real change from the database.
        let mut retitled = conversation.clone();
        retitled.view.title = format!("{}-renamed", retitled.view.title);
        assert_ne!(
            baseline,
            sandbox_history::conversation_digest(&retitled).unwrap(),
            "conversation row change must not be skipped"
        );

        let mut reactivated = conversation.clone();
        reactivated.view.active_message = !reactivated.view.active_message;
        assert_ne!(
            baseline,
            sandbox_history::conversation_digest(&reactivated).unwrap(),
            "active_message change must not be skipped"
        );

        let mut renamed_user = conversation.clone();
        let user = renamed_user.users.first_mut().expect("seed user");
        user.display_name = format!("{}-renamed", user.display_name);
        assert_ne!(
            baseline,
            sandbox_history::conversation_digest(&renamed_user).unwrap(),
            "user row change must not be skipped"
        );

        let mut edited_message = conversation.clone();
        let message = edited_message.messages.first_mut().expect("seed message");
        message.text = format!("{}-edited", message.text);
        assert_ne!(
            baseline,
            sandbox_history::conversation_digest(&edited_message).unwrap(),
            "message row change must not be skipped"
        );

        let mut appended = conversation.clone();
        let mut extra = appended.messages.last().expect("seed message").clone();
        extra.message_id = format!("{}-extra", extra.message_id);
        appended.messages.push(extra);
        assert_ne!(
            baseline,
            sandbox_history::conversation_digest(&appended).unwrap(),
            "appended message must not be skipped"
        );
    }

    #[tokio::test]
    async fn sandbox_history_load_keeps_blobs_lazy_and_empty_upsert_does_not_clobber() {
        use mutsuki_bot_sandbox::{SandboxApi, SandboxHistoryStore, SandboxService};

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.db");
        let repository = Arc::new(BotStateDbRepository::open(&path).unwrap());
        let first = SandboxService::with_history("qq-main", repository.clone()).unwrap();
        first.set_runtime(Arc::new(NoopSandboxRuntime));
        let uploaded = first
            .upload_media("pic.png", "image/png", b"image-bytes".to_vec())
            .await
            .unwrap();
        drop(first);
        drop(repository);

        let reopened = Arc::new(BotStateDbRepository::open(&path).unwrap());
        let loaded = SandboxHistoryStore::load(reopened.as_ref()).unwrap();
        let asset = loaded
            .media
            .iter()
            .find(|item| item.content_hash == uploaded.media_id)
            .expect("asset metadata");
        assert!(asset.bytes.is_empty());
        let blob = SandboxHistoryStore::load_media_blob(reopened.as_ref(), &uploaded.media_id)
            .unwrap()
            .expect("blob");
        assert_eq!(blob.bytes, b"image-bytes");

        SandboxHistoryStore::save(reopened.as_ref(), &loaded).unwrap();
        let blob_after_empty_upsert =
            SandboxHistoryStore::load_media_blob(reopened.as_ref(), &uploaded.media_id)
                .unwrap()
                .expect("blob after empty upsert");
        assert_eq!(blob_after_empty_upsert.bytes, b"image-bytes");
    }

    #[test]
    fn sandbox_history_migrates_segments_json_to_hash_refs() {
        use mutsuki_bot_sandbox::SandboxHistoryStore;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("legacy.db");
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            let conversation = serde_json::json!({
                "version": 1,
                "account_id": "qq-main",
                "kind": "group",
                "group_id": "sandbox:default"
            })
            .to_string();
            connection
                .execute_batch(
                    "PRAGMA foreign_keys=ON;
                 CREATE TABLE bot_sandbox_meta(singleton INTEGER PRIMARY KEY CHECK(singleton = 1), mode TEXT NOT NULL, account_id TEXT NOT NULL);
                 CREATE TABLE bot_sandbox_conversation(
                     store TEXT NOT NULL, conversation_id TEXT NOT NULL, account_id TEXT NOT NULL, kind TEXT NOT NULL,
                     title TEXT NOT NULL, avatar_url TEXT, conversation_json TEXT NOT NULL, last_preview TEXT,
                     last_activity_unix_ms INTEGER NOT NULL, message_count INTEGER NOT NULL, active_message INTEGER NOT NULL DEFAULT 0,
                     PRIMARY KEY(store, conversation_id));
                 CREATE TABLE bot_sandbox_message(
                     store TEXT NOT NULL, message_id TEXT NOT NULL, conversation_id TEXT NOT NULL, sender_id TEXT NOT NULL,
                     sender_name TEXT NOT NULL, role TEXT NOT NULL, text TEXT NOT NULL, segments_json TEXT NOT NULL,
                     reply_to TEXT, time_ms INTEGER NOT NULL, PRIMARY KEY(store, message_id));
                 CREATE TABLE bot_sandbox_media(
                     media_id TEXT PRIMARY KEY, mime TEXT NOT NULL, name TEXT NOT NULL, bytes BLOB NOT NULL,
                     created_at_unix_ms INTEGER NOT NULL);
                 INSERT INTO bot_sandbox_meta VALUES (1, 'simulate', 'qq-main');
                 INSERT INTO bot_sandbox_media VALUES ('media-old', 'image/png', 'pic.png', x'706e67', 1);",
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO bot_sandbox_conversation VALUES ('simulate', 'sandbox:default', 'qq-main', 'group', '群', NULL, ?1, 'pic', 1, 1, 0)",
                    rusqlite::params![conversation],
                )
                .unwrap();
            let segments = serde_json::json!([{
                "type": "platform_specific",
                "platform": "sandbox",
                "kind": "media",
                "payload": {"media_id": "media-old", "mime": "image/png", "name": "pic.png"}
            }]);
            connection
                .execute(
                    "INSERT INTO bot_sandbox_message VALUES ('simulate', 'msg-1', 'sandbox:default', 'u', 'Alice', 'user', '[图片]', ?1, NULL, 1)",
                    rusqlite::params![segments.to_string()],
                )
                .unwrap();
        }

        let repository = BotStateDbRepository::open(&path).unwrap();
        let loaded = SandboxHistoryStore::load(&repository).unwrap();
        let hash = mutsuki_bot_sandbox::hash_bytes(b"png");
        assert_eq!(loaded.media.len(), 1);
        assert_eq!(loaded.media[0].content_hash, hash);
        let message = loaded.simulate[0]
            .messages
            .iter()
            .find(|item| item.message_id == "msg-1")
            .unwrap();
        assert_eq!(message.refs[0].h.as_deref(), Some(hash.as_str()));
        assert!(repository.sandbox_media(&hash).unwrap().is_some());
    }

    #[test]
    fn sandbox_history_backfills_official_face_ids() {
        use mutsuki_bot_sandbox::SandboxHistoryStore;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("faces.db");
        {
            let _repository = BotStateDbRepository::open(&path).unwrap();
        }
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            let conversation = serde_json::json!({
                "version": 1,
                "account_id": "qq-main",
                "kind": "group",
                "group_id": "sandbox:default"
            })
            .to_string();
            connection
                .execute(
                    "INSERT INTO bot_sandbox_conversation(
                         store, conversation_id, account_id, kind, title, avatar_url, conversation_json,
                         last_preview, last_activity_unix_ms, message_count, active_message
                     ) VALUES ('simulate', 'sandbox:default', 'qq-main', 'group', '群', NULL, ?1, '', 1, 1, 0)",
                    rusqlite::params![conversation],
                )
                .unwrap();
            let refs = serde_json::json!([{ "t": "emoji", "at": 0, "id": "qq:6:0" }]);
            connection
                .execute(
                    "INSERT INTO bot_sandbox_message(
                         store, message_id, conversation_id, sender_id, sender_name, role, text, refs_json, reply_to, time_ms
                     ) VALUES ('simulate', 'msg-face', 'sandbox:default', 'u', 'Alice', 'user', '', ?1, NULL, 9)",
                    rusqlite::params![refs.to_string()],
                )
                .unwrap();
        }
        let repository = BotStateDbRepository::open(&path).unwrap();
        let loaded = SandboxHistoryStore::load(&repository).unwrap();
        assert!(
            loaded
                .faces
                .iter()
                .any(|item| item.face_key == "qq:6:0" && item.face_id == "0")
        );
    }

    #[test]
    fn sandbox_history_backfills_live_active_message_once() {
        use mutsuki_bot_sandbox::SandboxHistoryStore;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("active-message.db");
        let conversation = serde_json::json!({
            "version": 1,
            "account_id": "qq-main",
            "kind": "group",
            "group_id": "group-1"
        })
        .to_string();
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "PRAGMA user_version=9;
                 CREATE TABLE bot_sandbox_meta(singleton INTEGER PRIMARY KEY CHECK(singleton = 1), mode TEXT NOT NULL, account_id TEXT NOT NULL);
                 CREATE TABLE bot_sandbox_conversation(
                     store TEXT NOT NULL, conversation_id TEXT NOT NULL, account_id TEXT NOT NULL, kind TEXT NOT NULL,
                     title TEXT NOT NULL, avatar_url TEXT, conversation_json TEXT NOT NULL, last_preview TEXT,
                     last_activity_unix_ms INTEGER NOT NULL, message_count INTEGER NOT NULL, active_message INTEGER NOT NULL DEFAULT 0,
                     PRIMARY KEY(store, conversation_id));
                 INSERT INTO bot_sandbox_meta VALUES (1, 'live', 'qq-main');",
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO bot_sandbox_conversation VALUES ('live', 'live-group', 'qq-main', 'group', '群', NULL, ?1, '', 1, 0, 0)",
                    rusqlite::params![conversation],
                )
                .unwrap();
        }
        let loaded = {
            let repository = BotStateDbRepository::open(&path).unwrap();
            SandboxHistoryStore::load(&repository).unwrap()
        };
        assert!(loaded.live[0].view.active_message);
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute(
                "UPDATE bot_sandbox_conversation SET active_message=0 WHERE store='live'",
                [],
            )
            .unwrap();
        let loaded = {
            let repository = BotStateDbRepository::open(&path).unwrap();
            SandboxHistoryStore::load(&repository).unwrap()
        };
        assert!(!loaded.live[0].view.active_message);
    }

    #[test]
    fn blocking_repository_calls_survive_every_runtime_flavor() {
        use mutsuki_bot_sandbox::SandboxHistoryStore;

        let root = tempfile::tempdir().unwrap();
        let repository =
            Arc::new(BotStateDbRepository::open(root.path().join("flavor.db")).unwrap());

        // `block_in_place` panics outside a multi-threaded runtime, and the sandbox console reaches
        // this repository from both flavors plus plain Runner threads.
        SandboxHistoryStore::load(repository.as_ref()).unwrap();

        let current_thread = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        current_thread.block_on({
            let repository = repository.clone();
            async move { SandboxHistoryStore::load(repository.as_ref()).unwrap() }
        });

        let multi_thread = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        multi_thread.block_on(async move {
            // One worker: the repository wait must release it, otherwise the spawned task never runs.
            let peer = tokio::spawn(async { "alive" });
            SandboxHistoryStore::load(repository.as_ref()).unwrap();
            assert_eq!(peer.await.unwrap(), "alive");
        });
    }

    struct NoopSandboxRuntime;

    #[async_trait]
    impl mutsuki_bot_sandbox::SandboxRuntime for NoopSandboxRuntime {
        fn live_available(&self) -> bool {
            true
        }

        async fn ingest(
            &self,
            _event: mutsuki_bot_protocol::BotEvent,
        ) -> Result<(), mutsuki_bot_sandbox::SandboxError> {
            Ok(())
        }

        async fn deliver(
            &self,
            _operation_id: &str,
            _conversation: &QqConversationRef,
            _segments: &[MessageSegment],
            _reply_to: Option<&str>,
        ) -> Result<serde_json::Value, mutsuki_bot_sandbox::SandboxError> {
            Ok(serde_json::json!({ "delivered": true }))
        }
    }

    fn conversation() -> QqConversationRef {
        QqConversationRef {
            version: QQ_CONVERSATION_REF_VERSION,
            account_id: "main".into(),
            kind: BotConversationKind::Group,
            user_id: None,
            group_id: Some("group".into()),
            guild_id: None,
            channel_id: None,
            thread_id: None,
        }
    }

    fn binding(conversation: &QqConversationRef, generation: u64) -> AgentSessionBinding {
        AgentSessionBinding {
            origin_key: conversation.origin_key(),
            session_id: "session".into(),
            session_version: 2,
            last_event_sequence: 3,
            policy_revision: 2,
            generation,
        }
    }

    fn delivery(
        conversation: &QqConversationRef,
        delivery_id: &str,
        idempotency_key: &str,
    ) -> BotActiveDeliveryRequest {
        BotActiveDeliveryRequest {
            delivery_id: delivery_id.into(),
            idempotency_key: idempotency_key.into(),
            conversation: conversation.clone(),
            content: BotDeliveryContent {
                segments: vec![MessageSegment::Text {
                    text: "hello".into(),
                }],
                summary: None,
                reply_to: None,
            },
            policy: DeliveryPolicy {
                max_attempts: 3,
                initial_backoff_ms: 10,
                max_backoff_ms: 100,
                not_before_unix_ms: None,
                expires_at_unix_ms: None,
            },
            dry_run: false,
            source_execution_id: Some("execution".into()),
        }
    }

    fn reply_delivery(
        conversation: &QqConversationRef,
        reply_id: &str,
        part_ids: &[&str],
    ) -> BotReplyDeliveryRequest {
        BotReplyDeliveryRequest {
            reply_id: reply_id.into(),
            idempotency_key: format!("{reply_id}:key"),
            conversation: conversation.clone(),
            parts: part_ids
                .iter()
                .map(|part_id| BotReplyDeliveryPart {
                    part_id: (*part_id).into(),
                    content: BotDeliveryContent {
                        segments: vec![MessageSegment::text(format!("reply {part_id}"))],
                        summary: None,
                        reply_to: None,
                    },
                    not_before_unix_ms: None,
                })
                .collect(),
            policy: DeliveryPolicy {
                max_attempts: 3,
                initial_backoff_ms: 10,
                max_backoff_ms: 100,
                not_before_unix_ms: None,
                expires_at_unix_ms: None,
            },
            source_event_id: "event".into(),
            source_turn_id: "turn".into(),
            source_binding_key: None,
            occupancy_only: false,
        }
    }

    fn retry_attempt(delivery_id: &str) -> BotDeliveryAttempt {
        BotDeliveryAttempt {
            delivery_id: delivery_id.into(),
            attempt: 1,
            status: DeliveryStatus::RetryScheduled,
            started_at_unix_ms: 100,
            retry_at_unix_ms: Some(150),
            error_code: Some("qq.rate_limited".into()),
        }
    }

    fn retry_receipt(delivery_id: &str, idempotency_key: &str) -> BotDeliveryReceipt {
        BotDeliveryReceipt {
            delivery_id: delivery_id.into(),
            idempotency_key: idempotency_key.into(),
            status: DeliveryStatus::RetryScheduled,
            attempt_count: 1,
            platform_message_ids: Vec::new(),
            part_receipts: Vec::new(),
            delivered_at_unix_ms: None,
            error_code: Some("qq.rate_limited".into()),
            generation: 1,
            lease_expires_at_unix_ms: None,
        }
    }

    fn interaction(conversation: &QqConversationRef) -> BotInteractionSession {
        BotInteractionSession {
            session_id: "interaction".into(),
            conversation: conversation.clone(),
            scope: InteractionScope::ActorInConversation,
            actor_id: Some("actor".into()),
            state_ref_id: "state-ref".into(),
            wait: InteractionWaitSpec {
                event_kinds: Vec::new(),
                command: Some("verify".into()),
                predicate_service_id: None,
                timeout_at_unix_ms: 1_000,
                retry_prompt: None,
            },
            status: InteractionStatus::Waiting,
            generation: 1,
            version: 1,
            exclusive: true,
            retries_remaining: 2,
        }
    }
}
