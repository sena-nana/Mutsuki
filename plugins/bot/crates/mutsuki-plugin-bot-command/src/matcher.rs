use mutsuki_bot_protocol::{BotEvent, BotEventKind};

pub fn message_text(event: &BotEvent) -> Option<String> {
    if event.kind != BotEventKind::MessageCreated {
        return None;
    }
    event.message.as_ref().map(|message| message.plain_text())
}
