use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use mutsuki_bot_conversation::{AgentEventClaim, ConversationError, ConversationRepository};
use mutsuki_bot_delivery::{DELIVERY_SEND_LEASE_MS, DeliveryError, DeliveryRepository};
use mutsuki_bot_interaction::{InteractionError, InteractionRepository};
use mutsuki_bot_protocol::{
    AgentSessionBinding, BotActiveDeliveryRequest, BotDeliveryAttempt, BotDeliveryReceipt,
    BotInteractionSession, ConversationPolicyRule, DeliveryStatus, InteractionStatus,
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
                    Ok(connection) => {
                        let _ = started_tx.send(Ok(()));
                        actor_loop(connection, receiver, &actor_metrics);
                    }
                    Err(error) => {
                        let _ = started_tx.send(Err(error.to_string()));
                    }
                }
            })
            .map_err(|error| BotStateDbError::ActorStart(error.to_string()))?;
        started_rx
            .recv()
            .map_err(|_| BotStateDbError::ActorStopped)?
            .map_err(BotStateDbError::ActorStart)?;
        Ok(Self {
            inner: Arc::new(RepositoryInner {
                path,
                jobs,
                metrics,
            }),
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    #[must_use]
    pub fn metrics(&self) -> BotStateDbMetrics {
        self.inner.metrics.snapshot()
    }

    /// Atomically inserts or replaces a typed conversation policy rule.
    ///
    /// # Errors
    ///
    /// Returns an error when the actor is unavailable or `SQLite` rejects the write.
    pub async fn upsert_policy_rule(
        &self,
        rule: ConversationPolicyRule,
    ) -> Result<(), BotStateDbError> {
        self.call(|reply| DbJob::UpsertPolicyRule { rule, reply })
            .await
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
}

enum DbJob {
    UpsertPolicyRule {
        rule: ConversationPolicyRule,
        reply: DbReply<()>,
    },
    PolicyRules {
        reply: DbReply<Vec<ConversationPolicyRule>>,
    },
    SessionBinding {
        binding_key: String,
        reply: DbReply<Option<AgentSessionBinding>>,
    },
    CompareAndSetSessionBinding {
        binding_key: String,
        expected_generation: Option<u64>,
        binding: AgentSessionBinding,
        reply: DbReply<bool>,
    },
    BeginAgentEvent {
        binding_key: String,
        event_id: String,
        turn_id: String,
        reply: DbReply<AgentEventClaim>,
    },
    CompleteAgentEvent {
        binding_key: String,
        event_id: String,
        reply: DbReply<bool>,
    },
    ReserveDelivery {
        request: BotActiveDeliveryRequest,
        reply: DbReply<DeliveryReservation>,
    },
    DeliveryRequest {
        delivery_id: String,
        reply: DbReply<Option<BotActiveDeliveryRequest>>,
    },
    DeliveryReceipt {
        delivery_id: String,
        reply: DbReply<Option<BotDeliveryReceipt>>,
    },
    DeliveryAttempts {
        delivery_id: String,
        reply: DbReply<Vec<BotDeliveryAttempt>>,
    },
    SaveDeliveryOutcome {
        attempt: BotDeliveryAttempt,
        receipt: BotDeliveryReceipt,
        reply: DbReply<()>,
    },
    SaveDeliveryReceipt {
        receipt: BotDeliveryReceipt,
        reply: DbReply<()>,
    },
    ClaimDueDeliveries {
        now_unix_ms: u64,
        reply: DbReply<Vec<String>>,
    },
    BeginSendDelivery {
        delivery_id: String,
        attempt: BotDeliveryAttempt,
        now_unix_ms: u64,
        lease_ms: u64,
        reply: DbReply<BotDeliveryReceipt>,
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

impl DbJob {
    fn transactional(&self) -> bool {
        matches!(
            self,
            Self::CompareAndSetSessionBinding { .. }
                | Self::BeginAgentEvent { .. }
                | Self::ReserveDelivery { .. }
                | Self::SaveDeliveryOutcome { .. }
                | Self::ClaimDueDeliveries { .. }
                | Self::BeginSendDelivery { .. }
                | Self::CreateInteraction { .. }
                | Self::CompareAndSetInteraction { .. }
        )
    }

    fn execute(self, connection: &mut Connection, metrics: &ActorMetrics) {
        match self {
            Self::UpsertPolicyRule { rule, reply } => {
                send_reply(reply, upsert_policy_rule(connection, &rule), metrics);
            }
            Self::PolicyRules { reply } => {
                send_reply(reply, policy_rules(connection), metrics);
            }
            Self::SessionBinding { binding_key, reply } => {
                send_reply(reply, session_binding(connection, &binding_key), metrics);
            }
            Self::CompareAndSetSessionBinding {
                binding_key,
                expected_generation,
                binding,
                reply,
            } => send_reply(
                reply,
                compare_and_set_session_binding(
                    connection,
                    &binding_key,
                    expected_generation,
                    &binding,
                ),
                metrics,
            ),
            Self::BeginAgentEvent {
                binding_key,
                event_id,
                turn_id,
                reply,
            } => send_reply(
                reply,
                begin_agent_event(connection, &binding_key, &event_id, &turn_id),
                metrics,
            ),
            Self::CompleteAgentEvent {
                binding_key,
                event_id,
                reply,
            } => send_reply(
                reply,
                complete_agent_event(connection, &binding_key, &event_id),
                metrics,
            ),
            Self::ReserveDelivery { request, reply } => {
                send_reply(reply, reserve_delivery(connection, &request), metrics);
            }
            Self::DeliveryRequest { delivery_id, reply } => {
                send_reply(reply, delivery_request(connection, &delivery_id), metrics);
            }
            Self::DeliveryReceipt { delivery_id, reply } => {
                send_reply(reply, delivery_receipt(connection, &delivery_id), metrics);
            }
            Self::DeliveryAttempts { delivery_id, reply } => {
                send_reply(reply, delivery_attempts(connection, &delivery_id), metrics);
            }
            Self::SaveDeliveryOutcome {
                attempt,
                receipt,
                reply,
            } => send_reply(
                reply,
                save_delivery_outcome(connection, &attempt, &receipt),
                metrics,
            ),
            Self::SaveDeliveryReceipt { receipt, reply } => {
                send_reply(reply, save_delivery_receipt(connection, &receipt), metrics);
            }
            Self::ClaimDueDeliveries { now_unix_ms, reply } => send_reply(
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
            } => send_reply(
                reply,
                begin_send_delivery(connection, &delivery_id, attempt, now_unix_ms, lease_ms),
                metrics,
            ),
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

fn open_connection(path: &Path, metrics: &ActorMetrics) -> Result<Connection, BotStateDbError> {
    let connection = Connection::open(path)?;
    metrics
        .connection_open_count
        .fetch_add(1, Ordering::Relaxed);
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    migrate_schema(&connection)?;
    Ok(connection)
}

fn migrate_schema(connection: &Connection) -> Result<(), BotStateDbError> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE IF NOT EXISTS bot_conversation_policy(
             rule_id TEXT PRIMARY KEY,
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
         CREATE TABLE IF NOT EXISTS bot_interaction(
             session_id TEXT PRIMARY KEY,
             origin_key TEXT NOT NULL,
             status TEXT NOT NULL,
             version INTEGER NOT NULL,
             body TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS bot_delivery_attempt_due
             ON bot_delivery_attempt(status, retry_at, delivery_id, attempt);
         CREATE INDEX IF NOT EXISTS bot_interaction_active
             ON bot_interaction(origin_key, status, session_id);
         PRAGMA user_version=2;
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

fn upsert_policy_rule(
    connection: &Connection,
    rule: &ConversationPolicyRule,
) -> Result<(), BotStateDbError> {
    connection.execute(
        "INSERT INTO bot_conversation_policy(rule_id, body) VALUES (?1, ?2)
         ON CONFLICT(rule_id) DO UPDATE SET body=excluded.body",
        params![rule.rule_id, encode(rule)?],
    )?;
    Ok(())
}

fn policy_rules(connection: &Connection) -> Result<Vec<ConversationPolicyRule>, BotStateDbError> {
    let mut statement =
        connection.prepare("SELECT body FROM bot_conversation_policy ORDER BY rule_id")?;
    statement
        .query_map([], |row| row.get::<_, String>(0))?
        .map(|body| decode(&body?))
        .collect()
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
        transaction.commit()?;
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
        transaction.commit()?;
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
        transaction.commit()?;
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
    transaction.commit()?;
    Ok(if delivery_exists {
        DeliveryReservation::Conflict
    } else {
        return Err(BotStateDbError::Invariant(
            "delivery reservation disappeared after conflict".into(),
        ));
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

fn claim_send_lease_on_receipt(
    receipt: &mut BotDeliveryReceipt,
    now_unix_ms: u64,
    lease_ms: u64,
) {
    receipt.status = DeliveryStatus::Sending;
    receipt.generation = receipt.generation.saturating_add(1);
    receipt.lease_expires_at_unix_ms = Some(now_unix_ms.saturating_add(lease_ms));
    receipt.error_code = None;
}

fn claim_due_deliveries(
    connection: &mut Connection,
    now_unix_ms: u64,
) -> Result<Vec<String>, BotStateDbError> {
    let transaction = immediate(connection)?;
    let now = sqlite_integer(now_unix_ms)?;
    let candidates = {
        let mut statement = transaction.prepare(
            "SELECT delivery_id, status, body FROM bot_delivery_receipt
             WHERE status IN ('pending', 'retry_scheduled', 'sending')
             ORDER BY delivery_id",
        )?;
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
                }
            }
            _ => {}
        }
    }
    transaction.commit()?;
    Ok(claimed)
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
        .ok_or_else(|| BotStateDbError::Invariant(format!("delivery receipt missing: {delivery_id}")))?;
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
    async fn policy_rules(&self) -> Result<Vec<ConversationPolicyRule>, ConversationError> {
        self.call(|reply| DbJob::PolicyRules { reply })
            .await
            .map_err(conversation_error)
    }

    async fn session_binding(
        &self,
        binding_key: &str,
    ) -> Result<Option<AgentSessionBinding>, ConversationError> {
        let binding_key = binding_key.to_owned();
        self.call(|reply| DbJob::SessionBinding { binding_key, reply })
            .await
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
            .call(|reply| DbJob::CompareAndSetSessionBinding {
                binding_key,
                expected_generation,
                binding,
                reply,
            })
            .await
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
        self.call(|reply| DbJob::BeginAgentEvent {
            binding_key,
            event_id,
            turn_id,
            reply,
        })
        .await
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
            .call(|reply| DbJob::CompleteAgentEvent {
                binding_key,
                event_id,
                reply,
            })
            .await
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
            .call(|reply| DbJob::ReserveDelivery { request, reply })
            .await
            .map_err(delivery_error)?
        {
            DeliveryReservation::Reserved => Ok(None),
            DeliveryReservation::Existing(receipt) => Ok(Some(receipt)),
            DeliveryReservation::Conflict => Err(DeliveryError::Conflict),
        }
    }

    async fn request(&self, delivery_id: &str) -> Result<BotActiveDeliveryRequest, DeliveryError> {
        let delivery_id = delivery_id.to_owned();
        self.call(|reply| DbJob::DeliveryRequest { delivery_id, reply })
            .await
            .map_err(delivery_error)?
            .ok_or(DeliveryError::NotFound)
    }

    async fn receipt(&self, delivery_id: &str) -> Result<BotDeliveryReceipt, DeliveryError> {
        let delivery_id = delivery_id.to_owned();
        self.call(|reply| DbJob::DeliveryReceipt { delivery_id, reply })
            .await
            .map_err(delivery_error)?
            .ok_or(DeliveryError::NotFound)
    }

    async fn attempts(&self, delivery_id: &str) -> Result<Vec<BotDeliveryAttempt>, DeliveryError> {
        let delivery_id = delivery_id.to_owned();
        self.call(|reply| DbJob::DeliveryAttempts { delivery_id, reply })
            .await
            .map_err(delivery_error)
    }

    async fn save_outcome(
        &self,
        attempt: BotDeliveryAttempt,
        receipt: BotDeliveryReceipt,
    ) -> Result<(), DeliveryError> {
        self.call(|reply| DbJob::SaveDeliveryOutcome {
            attempt,
            receipt,
            reply,
        })
        .await
        .map_err(delivery_error)
    }

    async fn save_receipt(&self, receipt: BotDeliveryReceipt) -> Result<(), DeliveryError> {
        self.call(|reply| DbJob::SaveDeliveryReceipt { receipt, reply })
            .await
            .map_err(delivery_error)
    }

    async fn claim_due_delivery_ids(&self, now_unix_ms: u64) -> Result<Vec<String>, DeliveryError> {
        self.call(|reply| DbJob::ClaimDueDeliveries { now_unix_ms, reply })
            .await
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
        self.call(|reply| DbJob::BeginSendDelivery {
            delivery_id,
            attempt,
            now_unix_ms,
            lease_ms,
            reply,
        })
        .await
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
    #[error("invalid database actor configuration: {0}")]
    InvalidConfiguration(String),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mutsuki_bot_protocol::{
        AgentSessionScope, BotConversationKind, BotDeliveryContent, BotInteractionSession,
        BotPropagationPolicy, ConversationPolicyMatch, ConversationPolicyPatch, DeliveryPolicy,
        InteractionScope, InteractionWaitSpec, MessageSegment, QQ_CONVERSATION_REF_VERSION,
        QqConversationRef,
    };
    use tokio::task::JoinSet;

    use super::*;

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
    async fn state_recovers_after_reopen_and_uses_one_actor_connection() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("bot-state.db");
        let repository = BotStateDbRepository::open(&path).unwrap();
        let conversation = conversation();
        repository.upsert_policy_rule(policy_rule()).await.unwrap();
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
        drop(repository);

        let reopened = BotStateDbRepository::open(&path).unwrap();
        assert_eq!(reopened.policy_rules().await.unwrap().len(), 1);
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

    fn policy_rule() -> ConversationPolicyRule {
        ConversationPolicyRule {
            rule_id: "group-rule".into(),
            revision: 2,
            matcher: ConversationPolicyMatch {
                group_id: Some("group".into()),
                ..ConversationPolicyMatch::default()
            },
            patch: ConversationPolicyPatch {
                agent_enabled: Some(true),
                session_scope: Some(AgentSessionScope::SharedConversation),
                ..ConversationPolicyPatch::default()
            },
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
                propagation: BotPropagationPolicy::ConsumeOnSuccess,
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
