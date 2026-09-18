//! Persona persistence contract.
//!
//! Lives beside `ConversationContextStore` rather than in its own crate: both are
//! plain persistence traits over conversation-scoped state, both are implemented by
//! `mutsuki-bot-state-db`, and neither carries behavior of its own.

use mutsuki_bot_protocol::BotPersona;

pub trait PersonaStore: Send + Sync {
    fn upsert(&self, persona: BotPersona) -> Result<(), String>;
    fn list(&self) -> Result<Vec<BotPersona>, String>;
    fn get(&self, persona_id: &str) -> Result<Option<BotPersona>, String>;
    fn bind_conversation(&self, origin_key: &str, persona_id: &str) -> Result<(), String>;
    fn conversation_persona(&self, origin_key: &str) -> Result<Option<String>, String>;
}
