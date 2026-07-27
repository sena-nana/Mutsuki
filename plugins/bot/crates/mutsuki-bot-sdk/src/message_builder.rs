use mutsuki_bot_protocol::{BotExtMap, BotMessage, BotTarget, MessageSegment};

#[derive(Clone, Debug)]
pub struct MessageBuilder {
    target: BotTarget,
    segments: Vec<MessageSegment>,
    reply_to: Option<String>,
}

impl MessageBuilder {
    pub fn new(target: BotTarget) -> Self {
        Self {
            target,
            segments: Vec::new(),
            reply_to: None,
        }
    }

    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.segments.push(MessageSegment::text(text));
        self
    }

    pub fn mention_user(mut self, user_id: impl Into<String>) -> Self {
        self.segments.push(MessageSegment::MentionUser {
            user_id: user_id.into(),
        });
        self
    }

    pub fn reply_to(mut self, message_id: impl Into<String>) -> Self {
        self.reply_to = Some(message_id.into());
        self
    }

    pub fn build(self) -> BotMessage {
        BotMessage {
            message_id: None,
            target: self.target,
            sender: None,
            segments: self.segments,
            reply_to: self.reply_to,
            time_ms: None,
            ext: BotExtMap::new(),
        }
    }
}
