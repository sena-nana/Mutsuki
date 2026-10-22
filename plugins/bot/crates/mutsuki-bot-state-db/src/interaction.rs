//! `InteractionRepository`: versioned interaction waiter sessions.

use mutsuki_bot_interaction::{InteractionError, InteractionRepository};
use mutsuki_bot_protocol::{BotInteractionSession, InteractionStatus};
use rusqlite::{Connection, params};

use crate::codec::{decode, encode, immediate, interaction_status_name, sqlite_integer};
use crate::error::{BotStateDbError, interaction_error};
use crate::repository::{BotStateDbRepository, BotStatePage};

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

pub(crate) fn interaction_page(
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
