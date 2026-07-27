use mutsuki_runtime_contracts::ResourceRef;
use serde::{Deserialize, Serialize};

use crate::{BotAccountRef, BotExtMap, BotMessage, BotPlatform, BotTarget, BotUser};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotEvent {
    pub event_id: String,
    pub platform: BotPlatform,
    pub bot: BotAccountRef,
    pub kind: BotEventKind,
    pub time_ms: i64,
    pub target: BotTarget,
    pub actor: Option<BotUser>,
    pub message: Option<BotMessage>,
    pub raw: Option<ResourceRef>,
    pub ext: BotExtMap,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotEventKind {
    MessageCreated,
    MessageUpdated,
    MessageDeleted,
    MemberJoined,
    MemberLeft,
    ReactionAdded,
    ReactionRemoved,
    BotConnected,
    BotDisconnected,
    PlatformSpecific(String),
}
