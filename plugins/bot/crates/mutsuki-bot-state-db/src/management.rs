//! Revision-fenced management audit log and durable management operation reservations.

use rusqlite::{Connection, OptionalExtension, params};

use crate::codec::{decode, encode, immediate, sqlite_integer, sqlite_unsigned};
use crate::error::BotStateDbError;

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

pub(crate) fn management_revision(connection: &Connection) -> Result<u64, BotStateDbError> {
    let revision = connection.query_row(
        "SELECT revision FROM bot_management_meta WHERE singleton=1",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    sqlite_unsigned(revision, "management revision")
}

pub(crate) fn management_audits(
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

pub(crate) fn begin_management_operation(
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

pub(crate) fn complete_management_operation(
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

pub(crate) fn commit_management_audit(
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
