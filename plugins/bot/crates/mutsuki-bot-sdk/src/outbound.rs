//! Shared semantics of the outbound send nodes.
//!
//! The QQ `mutsuki.bot.qq.send` node and the platform-neutral `mutsuki.bot.send` node read the
//! same config (`text` overrides the body, `reply` quotes the triggering message) from the same
//! inputs (an outbound `BotMessage` or the triggering `BotEvent`). Both resolve the message here so
//! a graph behaves the same whichever sink it uses.

use mutsuki_bot_protocol::{
    BotAccountRef, BotEvent, BotMessage, BotNodeInvocation, MessageSegment,
};
use serde::Deserialize;
use serde_json::Value;

/// The message a send node delivers for `payload`, applying the node config of `invocation`.
///
/// # Errors
///
/// Returns the decode error when `payload` is neither a `BotMessage` nor a `BotEvent`.
pub fn send_message_from_payload(
    payload: &Value,
    invocation: Option<&BotNodeInvocation>,
) -> Result<BotMessage, serde_json::Error> {
    let text = invocation
        .and_then(|item| item.config.get("text"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let reply = invocation
        .and_then(|item| item.config.get("reply"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if let Ok(mut message) = BotMessage::deserialize(payload) {
        if let Some(text) = text {
            message.segments = vec![MessageSegment::text(text)];
        }
        if reply && message.reply_to.is_none() {
            message.reply_to = invocation.and_then(triggering_message_id);
        }
        return Ok(message);
    }
    let event = BotEvent::deserialize(payload)?;
    let body = text
        .or_else(|| event.message.as_ref().map(BotMessage::plain_text))
        .unwrap_or_default();
    let mut message = BotMessage::text(event.target.clone(), body);
    if reply {
        message.reply_to = event
            .message
            .as_ref()
            .and_then(|item| item.message_id.clone());
    }
    Ok(message)
}

/// The bot account an invocation acts for: the Flow context, else the triggering event's bot.
pub fn invocation_account(invocation: &BotNodeInvocation) -> Option<BotAccountRef> {
    invocation.input.context.bot.clone().or_else(|| {
        BotEvent::deserialize(&invocation.input.payload.value)
            .ok()
            .map(|event| event.bot)
    })
}

fn triggering_message_id(invocation: &BotNodeInvocation) -> Option<String> {
    BotEvent::deserialize(&invocation.input.payload.value)
        .ok()
        .and_then(|event| event.message.and_then(|item| item.message_id))
}

#[cfg(test)]
mod tests {
    use mutsuki_bot_protocol::{
        BotEventKind, BotExtMap, BotFlowEventEnvelope, BotNodeWiring, BotPlatform, BotTarget,
    };
    use serde_json::json;

    use super::*;

    fn event() -> BotEvent {
        let mut message = BotMessage::text(
            BotTarget::Group {
                group_id: "g1".into(),
            },
            "hello",
        );
        message.message_id = Some("m1".into());
        BotEvent {
            event_id: "e1".into(),
            platform: BotPlatform::Sandbox,
            bot: BotAccountRef {
                account_id: "sandbox".into(),
                platform: BotPlatform::Sandbox,
            },
            kind: BotEventKind::MessageCreated,
            time_ms: 1,
            target: BotTarget::Group {
                group_id: "g1".into(),
            },
            actor: None,
            message: Some(message),
            raw: None,
            ext: BotExtMap::new(),
        }
    }

    fn invocation(input: BotFlowEventEnvelope, config: Value) -> BotNodeInvocation {
        BotNodeInvocation {
            flow_id: "flow".into(),
            graph_revision: 1,
            execution_id: "exec".into(),
            node_id: "send".into(),
            input_port_id: "event".into(),
            wiring: BotNodeWiring::default(),
            config,
            input,
        }
    }

    #[test]
    fn event_input_echoes_or_overrides_and_quotes_the_trigger() {
        let envelope = BotFlowEventEnvelope::from_bot_event(event(), None, None).unwrap();
        let echo = invocation(envelope.clone(), json!({}));
        let message = send_message_from_payload(&echo.input.payload.value, Some(&echo)).unwrap();
        assert_eq!(message.plain_text(), "hello");
        assert_eq!(message.reply_to, None);

        let fixed = invocation(envelope, json!({"text": " pong ", "reply": true}));
        let message = send_message_from_payload(&fixed.input.payload.value, Some(&fixed)).unwrap();
        assert_eq!(message.plain_text(), "pong");
        assert_eq!(message.reply_to.as_deref(), Some("m1"));
        assert_eq!(
            invocation_account(&fixed),
            Some(BotAccountRef {
                account_id: "sandbox".into(),
                platform: BotPlatform::Sandbox,
            })
        );
    }
}
