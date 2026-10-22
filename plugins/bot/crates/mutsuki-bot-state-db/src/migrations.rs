//! Schema creation and versioned migrations, applied once when the actor opens.

use mutsuki_bot_protocol::BotDeliveryReceipt;
use rusqlite::{Connection, params};

use crate::codec::{decode, delivery_status_name};
use crate::error::BotStateDbError;
use crate::sandbox_history;

pub(crate) fn migrate_schema(connection: &Connection) -> Result<(), BotStateDbError> {
    let user_version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let sql = format!(
        "BEGIN IMMEDIATE;
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
    if user_version < 13 {
        connection.execute_batch(
            "DROP TABLE IF EXISTS bot_conversation_policy;
             DROP TABLE IF EXISTS bot_conversation_policy_meta;
             DROP TABLE IF EXISTS bot_conversation_policy_audit;",
        )?;
    }
    connection.pragma_update(None, "user_version", 13)?;
    Ok(())
}
