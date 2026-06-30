use mutsuki_bot_protocol::MessageSegment;
use serde_json::{Value, json};

pub fn qq_message_body_from_segments(segments: &[MessageSegment]) -> Value {
    let mut content = String::new();
    for segment in segments {
        match segment {
            MessageSegment::Text { text } => content.push_str(text),
            MessageSegment::MentionUser { user_id } => {
                content.push_str(&format!("<@{user_id}>"));
            }
            MessageSegment::MentionAll => content.push_str("@all"),
            MessageSegment::PlatformSpecific {
                platform,
                kind,
                payload,
            } if platform == "qqbot" && kind == "message_body" => return payload.clone(),
            _ => {}
        }
    }
    json!({
        "msg_type": 0,
        "content": content,
    })
}
