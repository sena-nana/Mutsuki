use std::path::{Path, PathBuf};

use mutsuki_bot_conversation::{AgentEventClaim, ConversationError, ConversationRepository};
use mutsuki_bot_delivery::{DeliveryError, DeliveryRepository};
use mutsuki_bot_interaction::{InteractionError, InteractionRepository};
use mutsuki_bot_protocol::{
    AgentSessionBinding, BotActiveDeliveryRequest, BotDeliveryAttempt, BotDeliveryReceipt,
    BotInteractionSession, ConversationPolicyRule, DeliveryStatus, InteractionStatus,
};
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, params_from_iter};
use serde_json::{Map, Value, json};
use thiserror::Error;

#[derive(Clone)]
pub struct BotStateDbRepository {
    path: PathBuf,
}

impl BotStateDbRepository {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, BotStateDbError> {
        let repository = Self { path: path.into() };
        repository.initialize()?;
        Ok(repository)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn upsert_policy_rule(&self, rule: &ConversationPolicyRule) -> Result<(), BotStateDbError> {
        self.execute(
            "INSERT INTO bot_conversation_policy(rule_id, body) VALUES (?1, ?2) \
             ON CONFLICT(rule_id) DO UPDATE SET body=excluded.body",
            vec![json!(rule.rule_id), encode(rule)?],
        )?;
        Ok(())
    }

    fn initialize(&self) -> Result<(), BotStateDbError> {
        self.connection()?.execute_batch(
            "CREATE TABLE IF NOT EXISTS bot_conversation_policy(rule_id TEXT PRIMARY KEY, body TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS bot_agent_binding(binding_key TEXT PRIMARY KEY, generation INTEGER NOT NULL, body TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS bot_agent_event(binding_key TEXT NOT NULL, event_id TEXT NOT NULL, turn_id TEXT NOT NULL, status TEXT NOT NULL, PRIMARY KEY(binding_key, event_id));
             CREATE TABLE IF NOT EXISTS bot_delivery_request(delivery_id TEXT PRIMARY KEY, idempotency_key TEXT NOT NULL UNIQUE, body TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS bot_delivery_attempt(delivery_id TEXT NOT NULL, attempt INTEGER NOT NULL, status TEXT NOT NULL, retry_at INTEGER, body TEXT NOT NULL, PRIMARY KEY(delivery_id, attempt, status));
             CREATE TABLE IF NOT EXISTS bot_delivery_receipt(delivery_id TEXT PRIMARY KEY, idempotency_key TEXT NOT NULL UNIQUE, body TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS bot_interaction(session_id TEXT PRIMARY KEY, origin_key TEXT NOT NULL, status TEXT NOT NULL, version INTEGER NOT NULL, body TEXT NOT NULL);",
        )?;
        Ok(())
    }

    fn connection(&self) -> Result<Connection, BotStateDbError> {
        Ok(Connection::open(&self.path)?)
    }

    fn query(
        &self,
        sql: &str,
        params: impl IntoIterator<Item = Value>,
    ) -> Result<Vec<Value>, BotStateDbError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(sql)?;
        let columns = statement
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let params = params
            .into_iter()
            .map(|value| json_to_sql(&value))
            .collect::<Vec<_>>();
        let mut rows = statement.query(params_from_iter(params))?;
        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            let mut value = Map::new();
            for (index, name) in columns.iter().enumerate() {
                value.insert(name.clone(), sql_to_json(row.get_ref(index)?));
            }
            result.push(Value::Object(value));
        }
        Ok(result)
    }

    fn execute(
        &self,
        sql: &str,
        params: impl IntoIterator<Item = Value>,
    ) -> Result<u64, BotStateDbError> {
        let connection = self.connection()?;
        let params = params
            .into_iter()
            .map(|value| json_to_sql(&value))
            .collect::<Vec<_>>();
        Ok(connection.execute(sql, params_from_iter(params))? as u64)
    }
}

impl ConversationRepository for BotStateDbRepository {
    fn policy_rules(&self) -> Result<Vec<ConversationPolicyRule>, ConversationError> {
        decode_rows(self.query(
            "SELECT body FROM bot_conversation_policy ORDER BY rule_id",
            Vec::new(),
        ))
        .map_err(conversation_error)
    }

