use mutsuki_bot_protocol::{
    BotAccountRef, BotEvent, BotEventKind, BotExtMap, BotMessage, BotPlatform, BotUser,
    MessageSegment,
};
use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::adapter::qq_target_from_payload;
use crate::gateway::{GatewayFrame, dedup_key};

pub fn qq_gateway_frame_to_bot_event(
    account_id: &str,
    frame: GatewayFrame,
) -> Result<BotEvent, String> {
    if frame.op != 0 {
        return Err(format!("expected_dispatch_op:{}", frame.op));
    }
    let event_type = frame.t.as_deref().unwrap_or("UNKNOWN");
    let data = &frame.d;
    let target = qq_target_from_payload(event_type, data);
    let actor = qq_actor(data);
    let message = qq_message(event_type, data, target.clone(), actor.clone());
    let mut ext = BotExtMap::new();
    ext.insert("qqbot.event_type".into(), Value::String(event_type.into()));
    ext.insert("qqbot.dedup_key".into(), Value::String(dedup_key(&frame)));
    Ok(BotEvent {
        event_id: frame
            .id
            .clone()
            .or_else(|| data.get("id").and_then(Value::as_str).map(str::to_owned))
            .or_else(|| frame.s.map(|sequence| format!("seq:{sequence}")))
            .unwrap_or_else(|| format!("{event_type}:unknown")),
        platform: BotPlatform::QqBot,
        bot: BotAccountRef {
            account_id: account_id.into(),
            platform: BotPlatform::QqBot,
        },
        kind: qq_event_kind(event_type),
        time_ms: event_time_ms(data).unwrap_or(0),
        target,
        actor,
        message,
        raw: None,
        ext,
    })
}

fn qq_event_kind(event_type: &str) -> BotEventKind {
    match event_type {
        "GROUP_MESSAGE_CREATE" | "GROUP_AT_MESSAGE_CREATE" | "C2C_MESSAGE_CREATE" => {
            BotEventKind::MessageCreated
        }
        "MESSAGE_REACTION_ADD" => BotEventKind::ReactionAdded,
        "MESSAGE_REACTION_REMOVE" => BotEventKind::ReactionRemoved,
        "GROUP_MEMBER_ADD" | "FRIEND_ADD" => BotEventKind::MemberJoined,
        "GROUP_MEMBER_REMOVE" | "FRIEND_DEL" => BotEventKind::MemberLeft,
        "READY" | "RESUMED" => BotEventKind::BotConnected,
        _ => BotEventKind::PlatformSpecific(event_type.into()),
    }
}

fn qq_actor(data: &Value) -> Option<BotUser> {
    let author = data.get("author").unwrap_or(data);
    let user_id = author
        .get("member_openid")
        .or_else(|| author.get("group_member_openid"))
        .or_else(|| author.get("user_openid"))
        .or_else(|| author.get("openid"))
        .or_else(|| author.get("user_id"))
        .or_else(|| author.get("id"))
        .and_then(Value::as_str)?;
    Some(BotUser {
        user_id: user_id.into(),
        display_name: author
            .get("username")
            .or_else(|| author.get("nick"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        avatar_url: author
            .get("avatar")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

fn qq_message(
    event_type: &str,
    data: &Value,
    target: mutsuki_bot_protocol::BotTarget,
    actor: Option<BotUser>,
) -> Option<BotMessage> {
    if !matches!(
        event_type,
        "GROUP_MESSAGE_CREATE" | "GROUP_AT_MESSAGE_CREATE" | "C2C_MESSAGE_CREATE"
    ) {
        return None;
    }
    let content = message_content(event_type, data);
    Some(BotMessage {
        message_id: data.get("id").and_then(Value::as_str).map(str::to_owned),
        target,
        sender: actor,
        segments: vec![MessageSegment::Text { text: content }],
        reply_to: data
            .get("message_reference")
            .and_then(|reference| reference.get("message_id"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        time_ms: event_time_ms(data),
        ext: BotExtMap::new(),
    })
}

fn message_content(event_type: &str, data: &Value) -> String {
    let raw = data
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let decoded = html_escape::decode_html_entities(raw);
    let mut content = decoded.trim();
    if event_type == "GROUP_AT_MESSAGE_CREATE"
        && let Some(bot_id) = data
            .get("mentions")
            .and_then(Value::as_array)
            .and_then(|mentions| {
                mentions.iter().find_map(|mention| {
                    mention
                        .get("is_you")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                        .then(|| mention.get("id").and_then(Value::as_str))
                        .flatten()
                })
            })
    {
        let prefixes = [
            format!("<@{bot_id}>"),
            format!("<@!{bot_id}>"),
            format!("<qqbot-at-user id=\"{bot_id}\" />"),
        ];
        if let Some(remainder) = prefixes
            .iter()
            .find_map(|prefix| content.strip_prefix(prefix))
        {
            content = remainder.trim_start();
        }
    }
    content.trim().to_owned()
}

fn event_time_ms(data: &Value) -> Option<i64> {
    if let Some(milliseconds) = data.get("time_ms").and_then(Value::as_i64) {
        return Some(milliseconds);
    }
    let value = data.get("timestamp")?;
    if let Some(timestamp) = value.as_i64() {
        return Some(if timestamp.unsigned_abs() < 100_000_000_000 {
            timestamp.saturating_mul(1_000)
        } else {
            timestamp
        });
    }
    let timestamp = value.as_str()?;
    let nanos = OffsetDateTime::parse(timestamp, &Rfc3339)
        .ok()?
        .unix_timestamp_nanos();
    i64::try_from(nanos / 1_000_000).ok()
}
