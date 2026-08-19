use mutsuki_bot_protocol::MessageSegment;
use serde_json::{Value, json};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SegmentMapError {
    #[error("message segment is not supported by QQBot standard send: {0}")]
    UnsupportedSegment(&'static str),
    #[error("QQ markdown cannot mix with text, mention or ark")]
    MarkdownMixedWithText,
    #[error("QQ message/send allows one markdown payload")]
    MultipleMarkdown,
    #[error("QQ message/send allows one keyboard payload")]
    MultipleKeyboard,
    #[error("QQ keyboard can only attach to markdown")]
    KeyboardWithoutMarkdown,
    #[error("QQ markdown content must not be empty")]
    EmptyMarkdown,
    #[error("QQ custom markdown must use the markdown segment")]
    CustomMarkdownMustUseSegment,
}

pub fn qq_message_body_from_segments(
    segments: &[MessageSegment],
) -> Result<Value, SegmentMapError> {
    let mut content = String::new();
    let mut has_text = false;
    let mut markdown: Option<Value> = None;
    let mut keyboard: Option<Value> = None;

    for segment in segments {
        match segment {
            MessageSegment::Text { text } => {
                reject_text_markdown_mix(markdown.is_some() || keyboard.is_some())?;
                has_text = true;
                content.push_str(text);
            }
            MessageSegment::MentionUser { user_id } => {
                reject_text_markdown_mix(markdown.is_some() || keyboard.is_some())?;
                has_text = true;
                content.push_str(&format!("<@{user_id}>"));
            }
            MessageSegment::MentionAll => {
                reject_text_markdown_mix(markdown.is_some() || keyboard.is_some())?;
                has_text = true;
                content.push_str("@all");
            }
            MessageSegment::Markdown { content: body } => {
                reject_text_markdown_mix(has_text)?;
                if markdown.is_some() {
                    return Err(SegmentMapError::MultipleMarkdown);
                }
                if body.trim().is_empty() {
                    return Err(SegmentMapError::EmptyMarkdown);
                }
                markdown = Some(json!({ "content": body }));
            }
            MessageSegment::PlatformSpecific {
                platform,
                kind,
                payload,
            } if platform == "qqbot" && kind == "keyboard" => {
                reject_text_markdown_mix(has_text)?;
                if keyboard.is_some() {
                    return Err(SegmentMapError::MultipleKeyboard);
                }
                keyboard = Some(payload.clone());
            }
            MessageSegment::PlatformSpecific {
                platform,
                kind,
                payload,
            } if platform == "qqbot" && kind == "markdown" => {
                reject_text_markdown_mix(has_text)?;
                if payload.get("content").is_some() {
                    return Err(SegmentMapError::CustomMarkdownMustUseSegment);
                }
                let template_id = payload
                    .get("custom_template_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                if template_id.is_none() {
                    return Err(SegmentMapError::UnsupportedSegment("platform_specific"));
                }
                if markdown.is_some() {
                    return Err(SegmentMapError::MultipleMarkdown);
                }
                markdown = Some(payload.clone());
            }
            unsupported => {
                return Err(SegmentMapError::UnsupportedSegment(segment_name(
                    unsupported,
                )));
            }
        }
    }

    if let Some(markdown) = markdown {
        let mut body = json!({
            "msg_type": 2,
            "markdown": markdown,
        });
        if let Some(keyboard) = keyboard {
            body["keyboard"] = keyboard;
        }
        return Ok(body);
    }
    if keyboard.is_some() {
        return Err(SegmentMapError::KeyboardWithoutMarkdown);
    }
    Ok(json!({
        "msg_type": 0,
        "content": content,
    }))
}

fn reject_text_markdown_mix(conflict: bool) -> Result<(), SegmentMapError> {
    if conflict {
        Err(SegmentMapError::MarkdownMixedWithText)
    } else {
        Ok(())
    }
}

fn segment_name(segment: &MessageSegment) -> &'static str {
    match segment {
        MessageSegment::Text { .. } => "text",
        MessageSegment::MentionUser { .. } => "mention_user",
        MessageSegment::MentionAll => "mention_all",
        MessageSegment::Image { .. } => "image",
        MessageSegment::File { .. } => "file",
        MessageSegment::Audio { .. } => "audio",
        MessageSegment::Video { .. } => "video",
        MessageSegment::Reply { .. } => "reply",
        MessageSegment::Quote { .. } => "quote",
        MessageSegment::Markdown { .. } => "markdown",
        MessageSegment::PlatformSpecific { .. } => "platform_specific",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_markdown_omits_content_field() {
        let body = qq_message_body_from_segments(&[MessageSegment::markdown("**hi**")]).unwrap();
        assert_eq!(body["msg_type"], 2);
        assert_eq!(body["markdown"]["content"], "**hi**");
        assert!(body.get("content").is_none());
    }

    #[test]
    fn markdown_and_keyboard_share_one_payload() {
        let body = qq_message_body_from_segments(&[
            MessageSegment::markdown("# title"),
            MessageSegment::platform_specific(
                "qqbot",
                "keyboard",
                json!({"content": {"rows": []}}),
            ),
        ])
        .unwrap();
        assert_eq!(body["msg_type"], 2);
        assert_eq!(body["markdown"]["content"], "# title");
        assert_eq!(body["keyboard"]["content"]["rows"], json!([]));
        assert!(body.get("content").is_none());
    }

    #[test]
    fn template_markdown_uses_platform_specific_payload() {
        let body = qq_message_body_from_segments(&[MessageSegment::platform_specific(
            "qqbot",
            "markdown",
            json!({
                "custom_template_id": "tpl-1",
                "params": [{"key": "title", "values": ["hi"]}]
            }),
        )])
        .unwrap();
        assert_eq!(body["msg_type"], 2);
        assert_eq!(body["markdown"]["custom_template_id"], "tpl-1");
    }

    #[test]
    fn rejects_markdown_mixed_with_text() {
        let error = qq_message_body_from_segments(&[
            MessageSegment::text("hi"),
            MessageSegment::markdown("# no"),
        ])
        .unwrap_err();
        assert_eq!(error, SegmentMapError::MarkdownMixedWithText);
    }

    #[test]
    fn rejects_keyboard_without_markdown() {
        let error = qq_message_body_from_segments(&[MessageSegment::platform_specific(
            "qqbot",
            "keyboard",
            json!({"id": "1"}),
        )])
        .unwrap_err();
        assert_eq!(error, SegmentMapError::KeyboardWithoutMarkdown);
    }

    #[test]
    fn rejects_custom_markdown_as_platform_specific() {
        let error = qq_message_body_from_segments(&[MessageSegment::platform_specific(
            "qqbot",
            "markdown",
            json!({"content": "**hi**"}),
        )])
        .unwrap_err();
        assert_eq!(error, SegmentMapError::CustomMarkdownMustUseSegment);
    }

    #[test]
    fn rejects_raw_message_body() {
        let error = qq_message_body_from_segments(&[MessageSegment::platform_specific(
            "qqbot",
            "message_body",
            json!({"msg_type": 0, "content": "raw"}),
        )])
        .unwrap_err();
        assert_eq!(
            error,
            SegmentMapError::UnsupportedSegment("platform_specific")
        );
    }
}
