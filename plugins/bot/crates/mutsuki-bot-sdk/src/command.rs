use std::collections::BTreeMap;

use mutsuki_bot_protocol::{BotCommandArgumentValue, BotCommandEvent, BotEvent};

#[derive(Clone, Debug, PartialEq)]
pub struct CommandContext {
    pub source: BotEvent,
    pub name: String,
    pub args: Vec<String>,
    pub command_path: Vec<String>,
    pub typed_args: BTreeMap<String, BotCommandArgumentValue>,
    pub source_event_id: String,
    pub raw_text: String,
}

impl CommandContext {
    pub fn from_event(event: &BotEvent, name: impl Into<String>, args: Vec<String>) -> Self {
        let name = name.into();
        Self {
            source: event.clone(),
            command_path: name.split('.').map(str::to_owned).collect(),
            name,
            args,
            typed_args: BTreeMap::new(),
            source_event_id: event.event_id.clone(),
            raw_text: String::new(),
        }
    }

    pub fn from_command_event(event: BotCommandEvent) -> Self {
        let source_event_id = event.source.event_id.clone();
        Self {
            source: event.source,
            name: event.name,
            args: event.args,
            command_path: event.command_path,
            typed_args: event.typed_args,
            source_event_id,
            raw_text: event.raw_text,
        }
    }
}

impl From<BotCommandEvent> for CommandContext {
    fn from(event: BotCommandEvent) -> Self {
        Self::from_command_event(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_bot_protocol::{BotAccountRef, BotEventKind, BotPlatform, BotTarget, BotUser};

    #[test]
    fn command_context_preserves_typed_group_data() {
        let source = BotEvent {
            event_id: "event".into(),
            platform: BotPlatform::QqBot,
            bot: BotAccountRef {
                account_id: "bot".into(),
                platform: BotPlatform::QqBot,
            },
            kind: BotEventKind::MessageCreated,
            time_ms: 1,
            target: BotTarget::Group {
                group_id: "group".into(),
            },
            actor: Some(BotUser {
                user_id: "actor".into(),
                display_name: None,
                avatar_url: None,
            }),
            message: None,
            raw: None,
            ext: Default::default(),
        };
        let event = BotCommandEvent {
            source,
            name: "admin.ban".into(),
            args: vec!["alice".into(), "7".into()],
            command_path: vec!["admin".into(), "ban".into()],
            typed_args: BTreeMap::from([("days".into(), BotCommandArgumentValue::Integer(7))]),
            raw_text: "/admin ban alice 7".into(),
        };

        let context = CommandContext::from(event);
        assert_eq!(context.command_path, ["admin", "ban"]);
        assert_eq!(
            context.typed_args["days"],
            BotCommandArgumentValue::Integer(7)
        );
    }
}
