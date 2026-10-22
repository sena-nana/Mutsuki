//! The crate error type and its mapping onto each store trait's error.

use mutsuki_bot_conversation::ConversationError;
use mutsuki_bot_delivery::DeliveryError;
use mutsuki_bot_interaction::InteractionError;
use thiserror::Error;

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
    #[error("state revision or idempotency reservation conflicts with the request")]
    Conflict,
}

pub(crate) fn conversation_error(error: BotStateDbError) -> ConversationError {
    ConversationError::Repository(state_db_error_message(error))
}

pub(crate) fn delivery_error(error: BotStateDbError) -> DeliveryError {
    DeliveryError::Repository(state_db_error_message(error))
}

pub(crate) fn interaction_error(error: BotStateDbError) -> InteractionError {
    InteractionError::Repository(state_db_error_message(error))
}

pub(crate) fn state_db_error_message(error: BotStateDbError) -> String {
    let message = error.to_string();
    drop(error);
    message
}
