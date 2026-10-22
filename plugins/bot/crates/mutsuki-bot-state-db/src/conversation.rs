//! `ConversationRepository`: agent session bindings and agent event claims.

use async_trait::async_trait;
use mutsuki_bot_conversation::{AgentEventClaim, ConversationError, ConversationRepository};
use mutsuki_bot_protocol::AgentSessionBinding;
use rusqlite::{Connection, OptionalExtension, params};

use crate::codec::{decode, encode, immediate, sqlite_integer};
use crate::error::{BotStateDbError, conversation_error};
use crate::repository::BotStateDbRepository;

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