    fn session_binding(
        &self,
        binding_key: &str,
    ) -> Result<Option<AgentSessionBinding>, ConversationError> {
        decode_optional(self.query(
            "SELECT body FROM bot_agent_binding WHERE binding_key=?1",
            vec![json!(binding_key)],
        ))
        .map_err(conversation_error)
    }

    fn compare_and_set_session_binding(
        &self,
        binding_key: &str,
        expected_generation: Option<u64>,
        binding: AgentSessionBinding,
    ) -> Result<(), ConversationError> {
        let body = encode(&binding).map_err(conversation_error)?;
        let changed = match expected_generation {
            None => self.execute(
                "INSERT OR IGNORE INTO bot_agent_binding(binding_key, generation, body) VALUES (?1, ?2, ?3)",
                vec![json!(binding_key), json!(binding.generation), body],
            ),
            Some(expected) => self.execute(
                "UPDATE bot_agent_binding SET generation=?2, body=?3 WHERE binding_key=?1 AND generation=?4",
                vec![json!(binding_key), json!(binding.generation), body, json!(expected)],
            ),
        }
        .map_err(conversation_error)?;
        if changed != 1 {
            return Err(ConversationError::GenerationConflict);
        }
        Ok(())
    }

    fn begin_agent_event(
        &self,
        binding_key: &str,
        event_id: &str,
        turn_id: &str,
    ) -> Result<AgentEventClaim, ConversationError> {
        let changed = self
            .execute(
                "INSERT OR IGNORE INTO bot_agent_event(binding_key, event_id, turn_id, status) VALUES (?1, ?2, ?3, 'pending')",
                vec![json!(binding_key), json!(event_id), json!(turn_id)],
            )
            .map_err(conversation_error)?;
        if changed == 1 {
            return Ok(AgentEventClaim::New);
        }
        let status = self
            .query(
                "SELECT status FROM bot_agent_event WHERE binding_key=?1 AND event_id=?2",
                vec![json!(binding_key), json!(event_id)],
            )
            .map_err(conversation_error)?
            .into_iter()
            .next()
            .and_then(|row| row.get("status").and_then(Value::as_str).map(str::to_owned))
            .ok_or_else(|| ConversationError::Repository("agent event status missing".into()))?;
        Ok(if status == "completed" {
            AgentEventClaim::Completed
        } else {
            AgentEventClaim::ResumePending
        })
    }

    fn complete_agent_event(
        &self,
        binding_key: &str,
        event_id: &str,
    ) -> Result<(), ConversationError> {
        let changed = self
            .execute(
                "UPDATE bot_agent_event SET status='completed' WHERE binding_key=?1 AND event_id=?2",
                vec![json!(binding_key), json!(event_id)],
            )
            .map_err(conversation_error)?;
        if changed != 1 {
            return Err(ConversationError::Repository(
                "agent event claim was not found".into(),
            ));
        }
        Ok(())
    }
}

impl DeliveryRepository for BotStateDbRepository {
    fn reserve(
        &self,
        request: &BotActiveDeliveryRequest,
    ) -> Result<Option<BotDeliveryReceipt>, DeliveryError> {
        let changed = self
            .execute(
                "INSERT OR IGNORE INTO bot_delivery_request(delivery_id, idempotency_key, body) VALUES (?1, ?2, ?3)",
                vec![json!(request.delivery_id), json!(request.idempotency_key), encode(request).map_err(delivery_error)?],
            )
            .map_err(delivery_error)?;
        if changed == 1 {
            return Ok(None);
        }
        if let Some(receipt) = decode_optional::<BotDeliveryReceipt>(self.query(
            "SELECT body FROM bot_delivery_receipt WHERE idempotency_key=?1",
            vec![json!(request.idempotency_key)],
        ))
        .map_err(delivery_error)?
        {
            return Ok(Some(receipt));
        }
        if let Some(owner) = decode_optional::<BotActiveDeliveryRequest>(self.query(
            "SELECT body FROM bot_delivery_request WHERE idempotency_key=?1",
            vec![json!(request.idempotency_key)],
        ))
        .map_err(delivery_error)?
        {
            return Ok(Some(BotDeliveryReceipt {
                delivery_id: owner.delivery_id,
                idempotency_key: owner.idempotency_key,
                status: DeliveryStatus::Pending,
                attempt_count: 0,
                platform_message_ids: Vec::new(),
                part_receipts: Vec::new(),
                delivered_at_unix_ms: None,
                error_code: None,
            }));
        }
        if decode_optional::<BotActiveDeliveryRequest>(self.query(
            "SELECT body FROM bot_delivery_request WHERE delivery_id=?1",
            vec![json!(request.delivery_id)],
        ))
        .map_err(delivery_error)?
        .is_some()
        {
            return Err(DeliveryError::Conflict);
        }
        Err(DeliveryError::Repository(
            "delivery reservation disappeared after conflict".into(),
        ))
    }

