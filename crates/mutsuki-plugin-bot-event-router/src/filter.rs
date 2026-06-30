use mutsuki_bot_protocol::{BotEvent, BotEventSubscription};

pub fn matches_subscription(event: &BotEvent, subscription: &BotEventSubscription) -> bool {
    if let Some(platform) = &subscription.platform
        && platform != event.platform.as_str()
        && platform != "any"
    {
        return false;
    }
    if let Some(kind) = &subscription.event_kind
        && kind != &event.kind
    {
        return false;
    }
    true
}
