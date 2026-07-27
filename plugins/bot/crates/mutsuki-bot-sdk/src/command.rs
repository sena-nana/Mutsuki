use mutsuki_bot_protocol::{BotCommandEvent, BotEvent};

#[derive(Clone, Debug, PartialEq)]
pub struct CommandContext {
    pub source: BotEvent,
    pub name: String,
    pub args: Vec<String>,
    pub source_event_id: String,
    pub raw_text: String,
}

impl CommandContext {
    pub fn from_event(event: &BotEvent, name: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            source: event.clone(),
            name: name.into(),
            args,
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