    fn request(&self, delivery_id: &str) -> Result<BotActiveDeliveryRequest, DeliveryError> {
        decode_optional(self.query(
            "SELECT body FROM bot_delivery_request WHERE delivery_id=?1",
            vec![json!(delivery_id)],
        ))
        .map_err(delivery_error)?
        .ok_or(DeliveryError::NotFound)
    }

    fn receipt(&self, delivery_id: &str) -> Result<BotDeliveryReceipt, DeliveryError> {
        decode_optional(self.query(
            "SELECT body FROM bot_delivery_receipt WHERE delivery_id=?1",
            vec![json!(delivery_id)],
        ))
        .map_err(delivery_error)?
        .ok_or(DeliveryError::NotFound)
    }

    fn attempts(&self, delivery_id: &str) -> Result<Vec<BotDeliveryAttempt>, DeliveryError> {
        decode_rows(self.query(
            "SELECT body FROM bot_delivery_attempt WHERE delivery_id=?1 ORDER BY attempt, status",
            vec![json!(delivery_id)],
        ))
        .map_err(delivery_error)
    }

    fn save_attempt(&self, attempt: BotDeliveryAttempt) -> Result<(), DeliveryError> {
        self.execute(
            "INSERT OR REPLACE INTO bot_delivery_attempt(delivery_id, attempt, status, retry_at, body) VALUES (?1, ?2, ?3, ?4, ?5)",
            vec![
                json!(attempt.delivery_id),
                json!(attempt.attempt),
                json!(status_name(attempt.status)),
                json!(attempt.retry_at_unix_ms),
                encode(&attempt).map_err(delivery_error)?,
            ],
        )
        .map_err(delivery_error)?;
        Ok(())
    }

    fn save_receipt(&self, receipt: BotDeliveryReceipt) -> Result<(), DeliveryError> {
        self.execute(
            "INSERT INTO bot_delivery_receipt(delivery_id, idempotency_key, body) VALUES (?1, ?2, ?3) \
             ON CONFLICT(delivery_id) DO UPDATE SET body=excluded.body",
            vec![json!(receipt.delivery_id), json!(receipt.idempotency_key), encode(&receipt).map_err(delivery_error)?],
        )
        .map_err(delivery_error)?;
        Ok(())
    }

    fn due_delivery_ids(&self, now_unix_ms: u64) -> Result<Vec<String>, DeliveryError> {
        self.query(
            "SELECT DISTINCT a.delivery_id FROM bot_delivery_attempt a \
             WHERE a.status='retry_scheduled' AND a.retry_at<=?1 \
             AND a.attempt=(SELECT MAX(b.attempt) FROM bot_delivery_attempt b WHERE b.delivery_id=a.delivery_id)",
            vec![json!(now_unix_ms)],
        )
        .map_err(delivery_error)?
        .into_iter()
        .map(|row| {
            row.get("delivery_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| DeliveryError::Repository("delivery_id missing".into()))
        })
        .collect()
    }
}

impl InteractionRepository for BotStateDbRepository {
    fn create(&self, session: BotInteractionSession) -> Result<(), InteractionError> {
        let changed = self
            .execute(
                "INSERT OR IGNORE INTO bot_interaction(session_id, origin_key, status, version, body) VALUES (?1, ?2, ?3, ?4, ?5)",
                vec![
                    json!(session.session_id),
                    json!(session.conversation.origin_key()),
                    json!(interaction_status_name(session.status)),
                    json!(session.version),
                    encode(&session).map_err(interaction_error)?,
                ],
            )
            .map_err(interaction_error)?;
        if changed != 1 {
            return Err(InteractionError::WaiterConflict);
        }
        Ok(())
    }

