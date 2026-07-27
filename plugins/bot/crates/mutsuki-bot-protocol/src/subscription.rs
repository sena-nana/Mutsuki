use serde::{Deserialize, Serialize};

use crate::BotEventKind;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotEventSubscription {
    pub subscription_id: String,
    pub handler_protocol_id: String,
    pub handler_binding_id: Option<String>,
    pub platform: Option<String>,
    pub event_kind: Option<BotEventKind>,
}

impl BotEventSubscription {
    pub fn new(subscription_id: impl Into<String>, handler_protocol_id: impl Into<String>) -> Self {
        Self {
            subscription_id: subscription_id.into(),
            handler_protocol_id: handler_protocol_id.into(),
            handler_binding_id: None,
            platform: None,
            event_kind: None,
        }
    }
}
