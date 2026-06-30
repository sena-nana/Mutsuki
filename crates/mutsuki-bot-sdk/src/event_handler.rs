use mutsuki_bot_protocol::BotEventKind;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventHandlerSpec {
    pub protocol_id: String,
    pub event_kind: Option<BotEventKind>,
    pub platform: Option<String>,
}

impl EventHandlerSpec {
    pub fn new(protocol_id: impl Into<String>) -> Self {
        Self {
            protocol_id: protocol_id.into(),
            event_kind: None,
            platform: None,
        }
    }

    pub fn event_kind(mut self, kind: BotEventKind) -> Self {
        self.event_kind = Some(kind);
        self
    }

    pub fn platform(mut self, platform: impl Into<String>) -> Self {
        self.platform = Some(platform.into());
        self
    }
}
