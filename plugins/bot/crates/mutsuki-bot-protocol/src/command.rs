use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::BotEvent;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotCommandEvent {
    pub source: BotEvent,
    pub name: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub command_path: Vec<String>,
    #[serde(default)]
    pub typed_args: BTreeMap<String, BotCommandArgumentValue>,
    pub raw_text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum BotCommandArgumentValue {
    String(String),
    Integer(i64),
    Number(f64),
    Boolean(bool),
    Strings(Vec<String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotCommandArgumentKind {
    String,
    Integer,
    Number,
    Boolean,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotCommandArgumentDescriptor {
    pub name: String,
    pub kind: BotCommandArgumentKind,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub variadic: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<BotCommandArgumentValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotCommandDescriptor {
    pub path: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<Vec<String>>,
    #[serde(default)]
    pub arguments: Vec<BotCommandArgumentDescriptor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotCommandHelpRequest {
    #[serde(default)]
    pub path: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotCommandHelpEntry {
    pub path: Vec<String>,
    pub aliases: Vec<Vec<String>>,
    pub arguments: Vec<BotCommandArgumentDescriptor>,
    pub summary: Option<String>,
    pub usage: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BotCommandHelpResult {
    pub commands: Vec<BotCommandHelpEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotCommandParseErrorCode {
    MissingPrefix,
    EmptyName,
    UnterminatedQuote,
    UnknownCommand,
    MissingArgument,
    InvalidArgument,
    UnexpectedArgument,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotCommandParseFailure {
    pub code: BotCommandParseErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<BotCommandHelpEntry>,
}

/// Stable target binding used by the generic command parser and command-owner manifests.
pub fn bot_command_binding_id(name: &str) -> String {
    format!(
        "binding:mutsuki.bot.command/{}@1",
        name.trim().to_ascii_lowercase()
    )
}
