use serde::{Deserialize, Serialize};

use crate::BotEvent;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotCommandEvent {
    pub source: BotEvent,
    pub name: String,
    pub args: Vec<String>,
    pub raw_text: String,
}

/// Stable target binding used by the generic command parser and command-owner manifests.
pub fn bot_command_binding_id(name: &str) -> String {
    format!(
        "binding:mutsuki.bot.command/{}@1",
        name.trim().to_ascii_lowercase()
    )
}
