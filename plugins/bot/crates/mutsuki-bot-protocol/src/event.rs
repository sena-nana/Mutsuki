use mutsuki_runtime_contracts::ResourceRef;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{BotAccountRef, BotExtMap, BotMessage, BotPlatform, BotTarget, BotUser};

/// Event ext flag: this message was sent by the bot itself and must not enter Flow sources.
pub const BOT_SELF_SENT_EXT_KEY: &str = "bot.self_sent";

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

impl BotEventKind {
    /// Message create/update/delete events that can loop if the bot is the actor.
    #[must_use]
    pub fn is_message_event(&self) -> bool {
        matches!(
            self,
            Self::MessageCreated | Self::MessageUpdated | Self::MessageDeleted
        )
    }
}

impl BotEvent {
    /// True when Flow must ignore this event because the bot sent the message.
    #[must_use]
    pub fn is_self_sent_message(&self) -> bool {
        self.kind.is_message_event()
            && self
                .ext
                .get(BOT_SELF_SENT_EXT_KEY)
                .and_then(Value::as_bool)
                .unwrap_or(false)
    }
}

/// Marks a message event as bot-originated when `actor.user_id` equals the connected bot OpenID.
///
/// Returns whether the event is a self-sent message after the update.
#[must_use]
pub fn apply_bot_self_sent(event: &mut BotEvent, self_user_id: Option<&str>) -> bool {
    let self_id = self_user_id
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if event.kind.is_message_event()
        && event
            .actor
            .as_ref()
            .is_some_and(|actor| Some(actor.user_id.as_str()) == self_id)
    {
        event
            .ext
            .insert(BOT_SELF_SENT_EXT_KEY.into(), Value::Bool(true));
    }
    event.is_self_sent_message()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BotPlatform;

    fn message_event(actor_id: &str) -> BotEvent {
        BotEvent {
            event_id: "e1".into(),
            platform: BotPlatform::QqBot,
            bot: BotAccountRef {
                account_id: "qq-main".into(),
                platform: BotPlatform::QqBot,
            },
            kind: BotEventKind::MessageCreated,
            time_ms: 1,
            target: crate::BotTarget::Group {
                group_id: "g1".into(),
            },
            actor: Some(BotUser {
                user_id: actor_id.into(),
                display_name: Some("bot".into()),
                avatar_url: None,
            }),
            message: None,
            raw: None,
            ext: BotExtMap::new(),
        }
    }

    #[test]
    fn apply_bot_self_sent_marks_matching_actor() {
        let mut event = message_event("BOT_OPENID");
        assert!(apply_bot_self_sent(&mut event, Some("BOT_OPENID")));
        assert!(event.is_self_sent_message());
        let mut other = message_event("member-1");
        assert!(!apply_bot_self_sent(&mut other, Some("BOT_OPENID")));
        assert!(!other.is_self_sent_message());
    }
}