    fn active_for_origin(
        &self,
        origin_key: &str,
    ) -> Result<Vec<BotInteractionSession>, InteractionError> {
        decode_rows(self.query(
            "SELECT body FROM bot_interaction WHERE origin_key=?1 AND status='waiting' ORDER BY session_id",
            vec![json!(origin_key)],
        ))
        .map_err(interaction_error)
    }

    fn compare_and_set(
        &self,
        expected_version: u64,
        session: BotInteractionSession,
    ) -> Result<(), InteractionError> {
        let changed = self
            .execute(
                "UPDATE bot_interaction SET status=?2, version=?3, body=?4 WHERE session_id=?1 AND version=?5",
                vec![
                    json!(session.session_id),
                    json!(interaction_status_name(session.status)),
                    json!(session.version),
                    encode(&session).map_err(interaction_error)?,
                    json!(expected_version),
                ],
            )
            .map_err(interaction_error)?;
        if changed != 1 {
            return Err(InteractionError::GenerationConflict);
        }
        Ok(())
    }

    fn recover_waiting(&self) -> Result<Vec<BotInteractionSession>, InteractionError> {
        decode_rows(self.query(
            "SELECT body FROM bot_interaction WHERE status='waiting' ORDER BY session_id",
            Vec::new(),
        ))
        .map_err(interaction_error)
    }
}

fn json_to_sql(value: &Value) -> SqlValue {
    match value {
        Value::Null => SqlValue::Null,
        Value::Bool(value) => SqlValue::Integer(i64::from(*value)),
        Value::Number(value) => value
            .as_i64()
            .map(SqlValue::Integer)
            .or_else(|| value.as_f64().map(SqlValue::Real))
            .unwrap_or(SqlValue::Null),
        Value::String(value) => SqlValue::Text(value.clone()),
        Value::Array(_) | Value::Object(_) => SqlValue::Text(value.to_string()),
    }
}

fn sql_to_json(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => json!(value),
        ValueRef::Real(value) => json!(value),
        ValueRef::Text(value) => Value::String(String::from_utf8_lossy(value).into_owned()),
        ValueRef::Blob(value) => Value::Array(value.iter().copied().map(Value::from).collect()),
    }
}

fn encode(value: &impl serde::Serialize) -> Result<Value, BotStateDbError> {
    serde_json::to_string(value)
        .map(Value::String)
        .map_err(|error| BotStateDbError::Serialization(error.to_string()))
}

fn decode_rows<T: serde::de::DeserializeOwned>(
    rows: Result<Vec<Value>, BotStateDbError>,
) -> Result<Vec<T>, BotStateDbError> {
    rows?.iter().map(decode_row).collect()
}

fn decode_optional<T: serde::de::DeserializeOwned>(
    rows: Result<Vec<Value>, BotStateDbError>,
) -> Result<Option<T>, BotStateDbError> {
    rows?.first().map(decode_row).transpose()
}

fn decode_row<T: serde::de::DeserializeOwned>(row: &Value) -> Result<T, BotStateDbError> {
    let body = row
        .get("body")
        .and_then(Value::as_str)
        .ok_or_else(|| BotStateDbError::Serialization("row body missing".into()))?;
    serde_json::from_str(body).map_err(|error| BotStateDbError::Serialization(error.to_string()))
}

fn status_name(status: DeliveryStatus) -> &'static str {
    match status {
        DeliveryStatus::Pending => "pending",
        DeliveryStatus::Sending => "sending",
        DeliveryStatus::Succeeded => "succeeded",
        DeliveryStatus::RetryScheduled => "retry_scheduled",
        DeliveryStatus::PermanentlyFailed => "permanently_failed",
        DeliveryStatus::Cancelled => "cancelled",
        DeliveryStatus::Previewed => "previewed",
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

fn conversation_error(error: BotStateDbError) -> ConversationError {
    ConversationError::Repository(error_message(error))
}

fn delivery_error(error: BotStateDbError) -> DeliveryError {
    DeliveryError::Repository(error_message(error))
}

fn interaction_error(error: BotStateDbError) -> InteractionError {
    InteractionError::Repository(error_message(error))
}

fn error_message(error: BotStateDbError) -> String {
    match error {
        BotStateDbError::Database(error) => error.to_string(),
        BotStateDbError::Serialization(message) => message,
    }
}

#[derive(Debug, Error)]
pub enum BotStateDbError {
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error("{0}")]
    Serialization(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_bot_protocol::{
        AgentSessionScope, BotConversationKind, BotDeliveryContent, BotInteractionSession,
        BotPropagationPolicy, ConversationPolicyMatch, ConversationPolicyPatch, DeliveryPolicy,
        InteractionScope, InteractionWaitSpec, MessageSegment, QQ_CONVERSATION_REF_VERSION,
        QqConversationRef,
    };

    #[test]
    fn sqlite_repository_recovers_policy_binding_delivery_and_interaction_after_reopen() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("bot-state.db");
        let repository = BotStateDbRepository::open(&path).unwrap();
        let conversation = conversation();
        repository.upsert_policy_rule(&policy_rule()).unwrap();
        ConversationRepository::compare_and_set_session_binding(
            &repository,
            &conversation.origin_key(),
            None,
            binding(&conversation),
        )
        .unwrap();
        assert_eq!(
            ConversationRepository::begin_agent_event(
                &repository,
                &conversation.origin_key(),
                "event",
                "turn",
            )
            .unwrap(),
            AgentEventClaim::New
        );
        ConversationRepository::complete_agent_event(
            &repository,
            &conversation.origin_key(),
            "event",
        )
        .unwrap();

        let request = delivery(&conversation);
        assert!(
            DeliveryRepository::reserve(&repository, &request)
                .unwrap()
                .is_none()
        );
        DeliveryRepository::save_attempt(
            &repository,
            BotDeliveryAttempt {
                delivery_id: request.delivery_id.clone(),
                attempt: 1,
                status: DeliveryStatus::RetryScheduled,
                started_at_unix_ms: 100,
                retry_at_unix_ms: Some(150),
                error_code: Some("qq.rate_limited".into()),
            },
        )
        .unwrap();
        DeliveryRepository::save_receipt(
            &repository,
            BotDeliveryReceipt {
                delivery_id: request.delivery_id.clone(),
                idempotency_key: request.idempotency_key.clone(),
                status: DeliveryStatus::RetryScheduled,
                attempt_count: 1,
                platform_message_ids: Vec::new(),
                part_receipts: Vec::new(),
                delivered_at_unix_ms: None,
                error_code: Some("qq.rate_limited".into()),
            },
        )
        .unwrap();
        InteractionRepository::create(&repository, interaction(&conversation)).unwrap();
        drop(repository);

        let reopened = BotStateDbRepository::open(&path).unwrap();
        assert_eq!(
            ConversationRepository::policy_rules(&reopened)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            ConversationRepository::session_binding(&reopened, &conversation.origin_key())
                .unwrap()
                .unwrap()
                .session_id,
            "session"
        );
        assert_eq!(
            ConversationRepository::begin_agent_event(
                &reopened,
                &conversation.origin_key(),
                "event",
                "turn",
            )
            .unwrap(),
            AgentEventClaim::Completed
        );
        assert_eq!(
            DeliveryRepository::due_delivery_ids(&reopened, 150).unwrap(),
            vec!["delivery".to_string()]
        );
        assert_eq!(
            DeliveryRepository::reserve(&reopened, &request)
                .unwrap()
                .unwrap()
                .attempt_count,
            1
        );
        assert_eq!(
            InteractionRepository::recover_waiting(&reopened)
                .unwrap()
                .len(),
            1
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

    fn binding(conversation: &QqConversationRef) -> AgentSessionBinding {
        AgentSessionBinding {
            origin_key: conversation.origin_key(),
            session_id: "session".into(),
            session_version: 2,
            last_event_sequence: 3,
            policy_revision: 2,
            generation: 1,
        }
    }

    fn delivery(conversation: &QqConversationRef) -> BotActiveDeliveryRequest {
        BotActiveDeliveryRequest {
            delivery_id: "delivery".into(),
            idempotency_key: "key".into(),
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
