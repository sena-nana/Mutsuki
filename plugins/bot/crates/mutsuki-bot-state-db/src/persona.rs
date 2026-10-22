//! `PersonaStore`: persona definitions and per-conversation persona bindings.

use mutsuki_bot_conversation::PersonaStore;
use mutsuki_bot_protocol::BotPersona;
use rusqlite::{Connection, OptionalExtension, params};

use crate::codec::{decode, encode};
use crate::error::{BotStateDbError, state_db_error_message};
use crate::repository::BotStateDbRepository;

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
