//! Shared row helpers: JSON body codec, SQLite integer conversions, status column names,
//! immediate transactions and page limits.

use mutsuki_bot_protocol::{DeliveryStatus, InteractionStatus};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::error::BotStateDbError;

pub(crate) fn immediate(connection: &mut Connection) -> Result<Transaction<'_>, BotStateDbError> {
    Ok(connection.transaction_with_behavior(TransactionBehavior::Immediate)?)
}

pub(crate) fn optional_body<T: serde::de::DeserializeOwned>(
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

pub(crate) fn bounded_page_limit(limit: u32) -> u32 {
    limit.clamp(1, 100)
}

pub(crate) fn encode(value: &impl serde::Serialize) -> Result<String, BotStateDbError> {
    serde_json::to_string(value).map_err(|error| BotStateDbError::Serialization(error.to_string()))
}

pub(crate) fn decode<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, BotStateDbError> {
    serde_json::from_str(body).map_err(|error| BotStateDbError::Serialization(error.to_string()))
}

pub(crate) fn sqlite_integer(value: u64) -> Result<i64, BotStateDbError> {
    i64::try_from(value)
        .map_err(|_| BotStateDbError::Invariant(format!("integer {value} exceeds SQLite range")))
}

pub(crate) fn sqlite_unsigned(value: i64, name: &str) -> Result<u64, BotStateDbError> {
    u64::try_from(value)
        .map_err(|_| BotStateDbError::Invariant(format!("negative {name}: {value}")))
}

pub(crate) fn delivery_status_name(status: DeliveryStatus) -> &'static str {
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

pub(crate) fn interaction_status_name(status: InteractionStatus) -> &'static str {
    match status {
        InteractionStatus::Waiting => "waiting",
        InteractionStatus::Completed => "completed",
        InteractionStatus::TimedOut => "timed_out",
        InteractionStatus::Cancelled => "cancelled",
        InteractionStatus::Failed => "failed",
    }
}
