use mutsuki_bot_protocol::BotEvent;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandContext {
    pub name: String,
    pub args: Vec<String>,
    pub source_event_id: String,
}

impl CommandContext {
    pub fn from_event(event: &BotEvent, name: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            name: name.into(),
            args,
            source_event_id: event.event_id.clone(),
        }
    }
}
