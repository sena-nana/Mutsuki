//! `ConversationContextStore`: bounded per-conversation ICL history.

use mutsuki_bot_conversation::ConversationContextStore;
use mutsuki_bot_protocol::ConversationIclEntry;
use rusqlite::{Connection, params};

use crate::codec::{decode, encode};
use crate::error::{BotStateDbError, state_db_error_message};
use crate::repository::BotStateDbRepository;

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
