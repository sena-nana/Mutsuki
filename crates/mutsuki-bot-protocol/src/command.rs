use serde::{Deserialize, Serialize};

use crate::BotEvent;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotCommandEvent {
    pub source: BotEvent,
    pub name: String,
    pub args: Vec<String>,
    pub raw_text: String,
}
