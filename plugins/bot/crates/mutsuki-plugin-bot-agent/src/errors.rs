#[allow(clippy::wildcard_imports)]
use super::*;

#[derive(Debug, Error)]
pub enum BotAgentError {
    #[error(transparent)]
    Conversation(#[from] ConversationError),
    #[error("Agent client failed: {code}: {message}")]
    AgentClient { code: String, message: String },
    #[error("Agent is disabled for this conversation")]
    AgentDisabled,
    #[error("Bot Agent configuration is invalid: {0}")]
    Config(#[from] BotAgentConfigError),
    #[error("Agent runtime profile binding is missing")]
    AgentProfileMissing,
    #[error("Bot message is missing")]
    MessageMissing,
    #[error("Bot media ResourceRef is missing provider, MIME schema, size, or content hash")]
    MediaResourceUnvalidated,
    #[error("Agent client mutex was poisoned")]
    ClientPoisoned,
    #[error("Agent session binding is missing")]
    SessionBindingMissing,
    #[error("Agent command is invalid or unsupported")]
    InvalidCommand,
}

impl From<AgentWireError> for BotAgentError {
    fn from(error: AgentWireError) -> Self {
        Self::AgentClient {
            code: error.code,
            message: error.message,
        }
    }
}
