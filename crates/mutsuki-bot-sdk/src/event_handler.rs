use mutsuki_bot_protocol::{BotEventKind, BotEventSubscription};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventHandlerSpec {
    pub protocol_id: String,
    pub subscription_id: Option<String>,
    pub handler_binding_id: Option<String>,
    pub event_kind: Option<BotEventKind>,
    pub platform: Option<String>,
}

impl EventHandlerSpec {
    pub fn new(protocol_id: impl Into<String>) -> Self {
        Self {
            protocol_id: protocol_id.into(),
            subscription_id: None,
            handler_binding_id: None,
            event_kind: None,
            platform: None,
        }
    }

    pub fn subscription_id(mut self, subscription_id: impl Into<String>) -> Self {
        self.subscription_id = Some(subscription_id.into());
        self
    }

    pub fn handler_binding_id(mut self, handler_binding_id: impl Into<String>) -> Self {
        self.handler_binding_id = Some(handler_binding_id.into());
        self
    }

    pub fn event_kind(mut self, kind: BotEventKind) -> Self {
        self.event_kind = Some(kind);
        self
    }

    pub fn platform(mut self, platform: impl Into<String>) -> Self {
        self.platform = Some(platform.into());
        self
    }

    pub fn into_subscription(self) -> BotEventSubscription {
        BotEventSubscription {
            subscription_id: self
                .subscription_id
                .unwrap_or_else(|| self.protocol_id.clone()),
            handler_protocol_id: self.protocol_id,
            handler_binding_id: self.handler_binding_id,
            platform: self.platform,
            event_kind: self.event_kind,
        }
    }
}

impl From<EventHandlerSpec> for BotEventSubscription {
    fn from(spec: EventHandlerSpec) -> Self {
        spec.into_subscription()
    }
}
