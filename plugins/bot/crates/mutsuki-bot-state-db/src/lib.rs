use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use mutsuki_bot_conversation::{AgentEventClaim, ConversationError, ConversationRepository};
use mutsuki_bot_delivery::{
    DELIVERY_SEND_LEASE_MS, DeliveryError, DeliveryRepository, ReplyDeliveryRepository,
    reply_part_request,
};
use mutsuki_bot_flow::{BotFlowError, BotFlowRepository};
use mutsuki_bot_interaction::{InteractionError, InteractionRepository};
use mutsuki_bot_protocol::{
    AgentSessionBinding, BotActiveDeliveryRequest, BotDeliveryAttempt, BotDeliveryReceipt,
    BotFlowDraft, BotFlowDraftSaveRequest, BotFlowPublishRequest, BotFlowPublishedSnapshot,
    BotFlowStateSnapshot, BotInteractionSession, BotReplyDeliveryReceipt, BotReplyDeliveryRequest,
    DeliveryStatus, InteractionStatus,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

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

    async fn call<T>(
        &self,
        make_job: impl FnOnce(oneshot::Sender<Result<T, BotStateDbError>>) -> DbJob,
    ) -> Result<T, BotStateDbError> {
        let (reply, response) = oneshot::channel();
        self.inner.metrics.queued();
        if self.inner.jobs.send(make_job(reply)).await.is_err() {
            self.inner.metrics.dequeued();
            return Err(BotStateDbError::ActorStopped);
        }
        response.await.map_err(|_| BotStateDbError::ActorStopped)?
    }

    fn call_sync<T>(
        &self,
        make_job: impl FnOnce(std::sync::mpsc::SyncSender<Result<T, BotStateDbError>>) -> DbJob,
    ) -> Result<T, BotStateDbError> {
        let (reply, response) = std::sync::mpsc::sync_channel(1);
        self.inner.metrics.queued();
        match self.inner.jobs.try_send(make_job(reply)) {
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
        response.recv().map_err(|_| BotStateDbError::ActorStopped)?
    }
}

enum DbJob {
    FlowSnapshot {
        reply: SyncDbReply<BotFlowStateSnapshot>,
    },
    FlowPublishedRevision {
        revision: u64,
        reply: SyncDbReply<Option<BotFlowPublishedSnapshot>>,
    },
    SaveFlowDraft {
        request: BotFlowDraftSaveRequest,
        now_ms: i64,
        reply: SyncDbReply<BotFlowDraft>,
    },
    DiscardFlowDraft {
        expected_revision: u64,
        reply: SyncDbReply<()>,
    },
    PublishFlowDraft {
        request: BotFlowPublishRequest,
        now_ms: i64,
        reply: SyncDbReply<BotFlowStateSnapshot>,
    },
    SessionBindingSync {
        binding_key: String,
        reply: SyncDbReply<Option<AgentSessionBinding>>,
    },
    CompareAndSetSessionBindingSync {
        binding_key: String,
        expected_generation: Option<u64>,
        binding: AgentSessionBinding,
        reply: SyncDbReply<bool>,
    },
    BeginAgentEventSync {
        binding_key: String,
        event_id: String,
        turn_id: String,
        reply: SyncDbReply<AgentEventClaim>,
    },
    CompleteAgentEventSync {
        binding_key: String,
        event_id: String,
        reply: SyncDbReply<bool>,
    },
    ReserveDelivery {
        request: BotActiveDeliveryRequest,
        reply: SyncDbReply<DeliveryReservation>,
    },
    DeliveryRequest {
        delivery_id: String,
        reply: SyncDbReply<Option<BotActiveDeliveryRequest>>,
    },
    DeliveryReceipt {
        delivery_id: String,
        reply: SyncDbReply<Option<BotDeliveryReceipt>>,
    },
    DeliveryAttempts {
        delivery_id: String,
        reply: SyncDbReply<Vec<BotDeliveryAttempt>>,
    },
    SaveDeliveryOutcome {
        attempt: BotDeliveryAttempt,
        receipt: BotDeliveryReceipt,
        reply: SyncDbReply<()>,
    },
    SaveDeliveryReceipt {
        receipt: BotDeliveryReceipt,
        reply: SyncDbReply<()>,
    },
    ClaimDueDeliveries {
        now_unix_ms: u64,
        reply: SyncDbReply<Vec<String>>,
    },
    BeginSendDelivery {
        delivery_id: String,
        attempt: BotDeliveryAttempt,
        now_unix_ms: u64,
        lease_ms: u64,
        reply: SyncDbReply<BotDeliveryReceipt>,
    },
    ReserveReplyDelivery {
        request: BotReplyDeliveryRequest,
        reply: SyncDbReply<ReplyDeliveryReservation>,
    },
    ReplyDeliveryReceipt {
        reply_id: String,
        reply: SyncDbReply<Option<BotReplyDeliveryReceipt>>,
    },
    ClaimDueReplyParts {
        now_unix_ms: u64,
        reply: SyncDbReply<Vec<String>>,
    },
    IsReplyPart {
        delivery_id: String,
        reply: SyncDbReply<bool>,
    },
    CreateInteraction {
        session: BotInteractionSession,
        reply: DbReply<bool>,
    },
    ActiveInteractions {
        origin_key: String,
        reply: DbReply<Vec<BotInteractionSession>>,
    },
    CompareAndSetInteraction {
        expected_version: u64,
        session: BotInteractionSession,
        reply: DbReply<bool>,
    },
    RecoverWaitingInteractions {
        reply: DbReply<Vec<BotInteractionSession>>,
    },
}

type DbReply<T> = oneshot::Sender<Result<T, BotStateDbError>>;
type SyncDbReply<T> = std::sync::mpsc::SyncSender<Result<T, BotStateDbError>>;

impl DbJob {
    fn transactional(&self) -> bool {
        matches!(
            self,
            Self::SaveFlowDraft { .. }
                | Self::DiscardFlowDraft { .. }
                | Self::PublishFlowDraft { .. }
                | Self::CompareAndSetSessionBindingSync { .. }
                | Self::BeginAgentEventSync { .. }
                | Self::ReserveDelivery { .. }
                | Self::SaveDeliveryOutcome { .. }
                | Self::ClaimDueDeliveries { .. }
                | Self::BeginSendDelivery { .. }
                | Self::ReserveReplyDelivery { .. }
                | Self::ClaimDueReplyParts { .. }
                | Self::CreateInteraction { .. }
                | Self::CompareAndSetInteraction { .. }
        )
    }

    fn execute(self, connection: &mut Connection, metrics: &ActorMetrics) {
        match self {
            Self::FlowSnapshot { reply } => {
                send_sync_reply(reply, flow_snapshot(connection), metrics);
            }
            Self::FlowPublishedRevision { revision, reply } => {
                send_sync_reply(
                    reply,
                    flow_published_revision(connection, revision),
                    metrics,
                );
            }
            Self::SaveFlowDraft {
                request,
                now_ms,
                reply,
            } => send_sync_reply(reply, save_flow_draft(connection, request, now_ms), metrics),
            Self::DiscardFlowDraft {
                expected_revision,
                reply,
            } => send_sync_reply(
                reply,
                discard_flow_draft(connection, expected_revision),
                metrics,
            ),
            Self::PublishFlowDraft {
                request,
                now_ms,
                reply,
            } => send_sync_reply(
                reply,
                publish_flow_draft(connection, request, now_ms),
                metrics,
            ),
            Self::SessionBindingSync { binding_key, reply } => {
                send_sync_reply(reply, session_binding(connection, &binding_key), metrics);
            }
            Self::CompareAndSetSessionBindingSync {
                binding_key,
                expected_generation,
                binding,
                reply,
            } => send_sync_reply(
                reply,
                compare_and_set_session_binding(
                    connection,
                    &binding_key,
                    expected_generation,
                    &binding,
                ),
                metrics,
            ),
            Self::BeginAgentEventSync {
                binding_key,
                event_id,
                turn_id,
                reply,
            } => send_sync_reply(
                reply,
                begin_agent_event(connection, &binding_key, &event_id, &turn_id),
                metrics,
            ),
            Self::CompleteAgentEventSync {
                binding_key,
                event_id,
                reply,
            } => send_sync_reply(
                reply,
                complete_agent_event(connection, &binding_key, &event_id),
                metrics,
            ),
            Self::ReserveDelivery { request, reply } => {
                send_sync_reply(reply, reserve_delivery(connection, &request), metrics);
            }
            Self::DeliveryRequest { delivery_id, reply } => {
                send_sync_reply(reply, delivery_request(connection, &delivery_id), metrics);
            }
            Self::DeliveryReceipt { delivery_id, reply } => {
                send_sync_reply(reply, delivery_receipt(connection, &delivery_id), metrics);
            }
            Self::DeliveryAttempts { delivery_id, reply } => {
                send_sync_reply(reply, delivery_attempts(connection, &delivery_id), metrics);
            }
            Self::SaveDeliveryOutcome {
                attempt,
                receipt,
                reply,
            } => send_sync_reply(
                reply,
                save_delivery_outcome(connection, &attempt, &receipt),
                metrics,
            ),
            Self::SaveDeliveryReceipt { receipt, reply } => {
                send_sync_reply(reply, save_delivery_receipt(connection, &receipt), metrics);
            }
            Self::ClaimDueDeliveries { now_unix_ms, reply } => send_sync_reply(
                reply,
                claim_due_deliveries(connection, now_unix_ms),
                metrics,
            ),
            Self::BeginSendDelivery {
                delivery_id,
                attempt,
                now_unix_ms,
                lease_ms,
                reply,
            } => send_sync_reply(
                reply,
                begin_send_delivery(connection, &delivery_id, attempt, now_unix_ms, lease_ms),
                metrics,
            ),
            Self::ReserveReplyDelivery { request, reply } => {
                send_sync_reply(reply, reserve_reply_delivery(connection, &request), metrics)
            }
            Self::ReplyDeliveryReceipt { reply_id, reply } => {
                send_sync_reply(
                    reply,
                    reply_delivery_receipt_by_id(connection, &reply_id),
                    metrics,
                );
            }
            Self::ClaimDueReplyParts { now_unix_ms, reply } => send_sync_reply(
                reply,
                claim_due_reply_parts(connection, now_unix_ms),
                metrics,
            ),
            Self::IsReplyPart { delivery_id, reply } => {
                send_sync_reply(reply, is_reply_part(connection, &delivery_id), metrics);
            }
            Self::CreateInteraction { session, reply } => {
                send_reply(reply, create_interaction(connection, &session), metrics);
            }
            Self::ActiveInteractions { origin_key, reply } => {
                send_reply(reply, active_interactions(connection, &origin_key), metrics);
            }
            Self::CompareAndSetInteraction {
                expected_version,
                session,
                reply,
            } => send_reply(
                reply,
                compare_and_set_interaction(connection, expected_version, &session),
                metrics,
            ),
            Self::RecoverWaitingInteractions { reply } => {
                send_reply(reply, recover_waiting_interactions(connection), metrics);
            }
        }
    }
}

fn actor_loop(mut connection: Connection, mut jobs: mpsc::Receiver<DbJob>, metrics: &ActorMetrics) {
    while let Some(job) = jobs.blocking_recv() {
        metrics.dequeued();
        let transactional = job.transactional();
        let started = Instant::now();
        job.execute(&mut connection, metrics);
        if transactional {
            metrics.transaction_finished(started.elapsed());
        }
    }
}

fn send_reply<T>(reply: DbReply<T>, result: Result<T, BotStateDbError>, metrics: &ActorMetrics) {
    if let Err(error) = &result {
        metrics.observe_error(error);
    }
    let _ = reply.send(result);
}

fn send_sync_reply<T>(
    reply: SyncDbReply<T>,
    result: Result<T, BotStateDbError>,
    metrics: &ActorMetrics,
) {
    if let Err(error) = &result {
        metrics.observe_error(error);
    }
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

/// Single-connection factory: busy timeout, foreign keys, prefer WAL, then synchronous=NORMAL.
/// When WAL is unavailable, SQLite keeps another mode and open still succeeds.
fn configure_connection(connection: &Connection) -> Result<String, BotStateDbError> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.pragma_update(None, "foreign_keys", true)?;
    let journal_mode: String =
        connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(journal_mode.to_ascii_lowercase())
}

fn migrate_schema(connection: &Connection) -> Result<(), BotStateDbError> {
    connection.execute_batch(
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
         CREATE TABLE IF NOT EXISTS bot_flow_meta(
             singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
             published_revision INTEGER NOT NULL,
             next_draft_revision INTEGER NOT NULL,
             active_draft_revision INTEGER
         );
         INSERT OR IGNORE INTO bot_flow_meta(
             singleton, published_revision, next_draft_revision, active_draft_revision
         ) VALUES (1, 0, 1, NULL);
         CREATE TABLE IF NOT EXISTS bot_flow_draft(
             singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
             revision INTEGER NOT NULL UNIQUE,
             base_published_revision INTEGER NOT NULL,
             body TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS bot_flow_version(
             revision INTEGER PRIMARY KEY,
             body TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS bot_flow_audit(
             audit_id INTEGER PRIMARY KEY AUTOINCREMENT,
             action TEXT NOT NULL,
             revision INTEGER NOT NULL,
             created_at_ms INTEGER NOT NULL,
             body TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS bot_delivery_attempt_due
             ON bot_delivery_attempt(status, retry_at, delivery_id, attempt);
         CREATE INDEX IF NOT EXISTS bot_interaction_active
             ON bot_interaction(origin_key, status, session_id);
         PRAGMA user_version=5;
         COMMIT;",
    )?;

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
    Ok(())
}

fn immediate(connection: &mut Connection) -> Result<Transaction<'_>, BotStateDbError> {
    Ok(connection.transaction_with_behavior(TransactionBehavior::Immediate)?)
}

fn flow_snapshot(connection: &Connection) -> Result<BotFlowStateSnapshot, BotStateDbError> {
    let (published_revision, active_draft_revision) = connection.query_row(
        "SELECT published_revision, active_draft_revision FROM bot_flow_meta WHERE singleton=1",
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?)),
    )?;
    let published_revision = sqlite_unsigned(published_revision, "published revision")?;
    let active_draft_revision = active_draft_revision
        .map(|revision| sqlite_unsigned(revision, "draft revision"))
        .transpose()?;
    let published = if published_revision == 0 {
        BotFlowPublishedSnapshot {
            revision: 0,
            flows: Vec::new(),
            published_at_ms: 0,
        }
    } else {
        flow_published_revision(connection, published_revision)?.ok_or_else(|| {
            BotStateDbError::Invariant(format!(
                "active Bot flow revision {published_revision} is missing"
            ))
        })?
    };
    let draft = match active_draft_revision {
        Some(revision) => {
            let body = connection
                .query_row(
                    "SELECT body FROM bot_flow_draft WHERE singleton=1 AND revision=?1",
                    params![sqlite_integer(revision)?],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            Some(decode(&body.ok_or_else(|| {
                BotStateDbError::Invariant(format!("active Bot flow draft {revision} is missing"))
            })?)?)
        }
        None => None,
    };
    Ok(BotFlowStateSnapshot { draft, published })
}

fn flow_published_revision(
    connection: &Connection,
    revision: u64,
) -> Result<Option<BotFlowPublishedSnapshot>, BotStateDbError> {
    if revision == 0 {
        return Ok(Some(BotFlowPublishedSnapshot {
            revision: 0,
            flows: Vec::new(),
            published_at_ms: 0,
        }));
    }
    connection
        .query_row(
            "SELECT body FROM bot_flow_version WHERE revision=?1",
            params![sqlite_integer(revision)?],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|body| decode(&body))
        .transpose()
}

fn save_flow_draft(
    connection: &mut Connection,
    request: BotFlowDraftSaveRequest,
    now_ms: i64,
) -> Result<BotFlowDraft, BotStateDbError> {
    let transaction = immediate(connection)?;
    let (published_revision, next_draft_revision, active_draft_revision) = transaction.query_row(
        "SELECT published_revision, next_draft_revision, active_draft_revision
         FROM bot_flow_meta WHERE singleton=1",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        },
    )?;
    let published_revision = sqlite_unsigned(published_revision, "published revision")?;
    let next_draft_revision = sqlite_unsigned(next_draft_revision, "next draft revision")?;
    let active_draft_revision = active_draft_revision
        .map(|revision| sqlite_unsigned(revision, "draft revision"))
        .transpose()?;
    let expected = request.expected_draft_revision.unwrap_or(0);
    let actual = active_draft_revision.unwrap_or(0);
    if expected != actual {
        return Err(BotStateDbError::FlowRevisionConflict { expected, actual });
    }
    if request.base_published_revision != published_revision {
        return Err(BotStateDbError::FlowRevisionConflict {
            expected: request.base_published_revision,
            actual: published_revision,
        });
    }
    let draft = BotFlowDraft {
        revision: next_draft_revision,
        base_published_revision: published_revision,
        flows: request.flows,
        updated_at_ms: now_ms,
    };
    let body = encode(&draft)?;
    transaction.execute(
        "INSERT INTO bot_flow_draft(singleton, revision, base_published_revision, body)
         VALUES (1, ?1, ?2, ?3)
         ON CONFLICT(singleton) DO UPDATE SET
             revision=excluded.revision,
             base_published_revision=excluded.base_published_revision,
             body=excluded.body",
        params![
            sqlite_integer(draft.revision)?,
            sqlite_integer(draft.base_published_revision)?,
            body
        ],
    )?;
    transaction.execute(
        "UPDATE bot_flow_meta SET next_draft_revision=?1, active_draft_revision=?2
         WHERE singleton=1",
        params![
            sqlite_integer(next_draft_revision + 1)?,
            sqlite_integer(draft.revision)?
        ],
    )?;
    transaction.execute(
        "INSERT INTO bot_flow_audit(action, revision, created_at_ms, body)
         VALUES ('draft.save', ?1, ?2, ?3)",
        params![sqlite_integer(draft.revision)?, now_ms, encode(&draft)?],
    )?;
    transaction.commit()?;
    Ok(draft)
}

fn discard_flow_draft(
    connection: &mut Connection,
    expected_revision: u64,
) -> Result<(), BotStateDbError> {
    let transaction = immediate(connection)?;
    let actual = transaction
        .query_row(
            "SELECT active_draft_revision FROM bot_flow_meta WHERE singleton=1",
            [],
            |row| row.get::<_, Option<i64>>(0),
        )?
        .map(|revision| sqlite_unsigned(revision, "draft revision"))
        .transpose()?
        .unwrap_or(0);
    if expected_revision != actual {
        return Err(BotStateDbError::FlowRevisionConflict {
            expected: expected_revision,
            actual,
        });
    }
    transaction.execute("DELETE FROM bot_flow_draft WHERE singleton=1", [])?;
    transaction.execute(
        "UPDATE bot_flow_meta SET active_draft_revision=NULL WHERE singleton=1",
        [],
    )?;
    transaction.execute(
        "INSERT INTO bot_flow_audit(action, revision, created_at_ms, body)
         VALUES ('draft.discard', ?1, 0, '{}')",
        params![sqlite_integer(actual)?],
    )?;
    transaction.commit()?;
    Ok(())
}

fn publish_flow_draft(
    connection: &mut Connection,
    request: BotFlowPublishRequest,
    now_ms: i64,
) -> Result<BotFlowStateSnapshot, BotStateDbError> {
    let transaction = immediate(connection)?;
    let (published_revision, active_draft_revision) = transaction.query_row(
        "SELECT published_revision, active_draft_revision FROM bot_flow_meta WHERE singleton=1",
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?)),
    )?;
    let published_revision = sqlite_unsigned(published_revision, "published revision")?;
    let active_draft_revision = active_draft_revision
        .map(|revision| sqlite_unsigned(revision, "draft revision"))
        .transpose()?;
    if published_revision != request.expected_published_revision {
        return Err(BotStateDbError::FlowRevisionConflict {
            expected: request.expected_published_revision,
            actual: published_revision,
        });
    }
    let actual_draft = active_draft_revision.unwrap_or(0);
    if actual_draft != request.expected_draft_revision {
        return Err(BotStateDbError::FlowRevisionConflict {
            expected: request.expected_draft_revision,
            actual: actual_draft,
        });
    }
    let draft_body = transaction
        .query_row(
            "SELECT body FROM bot_flow_draft WHERE singleton=1 AND revision=?1",
            params![sqlite_integer(actual_draft)?],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| BotStateDbError::Invariant("active Bot flow draft is missing".into()))?;
    let draft: BotFlowDraft = decode(&draft_body)?;
    if draft.base_published_revision != published_revision {
        return Err(BotStateDbError::FlowRevisionConflict {
            expected: draft.base_published_revision,
            actual: published_revision,
        });
    }
    let published = BotFlowPublishedSnapshot {
        revision: published_revision + 1,
        flows: draft.flows,
        published_at_ms: now_ms,
    };
    let body = encode(&published)?;
    transaction.execute(
        "INSERT INTO bot_flow_version(revision, body) VALUES (?1, ?2)",
        params![sqlite_integer(published.revision)?, body],
    )?;
    transaction.execute("DELETE FROM bot_flow_draft WHERE singleton=1", [])?;
    transaction.execute(
        "UPDATE bot_flow_meta SET published_revision=?1, active_draft_revision=NULL
         WHERE singleton=1",
        params![sqlite_integer(published.revision)?],
    )?;
    transaction.execute(
        "INSERT INTO bot_flow_audit(action, revision, created_at_ms, body)
         VALUES ('publish', ?1, ?2, ?3)",
        params![
            sqlite_integer(published.revision)?,
            now_ms,
            encode(&published)?
        ],
    )?;
    transaction.commit()?;
    Ok(BotFlowStateSnapshot {
        draft: None,
        published,
    })
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
        if existing != *request {
            return Ok(ReplyDeliveryReservation::Conflict);
        }
        complete_reply_source_event(&transaction, request)?;
        let receipt = reply_delivery_receipt(&transaction, &existing)?;
        transaction.commit()?;
        return Ok(ReplyDeliveryReservation::Existing(receipt));
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
    let mut statement = connection.prepare(
        "SELECT body FROM bot_delivery_attempt
         WHERE delivery_id=?1 ORDER BY attempt, status",
    )?;
    statement
        .query_map(params![delivery_id], |row| row.get::<_, String>(0))?
        .map(|body| decode(&body?))
        .collect()
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

fn claim_due_deliveries_by_kind(
    connection: &mut Connection,
    now_unix_ms: u64,
    reply_parts: bool,
) -> Result<Vec<String>, BotStateDbError> {
    let transaction = immediate(connection)?;
    let now = sqlite_integer(now_unix_ms)?;
    let candidates = {
        let query = if reply_parts {
            "SELECT r.delivery_id, r.status, r.body
             FROM bot_delivery_receipt r
             JOIN bot_reply_delivery_part p ON p.delivery_id=r.delivery_id
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
             ORDER BY p.reply_id, p.part_index"
        } else {
            "SELECT r.delivery_id, r.status, r.body
             FROM bot_delivery_receipt r
             WHERE r.status IN ('pending', 'retry_scheduled', 'sending')
               AND NOT EXISTS (
                   SELECT 1 FROM bot_reply_delivery_part p WHERE p.delivery_id=r.delivery_id
               )
             ORDER BY r.delivery_id"
        };
        let mut statement = transaction.prepare(query)?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut claimed = Vec::new();
    for (delivery_id, status, body) in candidates {
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
                let due = {
                    let mut statement = transaction.prepare(
                        "SELECT 1 FROM bot_delivery_attempt
                         WHERE delivery_id=?1 AND status='retry_scheduled' AND retry_at<=?2
                           AND attempt=(
                               SELECT MAX(b.attempt) FROM bot_delivery_attempt b
                               WHERE b.delivery_id=?1
                           )
                         LIMIT 1",
                    )?;
                    statement
                        .query_row(params![delivery_id, now], |_| Ok(()))
                        .optional()?
                        .is_some()
                };
                if !due {
                    continue;
                }
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
    let mut statement = connection.prepare(
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
    let mut statement = connection
        .prepare("SELECT body FROM bot_interaction WHERE status='waiting' ORDER BY session_id")?;
    statement
        .query_map([], |row| row.get::<_, String>(0))?
        .map(|body| decode(&body?))
        .collect()
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

#[async_trait]
impl ConversationRepository for BotStateDbRepository {
    async fn session_binding(
        &self,
        binding_key: &str,
    ) -> Result<Option<AgentSessionBinding>, ConversationError> {
        let binding_key = binding_key.to_owned();
        self.call_sync(|reply| DbJob::SessionBindingSync { binding_key, reply })
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
            .call_sync(|reply| DbJob::CompareAndSetSessionBindingSync {
                binding_key,
                expected_generation,
                binding,
                reply,
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
        self.call_sync(|reply| DbJob::BeginAgentEventSync {
            binding_key,
            event_id,
            turn_id,
            reply,
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
            .call_sync(|reply| DbJob::CompleteAgentEventSync {
                binding_key,
                event_id,
                reply,
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
            .call_sync(|reply| DbJob::ReserveDelivery { request, reply })
            .map_err(delivery_error)?
        {
            DeliveryReservation::Reserved => Ok(None),
            DeliveryReservation::Existing(receipt) => Ok(Some(receipt)),
            DeliveryReservation::Conflict => Err(DeliveryError::Conflict),
        }
    }

    async fn request(&self, delivery_id: &str) -> Result<BotActiveDeliveryRequest, DeliveryError> {
        let delivery_id = delivery_id.to_owned();
        self.call_sync(|reply| DbJob::DeliveryRequest { delivery_id, reply })
            .map_err(delivery_error)?
            .ok_or(DeliveryError::NotFound)
    }

    async fn receipt(&self, delivery_id: &str) -> Result<BotDeliveryReceipt, DeliveryError> {
        let delivery_id = delivery_id.to_owned();
        self.call_sync(|reply| DbJob::DeliveryReceipt { delivery_id, reply })
            .map_err(delivery_error)?
            .ok_or(DeliveryError::NotFound)
    }

    async fn attempts(&self, delivery_id: &str) -> Result<Vec<BotDeliveryAttempt>, DeliveryError> {
        let delivery_id = delivery_id.to_owned();
        self.call_sync(|reply| DbJob::DeliveryAttempts { delivery_id, reply })
            .map_err(delivery_error)
    }

    async fn save_outcome(
        &self,
        attempt: BotDeliveryAttempt,
        receipt: BotDeliveryReceipt,
    ) -> Result<(), DeliveryError> {
        self.call_sync(|reply| DbJob::SaveDeliveryOutcome {
            attempt,
            receipt,
            reply,
        })
        .map_err(delivery_error)
    }

    async fn save_receipt(&self, receipt: BotDeliveryReceipt) -> Result<(), DeliveryError> {
        self.call_sync(|reply| DbJob::SaveDeliveryReceipt { receipt, reply })
            .map_err(delivery_error)
    }

    async fn claim_due_delivery_ids(&self, now_unix_ms: u64) -> Result<Vec<String>, DeliveryError> {
        self.call_sync(|reply| DbJob::ClaimDueDeliveries { now_unix_ms, reply })
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
        self.call_sync(|reply| DbJob::BeginSendDelivery {
            delivery_id,
            attempt,
            now_unix_ms,
            lease_ms,
            reply,
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
            .call_sync(|reply| DbJob::ReserveReplyDelivery { request, reply })
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
        self.call_sync(|reply| DbJob::ReplyDeliveryReceipt { reply_id, reply })
            .map_err(delivery_error)?
            .ok_or(DeliveryError::NotFound)
    }

    async fn claim_due_reply_part_id(
        &self,
        now_unix_ms: u64,
    ) -> Result<Option<String>, DeliveryError> {
        self.call_sync(|reply| DbJob::ClaimDueReplyParts { now_unix_ms, reply })
            .map(|ids| ids.into_iter().next())
            .map_err(delivery_error)
    }

    async fn is_reply_part(&self, delivery_id: &str) -> Result<bool, DeliveryError> {
        let delivery_id = delivery_id.to_owned();
        self.call_sync(|reply| DbJob::IsReplyPart { delivery_id, reply })
            .map_err(delivery_error)
    }
}

#[async_trait]
impl InteractionRepository for BotStateDbRepository {
    async fn create(&self, session: BotInteractionSession) -> Result<(), InteractionError> {
        let changed = self
            .call(|reply| DbJob::CreateInteraction { session, reply })
            .await
            .map_err(interaction_error)?;
        changed
            .then_some(())
            .ok_or(InteractionError::WaiterConflict)
    }

    async fn active_for_origin(
        &self,
        origin_key: &str,
    ) -> Result<Vec<BotInteractionSession>, InteractionError> {
        let origin_key = origin_key.to_owned();
        self.call(|reply| DbJob::ActiveInteractions { origin_key, reply })
            .await
            .map_err(interaction_error)
    }

    async fn compare_and_set(
        &self,
        expected_version: u64,
        session: BotInteractionSession,
    ) -> Result<(), InteractionError> {
        let changed = self
            .call(|reply| DbJob::CompareAndSetInteraction {
                expected_version,
                session,
                reply,
            })
            .await
            .map_err(interaction_error)?;
        changed
            .then_some(())
            .ok_or(InteractionError::GenerationConflict)
    }

    async fn recover_waiting(&self) -> Result<Vec<BotInteractionSession>, InteractionError> {
        self.call(|reply| DbJob::RecoverWaitingInteractions { reply })
            .await
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

fn state_db_error_message(error: BotStateDbError) -> String {
    let message = error.to_string();
    drop(error);
    message
}

impl BotFlowRepository for BotStateDbRepository {
    fn snapshot(&self) -> Result<BotFlowStateSnapshot, BotFlowError> {
        self.call_sync(|reply| DbJob::FlowSnapshot { reply })
            .map_err(flow_error)
    }

    fn published_revision(
        &self,
        revision: u64,
    ) -> Result<Option<BotFlowPublishedSnapshot>, BotFlowError> {
        self.call_sync(|reply| DbJob::FlowPublishedRevision { revision, reply })
            .map_err(flow_error)
    }

    fn save_draft(
        &self,
        request: BotFlowDraftSaveRequest,
        now_ms: i64,
    ) -> Result<BotFlowDraft, BotFlowError> {
        self.call_sync(|reply| DbJob::SaveFlowDraft {
            request,
            now_ms,
            reply,
        })
        .map_err(flow_error)
    }

    fn discard_draft(&self, expected_revision: u64) -> Result<(), BotFlowError> {
        self.call_sync(|reply| DbJob::DiscardFlowDraft {
            expected_revision,
            reply,
        })
        .map_err(flow_error)
    }

    fn publish(
        &self,
        request: BotFlowPublishRequest,
        now_ms: i64,
    ) -> Result<BotFlowStateSnapshot, BotFlowError> {
        self.call_sync(|reply| DbJob::PublishFlowDraft {
            request,
            now_ms,
            reply,
        })
        .map_err(flow_error)
    }
}

fn flow_error(error: BotStateDbError) -> BotFlowError {
    match error {
        BotStateDbError::FlowRevisionConflict { expected, actual } => {
            BotFlowError::RevisionConflict { expected, actual }
        }
        other => BotFlowError::Repository(state_db_error_message(other)),
    }
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
    #[error("expected Bot flow revision {expected}, current revision is {actual}")]
    FlowRevisionConflict { expected: u64, actual: u64 },
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::AtomicUsize;

    use mutsuki_bot_protocol::{
        BotConversationKind, BotDeliveryContent, BotInteractionSession, BotReplyDeliveryPart,
        DeliveryPolicy, InteractionScope, InteractionWaitSpec, MessageSegment,
        QQ_CONVERSATION_REF_VERSION, QqConversationRef,
    };
    use tokio::task::JoinSet;

    use super::*;

    #[test]
    fn flow_draft_publish_uses_cas_and_recovers_immutable_revision() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("flow.db");
        let repository = BotStateDbRepository::open(&path).unwrap();
        let draft = BotFlowRepository::save_draft(
            &repository,
            BotFlowDraftSaveRequest {
                expected_draft_revision: None,
                base_published_revision: 0,
                flows: Vec::new(),
            },
            10,
        )
        .unwrap();
        assert_eq!(draft.revision, 1);
        assert!(matches!(
            BotFlowRepository::save_draft(
                &repository,
                BotFlowDraftSaveRequest {
                    expected_draft_revision: None,
                    base_published_revision: 0,
                    flows: Vec::new(),
                },
                11,
            ),
            Err(BotFlowError::RevisionConflict {
                expected: 0,
                actual: 1
            })
        ));
        let snapshot = BotFlowRepository::publish(
            &repository,
            BotFlowPublishRequest {
                expected_draft_revision: 1,
                expected_published_revision: 0,
            },
            20,
        )
        .unwrap();
        assert_eq!(snapshot.published.revision, 1);
        drop(repository);

        let reopened = BotStateDbRepository::open(path).unwrap();
        let snapshot = BotFlowRepository::snapshot(&reopened).unwrap();
        assert_eq!(snapshot.published.revision, 1);
        assert!(snapshot.draft.is_none());
        assert_eq!(
            BotFlowRepository::published_revision(&reopened, 1)
                .unwrap()
                .unwrap(),
            snapshot.published
        );
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
        repository.create(interaction(&conversation)).await.unwrap();
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
        assert_eq!(reopened.recover_waiting().await.unwrap().len(), 1);
        assert_eq!(reopened.metrics().connection_open_count, 1);
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
        repository.create(waiting.clone()).await.unwrap();
        let mut interaction_cas = JoinSet::new();
        for index in 0..64 {
            let repository = repository.clone();
            let mut next = waiting.clone();
            next.version = 2;
            next.retries_remaining = index + 1;
            interaction_cas.spawn(async move { repository.compare_and_set(1, next).await });
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
        repository.create(waiting.clone()).await.unwrap();

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
                        let _ = repository.compare_and_set(1, next).await;
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
