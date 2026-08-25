use mutsuki_bot_protocol::{
    BotAccountRef, BotEvent, BotEventKind, BotExtMap, BotMessage, BotPlatform, BotTarget, BotUser,
    MessageSegment,
};
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::adapter::{qq_target_from_payload, upgrade_qq_cdn_https};
use crate::gateway::{GatewayFrame, dedup_key};

pub fn qq_gateway_frame_to_bot_event(
    account_id: &str,
    app_id: &str,
    frame: GatewayFrame,
) -> Result<BotEvent, String> {
    if frame.op != 0 {
        return Err(format!("expected_dispatch_op:{}", frame.op));
    }
    let event_type = frame.t.as_deref().unwrap_or("UNKNOWN");
    let data = &frame.d;
    let target = qq_target_from_payload(event_type, data);
    let actor = if event_type == "READY" {
        data.get("user").and_then(|user| qq_self_user(user, app_id))
    } else {
        qq_actor(data, app_id)
    };
    let message = qq_message(event_type, data, target.clone(), actor.clone());
    let mut ext = BotExtMap::new();
    ext.insert("qqbot.event_type".into(), Value::String(event_type.into()));
    ext.insert("qqbot.dedup_key".into(), Value::String(dedup_key(&frame)));
    if let Some(sequence) = frame.s {
        ext.insert("qqbot.sequence".into(), Value::from(sequence));
    }
    ext.insert(
        "qqbot.mentioned_bot".into(),
        Value::Bool(qq_mentioned_bot(account_id, event_type, data)),
    );
    if let Some(role) = qq_actor_role(data) {
        ext.insert("qqbot.actor_role".into(), Value::String(role.into()));
    }
    if let Some(thread_id) = data
        .get("thread_id")
        .or_else(|| data.get("topic_id"))
        .and_then(Value::as_str)
    {
        ext.insert("qqbot.thread_id".into(), Value::String(thread_id.into()));
    }
    if let Some(group_name) = qq_payload_group_name(event_type, data) {
        ext.insert("qqbot.group_name".into(), Value::String(group_name));
    }
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

#[must_use]
pub fn qq_bot_disconnected_event(account_id: &str) -> BotEvent {
    let mut ext = BotExtMap::new();
    ext.insert(
        "qqbot.event_type".into(),
        Value::String("DISCONNECTED".into()),
    );
    let now_ms = OffsetDateTime::now_utc().unix_timestamp() * 1_000;
    BotEvent {
        event_id: format!("qqbot:disconnected:{account_id}:{now_ms}"),
        platform: BotPlatform::QqBot,
        bot: BotAccountRef {
            account_id: account_id.into(),
            platform: BotPlatform::QqBot,
        },
        kind: BotEventKind::BotDisconnected,
        time_ms: now_ms,
        target: BotTarget::platform_specific("qqbot", "session", account_id),
        actor: None,
        message: None,
        raw: None,
        ext,
    }
}

fn qq_event_kind(event_type: &str) -> BotEventKind {
    match event_type {
        "GROUP_MESSAGE_CREATE"
        | "GROUP_AT_MESSAGE_CREATE"
        | "C2C_MESSAGE_CREATE"
        | "AT_MESSAGE_CREATE"
        | "MESSAGE_CREATE"
        | "DIRECT_MESSAGE_CREATE" => BotEventKind::MessageCreated,
        "MESSAGE_UPDATE" => BotEventKind::MessageUpdated,
        "MESSAGE_DELETE" | "PUBLIC_MESSAGE_DELETE" | "DIRECT_MESSAGE_DELETE" => {
            BotEventKind::MessageDeleted
        }
        "MESSAGE_REACTION_ADD" => BotEventKind::ReactionAdded,
        "MESSAGE_REACTION_REMOVE" => BotEventKind::ReactionRemoved,
        "GROUP_MEMBER_ADD" | "GUILD_MEMBER_ADD" | "FRIEND_ADD" => BotEventKind::MemberJoined,
        "GROUP_MEMBER_REMOVE" | "GUILD_MEMBER_REMOVE" | "FRIEND_DEL" => BotEventKind::MemberLeft,
        "READY" | "RESUMED" => BotEventKind::BotConnected,
        _ => BotEventKind::PlatformSpecific(event_type.into()),
    }
}

/// Maps `/users/@me` or READY `user` into a Bot self profile.
///
/// Missing avatars synthesize the QQ App CDN URL using `app_id` and the user id.
#[must_use]
pub fn qq_self_user(user: &Value, app_id: &str) -> Option<BotUser> {
    let (user_id, _) = qq_actor_id(user)?;
    Some(qq_user_fields(user, user_id, true, app_id))
}

/// Reads `group_name` from a QQ `GET /v2/groups/{group_openid}/info` body.
#[must_use]
pub fn qq_group_name_from_info(body: &Value) -> Option<String> {
    nonempty_json_string(body.get("group_name"))
}

fn qq_payload_group_name(event_type: &str, data: &Value) -> Option<String> {
    if !event_type.starts_with("GROUP") {
        return None;
    }
    nonempty_json_string(data.get("group_name")).or_else(|| nonempty_json_string(data.get("name")))
}

fn nonempty_json_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn qq_actor(data: &Value, app_id: &str) -> Option<BotUser> {
    let author = data.get("author").unwrap_or(data);
    let (user_id, from_openid) = qq_actor_id(author)?;
    Some(qq_user_fields(author, user_id, from_openid, app_id))
}

fn qq_user_fields(
    source: &Value,
    user_id: String,
    synthesize_qqapp_avatar: bool,
    app_id: &str,
) -> BotUser {
    let avatar = source
        .get("avatar")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(upgrade_qq_cdn_https)
        .or_else(|| {
            let app_id = app_id.trim();
            (synthesize_qqapp_avatar && !app_id.is_empty())
                .then(|| format!("https://q.qlogo.cn/qqapp/{app_id}/{user_id}/640"))
        });
    BotUser {
        user_id,
        display_name: source
            .get("username")
            .or_else(|| source.get("nick"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        avatar_url: avatar,
    }
}

fn qq_actor_id(author: &Value) -> Option<(String, bool)> {
    const OPENID_KEYS: [&str; 4] = [
        "member_openid",
        "group_member_openid",
        "user_openid",
        "openid",
    ];
    for key in OPENID_KEYS {
        if let Some(user_id) = author
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            return Some((user_id.to_owned(), true));
        }
    }
    author
        .get("user_id")
        .or_else(|| author.get("id"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(|user_id| (user_id.to_owned(), false))
}

fn encoded_role(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_ascii_lowercase)
        .or_else(|| value.as_u64().map(|rank| rank.to_string()))
}

fn qq_actor_role(data: &Value) -> Option<&'static str> {
    let mut rank = 0_u8;
    if let Some(roles) = data
        .pointer("/member/roles")
        .or_else(|| data.get("roles"))
        .and_then(Value::as_array)
    {
        for role in roles {
            if let Some(encoded) = encoded_role(role) {
                rank = rank.max(role_rank(&encoded));
            }
        }
    }
    for pointer in ["/author/member_role", "/member_role", "/author/role"] {
        if let Some(encoded) = data.pointer(pointer).and_then(encoded_role) {
            rank = rank.max(role_rank(&encoded));
        }
    }
    match rank {
        3 => Some("owner"),
        2 => Some("administrator"),
        1 => Some("member"),
        _ => None,
    }
}

fn role_rank(role: &str) -> u8 {
    match role {
        "4" | "owner" => 3,
        "2" | "5" | "admin" | "administrator" => 2,
        "1" | "member" => 1,
        _ => 0,
    }
}

fn qq_message(
    event_type: &str,
    data: &Value,
    target: mutsuki_bot_protocol::BotTarget,
    actor: Option<BotUser>,
) -> Option<BotMessage> {
    if !matches!(
        event_type,
        "GROUP_MESSAGE_CREATE"
            | "GROUP_AT_MESSAGE_CREATE"
            | "C2C_MESSAGE_CREATE"
            | "AT_MESSAGE_CREATE"
            | "MESSAGE_CREATE"
            | "DIRECT_MESSAGE_CREATE"
            | "MESSAGE_UPDATE"
            | "MESSAGE_DELETE"
            | "PUBLIC_MESSAGE_DELETE"
            | "DIRECT_MESSAGE_DELETE"
    ) {
        return None;
    }
    let content = message_content(event_type, data);
    let mut segments = content_segments(&content);
    extend_qq_rich_segments(&mut segments, data);
    let reply_to = data
        .get("message_reference")
        .and_then(|reference| reference.get("message_id"))
        .or_else(|| {
            data.get("referenced_message")
                .and_then(|message| message.get("id"))
        })
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(message_id) = reply_to.as_ref() {
        segments.push(MessageSegment::Reply {
            message_id: message_id.clone(),
        });
        segments.push(MessageSegment::Quote {
            message_id: message_id.clone(),
        });
    }
    Some(BotMessage {
        message_id: data.get("id").and_then(Value::as_str).map(str::to_owned),
        target,
        sender: actor,
        segments,
        reply_to,
        time_ms: event_time_ms(data),
        ext: BotExtMap::new(),
    })
}

fn content_segments(content: &str) -> Vec<MessageSegment> {
    let mut segments = Vec::new();
    let mut cursor = 0;
    while cursor < content.len() {
        match next_mention(&content[cursor..]) {
            None => {
                let rest = &content[cursor..];
                if !rest.is_empty() {
                    segments.push(MessageSegment::Text {
                        text: rest.to_owned(),
                    });
                }
                break;
            }
            Some((start, end, segment)) => {
                if start > 0 {
                    segments.push(MessageSegment::Text {
                        text: content[cursor..cursor + start].to_owned(),
                    });
                }
                segments.push(segment);
                cursor += end;
            }
        }
    }
    segments
}

fn next_mention(content: &str) -> Option<(usize, usize, MessageSegment)> {
    let mut best: Option<(usize, usize, MessageSegment)> = None;
    let consider = |best: &mut Option<(usize, usize, MessageSegment)>,
                    start: usize,
                    end: usize,
                    segment: MessageSegment| {
        if best.as_ref().is_none_or(|(current, _, _)| start < *current) {
            *best = Some((start, end, segment));
        }
    };

    let mut search = 0;
    while let Some(rel) = content[search..].find('<') {
        let start = search + rel;
        let rest = &content[start..];
        if let Some(mention) = parse_angle_mention(rest) {
            consider(&mut best, start, start + mention.0, mention.1);
            break;
        }
        if let Some(face) = parse_qq_face(rest) {
            consider(&mut best, start, start + face.0, face.1);
            break;
        }
        search = start + 1;
    }

    for (token, segment) in [
        ("@all", MessageSegment::MentionAll),
        ("@everyone", MessageSegment::MentionAll),
    ] {
        if let Some(start) = content.find(token) {
            let end = start + token.len();
            if mention_token_boundary(content, start, end) {
                consider(&mut best, start, end, segment);
            }
        }
    }
    best
}

fn parse_angle_mention(rest: &str) -> Option<(usize, MessageSegment)> {
    if let Some(body) = rest.strip_prefix("<qqbot-at-user ") {
        let id = body
            .strip_prefix("id=\"")
            .and_then(|value| value.split('"').next())
            .filter(|value| !value.is_empty())?;
        let close = body
            .find("/>")
            .map(|index| "<qqbot-at-user ".len() + index + 2)?;
        return Some((
            close,
            MessageSegment::MentionUser {
                user_id: id.to_owned(),
            },
        ));
    }
    let body = rest.strip_prefix("<@")?;
    let body = body.strip_prefix('!').unwrap_or(body);
    let (user_id, tail) = body.split_once('>')?;
    if user_id.is_empty() || user_id.chars().any(char::is_whitespace) {
        return None;
    }
    let consumed = rest.len() - tail.len();
    if user_id.eq_ignore_ascii_case("all") || user_id.eq_ignore_ascii_case("everyone") {
        Some((consumed, MessageSegment::MentionAll))
    } else {
        Some((
            consumed,
            MessageSegment::MentionUser {
                user_id: user_id.to_owned(),
            },
        ))
    }
}

fn mention_token_boundary(content: &str, start: usize, end: usize) -> bool {
    let before_ok = content[..start]
        .chars()
        .next_back()
        .is_none_or(|ch| !ch.is_ascii_alphanumeric());
    let after_ok = content[end..]
        .chars()
        .next()
        .is_none_or(|ch| !ch.is_ascii_alphanumeric());
    before_ok && after_ok
}

fn extend_qq_rich_segments(segments: &mut Vec<MessageSegment>, data: &Value) {
    if let Some(attachments) = data.get("attachments").and_then(Value::as_array) {
        for attachment in attachments {
            let Some(url) = attachment.get("url").and_then(Value::as_str) else {
                continue;
            };
            let content_type = attachment
                .get("content_type")
                .or_else(|| attachment.get("contentType"))
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream");
            let filename = attachment.get("filename").and_then(Value::as_str);
            segments.push(MessageSegment::PlatformSpecific {
                platform: "qqbot".into(),
                kind: "attachment".into(),
                payload: json!({
                    "url": upgrade_qq_cdn_https(url),
                    "content_type": content_type,
                    "filename": filename,
                }),
            });
        }
    }
    push_named_payload(segments, data, "ark");
    push_inbound_markdown(segments, data);
    push_named_payload(segments, data, "embed");
    if let Some(embeds) = data.get("embeds").and_then(Value::as_array) {
        for embed in embeds {
            segments.push(platform_payload("embed", embed.clone()));
        }
    }
    push_named_payload(segments, data, "keyboard");
}

fn push_inbound_markdown(segments: &mut Vec<MessageSegment>, data: &Value) {
    let Some(payload) = data.get("markdown") else {
        return;
    };
    if let Some(content) = payload
        .get("content")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        segments.push(MessageSegment::markdown(content));
        return;
    }
    segments.push(platform_payload("markdown", payload.clone()));
}

fn push_named_payload(segments: &mut Vec<MessageSegment>, data: &Value, kind: &str) {
    if let Some(payload) = data.get(kind) {
        segments.push(platform_payload(kind, payload.clone()));
    }
}

fn platform_payload(kind: &str, payload: Value) -> MessageSegment {
    MessageSegment::PlatformSpecific {
        platform: "qqbot".into(),
        kind: kind.into(),
        payload,
    }
}

/// True when this payload @-mentions the current bot, not `@all` / `MentionAll`.
///
/// Official `GROUP_AT_MESSAGE_CREATE` is delivered only when the user @-mentions
/// the bot (`GROUP_AND_C2C` AT mode). QQ strips that mention from `content` and
/// omits the bot from `mentions`, so the event type is the @-bot signal. Full
/// group traffic uses the mapped `GROUP_MESSAGE_CREATE` path instead; there
/// `@all` and other users are not treated as mentioning this bot.
fn qq_mentioned_bot(account_id: &str, event_type: &str, data: &Value) -> bool {
    if event_type == "GROUP_AT_MESSAGE_CREATE" {
        return true;
    }
    mentions_include_this_bot(account_id, data) || content_mentions_this_bot(account_id, data)
}

fn mentions_include_this_bot(account_id: &str, data: &Value) -> bool {
    data.get("mentions")
        .and_then(Value::as_array)
        .is_some_and(|mentions| {
            mentions
                .iter()
                .any(|mention| mention_targets_this_bot(account_id, mention))
        })
}

fn mention_targets_this_bot(account_id: &str, mention: &Value) -> bool {
    if mention
        .get("is_you")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }
    if account_id.is_empty() {
        return false;
    }
    ["id", "user_id", "user_openid"]
        .iter()
        .any(|key| mention.get(*key).and_then(Value::as_str) == Some(account_id))
}

fn content_mentions_this_bot(account_id: &str, data: &Value) -> bool {
    if account_id.is_empty() {
        return false;
    }
    let raw = data
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let decoded = html_escape::decode_html_entities(raw);
    content_segments(&decoded).iter().any(|segment| {
        matches!(
            segment,
            MessageSegment::MentionUser { user_id } if user_id == account_id
        )
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

fn parse_qq_face(rest: &str) -> Option<(usize, MessageSegment)> {
    let body = rest.strip_prefix("<faceType=")?;
    let end = body.find('>')?;
    let inner = &body[..end];
    let mut parts = inner.split(',');
    let face_type = parts.next()?.trim();
    if face_type.is_empty() {
        return None;
    }
    let face_id = parts
        .find_map(|part| {
            part.trim()
                .strip_prefix("faceId=")
                .map(|value| value.trim_matches('"'))
        })
        .unwrap_or("");
    Some((
        "<faceType=".len() + end + 1,
        MessageSegment::PlatformSpecific {
            platform: "qqbot".into(),
            kind: "face".into(),
            payload: json!({
                "face_type": face_type,
                "face_id": face_id,
            }),
        },
    ))
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
