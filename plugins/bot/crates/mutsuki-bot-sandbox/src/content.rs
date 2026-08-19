use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use mutsuki_bot_protocol::MessageSegment;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::types::{
    SANDBOX_MAX_MEDIA_BYTES, SANDBOX_MAX_MEDIA_ITEMS, SANDBOX_MAX_STICKER_ITEMS, SandboxAsset,
    SandboxFace, SandboxSticker, SandboxUserView,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxRefKind {
    Mention,
    MentionAll,
    Img,
    File,
    Audio,
    Video,
    Sticker,
    Emoji,
    Ark,
    Markdown,
    Keyboard,
    Embed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SandboxContentRef {
    #[serde(rename = "t")]
    pub kind: SandboxRefKind,
    pub at: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(rename = "p", default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}

impl SandboxRefKind {
    pub(crate) fn is_media(self) -> bool {
        matches!(self, Self::Img | Self::File | Self::Audio | Self::Video)
    }
}

impl SandboxContentRef {
    fn at(kind: SandboxRefKind, text: &str) -> Self {
        Self {
            kind,
            at: char_len(text),
            h: None,
            id: None,
            name: None,
            mime: None,
            url: None,
            payload: None,
        }
    }
}

#[must_use]
pub fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::from("sha256:");
    for byte in digest {
        write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out
}

fn is_content_hash(value: &str) -> bool {
    let rest = value.strip_prefix("sha256:").unwrap_or("");
    rest.len() == 64 && rest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[must_use]
pub fn normalize_segments(
    segments: &[MessageSegment],
    users: &[SandboxUserView],
    assets: &mut HashMap<String, SandboxAsset>,
    now: u64,
) -> (String, Vec<SandboxContentRef>) {
    let mut text = String::new();
    let mut refs = Vec::new();
    let mut pending = Vec::new();
    for segment in segments {
        match segment {
            MessageSegment::PlatformSpecific {
                platform,
                kind,
                payload,
            } if platform == "qqbot" && kind == "attachment" => {
                pending.push(attachment_meta(payload));
            }
            MessageSegment::Image { resource }
            | MessageSegment::Audio { resource }
            | MessageSegment::Video { resource }
            | MessageSegment::File { resource, .. } => {
                let kind = media_kind(segment);
                let attachment = take_front(&mut pending);
                push_media(
                    &text,
                    &mut refs,
                    assets,
                    now,
                    kind,
                    resource.content_hash.as_deref(),
                    attachment.as_ref().and_then(|item| item.url.clone()),
                    attachment
                        .as_ref()
                        .and_then(|item| item.mime.clone())
                        .or_else(|| mime_for(kind)),
                    file_name(segment).or_else(|| attachment.and_then(|item| item.name)),
                );
            }
            other => {
                flush_attachments(&text, &mut refs, &mut pending);
                push_inline(&mut text, &mut refs, assets, users, now, other);
            }
        }
    }
    flush_attachments(&text, &mut refs, &mut pending);
    (text, refs)
}

#[must_use]
pub fn hydrate_segments(
    text: &str,
    refs: &[SandboxContentRef],
    reply_to: Option<&str>,
) -> Vec<MessageSegment> {
    let chars = text.chars().collect::<Vec<_>>();
    let mut cursor = 0usize;
    let mut segments = Vec::new();
    if let Some(message_id) = reply_to.filter(|value| !value.is_empty()) {
        segments.push(MessageSegment::Quote {
            message_id: message_id.to_owned(),
        });
    }
    for item in refs {
        let at = span_at(item.at, chars.len());
        if at > cursor {
            segments.push(MessageSegment::text(
                chars[cursor..at].iter().collect::<String>(),
            ));
        }
        match item.kind {
            SandboxRefKind::Mention => {
                if let Some(user_id) = &item.id {
                    segments.push(MessageSegment::MentionUser {
                        user_id: user_id.clone(),
                    });
                }
                cursor = at + mention_span(item);
            }
            SandboxRefKind::MentionAll => {
                segments.push(MessageSegment::MentionAll);
                cursor = at + mention_span(item);
            }
            SandboxRefKind::Sticker => {
                segments.push(hydrate_sticker(item));
                cursor = at;
            }
            SandboxRefKind::Emoji => {
                segments.push(hydrate_emoji(item));
                cursor = at;
            }
            SandboxRefKind::Markdown => {
                segments.push(hydrate_markdown(item));
                cursor = at;
            }
            SandboxRefKind::Ark | SandboxRefKind::Keyboard | SandboxRefKind::Embed => {
                segments.push(MessageSegment::PlatformSpecific {
                    platform: "qqbot".into(),
                    kind: payload_kind_name(item.kind).into(),
                    payload: item.payload.clone().unwrap_or(Value::Null),
                });
                cursor = at;
            }
            SandboxRefKind::Img
            | SandboxRefKind::File
            | SandboxRefKind::Audio
            | SandboxRefKind::Video => {
                segments.push(hydrate_media(item));
                cursor = at;
            }
        }
    }
    if cursor < chars.len() {
        segments.push(MessageSegment::text(
            chars[cursor..].iter().collect::<String>(),
        ));
    }
    segments.retain(|segment| match segment {
        MessageSegment::Text { text } => !text.is_empty(),
        _ => true,
    });
    segments
}

#[must_use]
pub fn preview_content(text: &str, refs: &[SandboxContentRef]) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let mut cursor = 0usize;
    let mut parts = Vec::new();
    for item in refs {
        let at = span_at(item.at, chars.len());
        if at > cursor {
            parts.push(chars[cursor..at].iter().collect::<String>());
        }
        if matches!(
            item.kind,
            SandboxRefKind::Mention | SandboxRefKind::MentionAll
        ) {
            cursor = at + mention_span(item);
            if at < cursor {
                parts.push(
                    chars[at..cursor.min(chars.len())]
                        .iter()
                        .collect::<String>(),
                );
            }
        } else {
            parts.push(ref_label(item));
            cursor = at;
        }
    }
    if cursor < chars.len() {
        parts.push(chars[cursor..].iter().collect::<String>());
    }
    parts.join("")
}

pub(crate) fn gc_assets(assets: &mut HashMap<String, SandboxAsset>, referenced: &HashSet<String>) {
    let mut unreferenced = assets
        .values()
        .filter(|asset| !referenced.contains(&asset.content_hash))
        .cloned()
        .collect::<Vec<_>>();
    unreferenced.sort_by(|left, right| {
        right
            .created_at_unix_ms
            .cmp(&left.created_at_unix_ms)
            .then_with(|| left.content_hash.cmp(&right.content_hash))
    });
    let mut kept_bytes = 0usize;
    let mut keep_drafts = HashSet::new();
    for asset in unreferenced {
        if keep_drafts.len() >= SANDBOX_MAX_MEDIA_ITEMS {
            break;
        }
        let next_bytes = kept_bytes.saturating_add(asset.bytes.len());
        if !asset.bytes.is_empty() && next_bytes > SANDBOX_MAX_MEDIA_BYTES {
            continue;
        }
        keep_drafts.insert(asset.content_hash.clone());
        kept_bytes = next_bytes;
    }
    assets.retain(|hash, _| referenced.contains(hash) || keep_drafts.contains(hash));
}

pub(crate) fn gc_stickers(stickers: &mut HashMap<String, SandboxSticker>) {
    if stickers.len() <= SANDBOX_MAX_STICKER_ITEMS {
        return;
    }
    let mut items = stickers.values().cloned().collect::<Vec<_>>();
    items.sort_by(|left, right| {
        left.created_at_unix_ms
            .cmp(&right.created_at_unix_ms)
            .then_with(|| left.content_hash.cmp(&right.content_hash))
    });
    let drop_count = stickers.len() - SANDBOX_MAX_STICKER_ITEMS;
    for item in items.into_iter().take(drop_count) {
        stickers.remove(&item.content_hash);
    }
}

pub(crate) fn record_faces(
    faces: &mut HashMap<String, SandboxFace>,
    refs: &[SandboxContentRef],
    now: u64,
) {
    for item in refs {
        if item.kind != SandboxRefKind::Emoji {
            continue;
        }
        let Some(id) = item.id.as_deref() else {
            continue;
        };
        let Some((face_type, face_id)) = parse_face_id(id) else {
            continue;
        };
        let face_key = format!("qq:{face_type}:{face_id}");
        match faces.get_mut(&face_key) {
            Some(existing) => {
                existing.last_seen_unix_ms = existing.last_seen_unix_ms.max(now);
            }
            None => {
                faces.insert(
                    face_key.clone(),
                    SandboxFace {
                        face_key,
                        face_type,
                        face_id,
                        last_seen_unix_ms: now,
                    },
                );
            }
        }
    }
}

#[must_use]
pub fn parse_face_id(id: &str) -> Option<(String, String)> {
    let rest = id.strip_prefix("qq:").unwrap_or(id);
    let (face_type, face_id) = rest.split_once(':')?;
    if face_type.is_empty() && face_id.is_empty() {
        return None;
    }
    Some((face_type.to_owned(), face_id.to_owned()))
}

pub fn remap_sandbox_media_ids(segments: &mut [MessageSegment], aliases: &HashMap<String, String>) {
    for segment in segments {
        let MessageSegment::PlatformSpecific {
            platform,
            kind,
            payload,
        } = segment
        else {
            continue;
        };
        if platform != "sandbox" || kind != "media" {
            continue;
        }
        let Some(media_id) = payload.get("media_id").and_then(Value::as_str) else {
            continue;
        };
        if let Some(hash) = aliases.get(media_id) {
            payload["media_id"] = Value::String(hash.clone());
        }
    }
}

fn push_inline(
    text: &mut String,
    refs: &mut Vec<SandboxContentRef>,
    assets: &mut HashMap<String, SandboxAsset>,
    users: &[SandboxUserView],
    now: u64,
    segment: &MessageSegment,
) {
    match segment {
        MessageSegment::Text { text: value } => text.push_str(value),
        MessageSegment::MentionUser { user_id } => {
            let name = users
                .iter()
                .find(|user| user.user_id == *user_id)
                .map(|user| user.display_name.as_str())
                .filter(|name| !name.is_empty())
                .unwrap_or(user_id);
            let mut item = SandboxContentRef::at(SandboxRefKind::Mention, text);
            item.id = Some(user_id.clone());
            item.name = Some(name.to_owned());
            text.push('@');
            text.push_str(name);
            refs.push(item);
        }
        MessageSegment::MentionAll => {
            let mut item = SandboxContentRef::at(SandboxRefKind::MentionAll, text);
            item.name = Some("全体成员".into());
            text.push_str("@全体成员");
            refs.push(item);
        }
        MessageSegment::Reply { .. }
        | MessageSegment::Quote { .. }
        | MessageSegment::Image { .. }
        | MessageSegment::File { .. }
        | MessageSegment::Audio { .. }
        | MessageSegment::Video { .. } => {}
        MessageSegment::Markdown { content } => {
            let mut item = SandboxContentRef::at(SandboxRefKind::Markdown, text);
            item.payload = Some(json!({ "content": content }));
            refs.push(item);
        }
        MessageSegment::PlatformSpecific {
            platform,
            kind,
            payload,
        } if platform == "sandbox" && kind == "sticker" => {
            let mut item = SandboxContentRef::at(SandboxRefKind::Sticker, text);
            item.h = payload
                .get("sticker_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| is_content_hash(value))
                .map(str::to_owned);
            item.mime = payload
                .get("mime")
                .and_then(Value::as_str)
                .map(str::to_owned);
            item.name = payload
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned);
            refs.push(item);
        }
        MessageSegment::PlatformSpecific {
            platform,
            kind,
            payload,
        } if platform == "sandbox" && kind == "media" => {
            let mime = payload
                .get("mime")
                .and_then(Value::as_str)
                .map(str::to_owned);
            push_media(
                text,
                refs,
                assets,
                now,
                kind_from_mime(mime.as_deref()),
                payload.get("media_id").and_then(Value::as_str),
                None,
                mime,
                payload
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            );
        }
        MessageSegment::PlatformSpecific {
            platform,
            kind,
            payload,
        } if platform == "qqbot" && kind == "face" => {
            let mut item = SandboxContentRef::at(SandboxRefKind::Emoji, text);
            item.id = Some(format!(
                "qq:{}:{}",
                payload_text(payload, "face_type"),
                payload_text(payload, "face_id")
            ));
            refs.push(item);
        }
        MessageSegment::PlatformSpecific { kind, payload, .. }
            if matches!(kind.as_str(), "ark" | "embed" | "markdown" | "keyboard") =>
        {
            let mut item = SandboxContentRef::at(payload_kind(kind), text);
            item.payload = Some(payload.clone());
            refs.push(item);
        }
        MessageSegment::PlatformSpecific { kind, payload, .. } => {
            let mut item = SandboxContentRef::at(SandboxRefKind::Embed, text);
            item.id = Some(kind.clone());
            item.payload = Some(payload.clone());
            refs.push(item);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_media(
    text: &str,
    refs: &mut Vec<SandboxContentRef>,
    assets: &mut HashMap<String, SandboxAsset>,
    now: u64,
    kind: SandboxRefKind,
    hash: Option<&str>,
    url: Option<String>,
    mime: Option<String>,
    name: Option<String>,
) {
    let hash = hash
        .map(str::trim)
        .filter(|value| is_content_hash(value))
        .map(str::to_owned);
    if let Some(hash) = hash.as_ref() {
        upsert_asset(
            assets,
            SandboxAsset {
                content_hash: hash.clone(),
                kind: asset_kind(kind).into(),
                mime: mime.clone().unwrap_or_default(),
                name: name.clone().unwrap_or_default(),
                bytes: Vec::new(),
                url: url.clone(),
                created_at_unix_ms: now,
            },
        );
    }
    let mut item = SandboxContentRef::at(kind, text);
    item.h = hash;
    item.name = name;
    item.mime = mime;
    item.url = url;
    refs.push(item);
}

fn flush_attachments(
    text: &str,
    refs: &mut Vec<SandboxContentRef>,
    pending: &mut Vec<AttachmentMeta>,
) {
    for item in pending.drain(..) {
        let mut content = SandboxContentRef::at(kind_from_mime(item.mime.as_deref()), text);
        content.name = item.name;
        content.mime = item.mime;
        content.url = item.url;
        refs.push(content);
    }
}

fn take_front(pending: &mut Vec<AttachmentMeta>) -> Option<AttachmentMeta> {
    pending.drain(..1).next()
}

fn hydrate_media(item: &SandboxContentRef) -> MessageSegment {
    if let Some(hash) = &item.h {
        return MessageSegment::PlatformSpecific {
            platform: "sandbox".into(),
            kind: "media".into(),
            payload: serde_json::json!({
                "media_id": hash,
                "mime": item.mime.clone().unwrap_or_else(|| mime_for(item.kind).unwrap_or_default()),
                "name": item.name.clone().unwrap_or_default(),
            }),
        };
    }
    MessageSegment::PlatformSpecific {
        platform: "qqbot".into(),
        kind: "attachment".into(),
        payload: serde_json::json!({
            "url": item.url,
            "content_type": item.mime,
            "filename": item.name,
        }),
    }
}

fn hydrate_sticker(item: &SandboxContentRef) -> MessageSegment {
    MessageSegment::PlatformSpecific {
        platform: "sandbox".into(),
        kind: "sticker".into(),
        payload: serde_json::json!({
            "sticker_id": item.h,
            "mime": item.mime.clone().unwrap_or_else(|| "image/*".into()),
            "name": item.name.clone().unwrap_or_default(),
        }),
    }
}

fn hydrate_emoji(item: &SandboxContentRef) -> MessageSegment {
    let id = item.id.as_deref().unwrap_or_default();
    let rest = id.strip_prefix("qq:").unwrap_or(id);
    let (face_type, face_id) = rest.split_once(':').unwrap_or((rest, ""));
    MessageSegment::PlatformSpecific {
        platform: "qqbot".into(),
        kind: "face".into(),
        payload: serde_json::json!({
            "face_type": face_type,
            "face_id": face_id,
        }),
    }
}

pub(crate) fn upsert_asset(
    assets: &mut HashMap<String, SandboxAsset>,
    incoming: SandboxAsset,
) -> String {
    let hash = incoming.content_hash.clone();
    match assets.get_mut(&hash) {
        Some(existing) => {
            if let Some(url) = incoming.url {
                existing.url = Some(url);
            }
            if existing.bytes.is_empty() && !incoming.bytes.is_empty() {
                existing.bytes = incoming.bytes;
                if !incoming.mime.is_empty() {
                    incoming.mime.clone_into(&mut existing.mime);
                }
                if !incoming.name.is_empty() {
                    incoming.name.clone_into(&mut existing.name);
                }
            }
        }
        None => {
            assets.insert(hash.clone(), incoming);
        }
    }
    hash
}

fn mention_span(item: &SandboxContentRef) -> usize {
    match item.kind {
        SandboxRefKind::Mention => 1 + item.name.as_deref().unwrap_or_default().chars().count(),
        SandboxRefKind::MentionAll => "@全体成员".chars().count(),
        _ => 0,
    }
}

fn span_at(at: u32, len: usize) -> usize {
    usize::try_from(at).unwrap_or(usize::MAX).min(len)
}

fn char_len(text: &str) -> u32 {
    u32::try_from(text.chars().count()).unwrap_or(u32::MAX)
}

fn media_kind(segment: &MessageSegment) -> SandboxRefKind {
    match segment {
        MessageSegment::Image { .. } => SandboxRefKind::Img,
        MessageSegment::Audio { .. } => SandboxRefKind::Audio,
        MessageSegment::Video { .. } => SandboxRefKind::Video,
        _ => SandboxRefKind::File,
    }
}

fn file_name(segment: &MessageSegment) -> Option<String> {
    match segment {
        MessageSegment::File { name, .. } => name.clone(),
        _ => None,
    }
}

fn mime_for(kind: SandboxRefKind) -> Option<String> {
    Some(
        match kind {
            SandboxRefKind::Img => "image/*",
            SandboxRefKind::Audio => "audio/*",
            SandboxRefKind::Video => "video/*",
            SandboxRefKind::File => "application/octet-stream",
            _ => return None,
        }
        .into(),
    )
}

fn kind_from_mime(mime: Option<&str>) -> SandboxRefKind {
    let mime = mime.unwrap_or_default();
    if mime.starts_with("image/") {
        SandboxRefKind::Img
    } else if mime.starts_with("audio/") {
        SandboxRefKind::Audio
    } else if mime.starts_with("video/") {
        SandboxRefKind::Video
    } else {
        SandboxRefKind::File
    }
}

fn asset_kind(kind: SandboxRefKind) -> &'static str {
    match kind {
        SandboxRefKind::Img => "image",
        SandboxRefKind::Audio => "audio",
        SandboxRefKind::Video => "video",
        SandboxRefKind::File => "file",
        _ => "embed",
    }
}

fn payload_kind(kind: &str) -> SandboxRefKind {
    match kind {
        "ark" => SandboxRefKind::Ark,
        "markdown" => SandboxRefKind::Markdown,
        "keyboard" => SandboxRefKind::Keyboard,
        _ => SandboxRefKind::Embed,
    }
}

fn hydrate_markdown(item: &SandboxContentRef) -> MessageSegment {
    let payload = item.payload.clone().unwrap_or(Value::Null);
    if let Some(content) = payload
        .get("content")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        MessageSegment::markdown(content)
    } else {
        MessageSegment::platform_specific("qqbot", "markdown", payload)
    }
}

fn payload_kind_name(kind: SandboxRefKind) -> &'static str {
    match kind {
        SandboxRefKind::Ark => "ark",
        SandboxRefKind::Markdown => "markdown",
        SandboxRefKind::Keyboard => "keyboard",
        _ => "embed",
    }
}

fn ref_label(item: &SandboxContentRef) -> String {
    match item.kind {
        SandboxRefKind::Img => "[图片]".into(),
        SandboxRefKind::File => format!("[{}]", item.name.as_deref().unwrap_or("文件")),
        SandboxRefKind::Audio => "[语音]".into(),
        SandboxRefKind::Video => "[视频]".into(),
        SandboxRefKind::Sticker => "[表情包]".into(),
        SandboxRefKind::Emoji => "[表情]".into(),
        SandboxRefKind::Ark | SandboxRefKind::Embed => "[小卡片]".into(),
        SandboxRefKind::Markdown => "[Markdown]".into(),
        SandboxRefKind::Keyboard => "[按钮]".into(),
        SandboxRefKind::Mention | SandboxRefKind::MentionAll => String::new(),
    }
}

fn payload_text(payload: &Value, key: &str) -> String {
    match payload.get(key) {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        _ => String::new(),
    }
}

#[derive(Clone)]
struct AttachmentMeta {
    url: Option<String>,
    mime: Option<String>,
    name: Option<String>,
}

fn attachment_meta(payload: &Value) -> AttachmentMeta {
    AttachmentMeta {
        url: payload
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_owned),
        mime: payload
            .get("content_type")
            .or_else(|| payload.get("mime"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        name: payload
            .get("filename")
            .or_else(|| payload.get("name"))
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SandboxUserView;

    fn user(name: &str) -> SandboxUserView {
        SandboxUserView {
            user_id: format!("sandbox:{name}"),
            display_name: name.into(),
            avatar_url: None,
            last_seen_unix_ms: 0,
            message_count: 0,
        }
    }

    #[test]
    fn normalize_then_hydrate_roundtrips_mentions_and_media_hash() {
        let users = [user("Alice")];
        let mut assets = HashMap::new();
        let hash = hash_bytes(b"png");
        assets.insert(
            hash.clone(),
            SandboxAsset {
                content_hash: hash.clone(),
                kind: "image".into(),
                mime: "image/png".into(),
                name: "pic.png".into(),
                bytes: b"png".to_vec(),
                url: None,
                created_at_unix_ms: 1,
            },
        );
        let segments = vec![
            MessageSegment::text("hi "),
            MessageSegment::MentionUser {
                user_id: "sandbox:Alice".into(),
            },
            MessageSegment::PlatformSpecific {
                platform: "sandbox".into(),
                kind: "media".into(),
                payload: serde_json::json!({
                    "media_id": hash,
                    "mime": "image/png",
                    "name": "pic.png"
                }),
            },
        ];
        let (text, refs) = normalize_segments(&segments, &users, &mut assets, 1);
        assert_eq!(text, "hi @Alice");
        assert_eq!(assets.len(), 1);
        let hydrated = hydrate_segments(&text, &refs, None);
        assert!(hydrated.iter().any(|segment| matches!(
            segment,
            MessageSegment::MentionUser { user_id } if user_id == "sandbox:Alice"
        )));
        assert!(hydrated.iter().any(|segment| matches!(
            segment,
            MessageSegment::PlatformSpecific { kind, payload, .. }
                if kind == "media" && payload.get("media_id").and_then(Value::as_str) == Some(hash.as_str())
        )));
    }

    #[test]
    fn same_content_hash_with_different_urls_dedups_and_refreshes() {
        let mut assets = HashMap::new();
        let hash = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let first = vec![
            MessageSegment::PlatformSpecific {
                platform: "qqbot".into(),
                kind: "attachment".into(),
                payload: serde_json::json!({
                    "url": "https://cdn.example/old.png",
                    "content_type": "image/png",
                    "filename": "a.png"
                }),
            },
            MessageSegment::Image {
                resource: test_resource("ref-old", hash),
            },
        ];
        let (_, refs) = normalize_segments(&first, &[], &mut assets, 1);
        assert_eq!(assets.len(), 1);
        assert_eq!(refs[0].h.as_deref(), Some(hash));
        assert_eq!(
            assets[hash].url.as_deref(),
            Some("https://cdn.example/old.png")
        );

        let second = vec![
            MessageSegment::PlatformSpecific {
                platform: "qqbot".into(),
                kind: "attachment".into(),
                payload: serde_json::json!({
                    "url": "https://cdn.example/new.png",
                    "content_type": "image/png",
                    "filename": "b.png"
                }),
            },
            MessageSegment::Image {
                resource: test_resource("ref-new", hash),
            },
        ];
        let (_, refs) = normalize_segments(&second, &[], &mut assets, 2);
        assert_eq!(assets.len(), 1);
        assert_eq!(refs[0].url.as_deref(), Some("https://cdn.example/new.png"));
        assert_eq!(
            assets[hash].url.as_deref(),
            Some("https://cdn.example/new.png")
        );
    }

    #[test]
    fn sticker_segments_do_not_enter_media_assets() {
        let hash = hash_bytes(b"sticker");
        let mut assets = HashMap::new();
        let segments = vec![MessageSegment::PlatformSpecific {
            platform: "sandbox".into(),
            kind: "sticker".into(),
            payload: serde_json::json!({
                "sticker_id": hash,
                "mime": "image/png",
                "name": "pack.png"
            }),
        }];
        let (text, refs) = normalize_segments(&segments, &[], &mut assets, 1);
        assert!(text.is_empty());
        assert!(assets.is_empty());
        assert_eq!(refs[0].kind, SandboxRefKind::Sticker);
        assert_eq!(refs[0].h.as_deref(), Some(hash.as_str()));
        let hydrated = hydrate_segments(&text, &refs, None);
        assert!(hydrated.iter().any(|segment| matches!(
            segment,
            MessageSegment::PlatformSpecific { kind, payload, .. }
                if kind == "sticker"
                    && payload.get("sticker_id").and_then(Value::as_str) == Some(hash.as_str())
        )));
    }

    #[test]
    fn official_faces_record_ids_without_blobs() {
        let mut faces = HashMap::new();
        let segments = vec![MessageSegment::PlatformSpecific {
            platform: "qqbot".into(),
            kind: "face".into(),
            payload: serde_json::json!({ "face_type": "6", "face_id": "0" }),
        }];
        let mut assets = HashMap::new();
        let (text, refs) = normalize_segments(&segments, &[], &mut assets, 9);
        assert!(assets.is_empty());
        record_faces(&mut faces, &refs, 9);
        assert_eq!(faces.len(), 1);
        let face = &faces["qq:6:0"];
        assert_eq!(face.face_type, "6");
        assert_eq!(face.face_id, "0");
        assert_eq!(face.last_seen_unix_ms, 9);
        let hydrated = hydrate_segments(&text, &refs, None);
        assert!(hydrated.iter().any(|segment| matches!(
            segment,
            MessageSegment::PlatformSpecific { kind, payload, .. }
                if kind == "face"
                    && payload.get("face_type").and_then(Value::as_str) == Some("6")
                    && payload.get("face_id").and_then(Value::as_str) == Some("0")
        )));
    }

    #[test]
    fn first_class_markdown_roundtrips_and_template_stays_platform_specific() {
        let mut assets = HashMap::new();
        let custom = vec![MessageSegment::markdown("**hi**")];
        let (text, refs) = normalize_segments(&custom, &[], &mut assets, 1);
        assert!(text.is_empty());
        assert_eq!(refs[0].kind, SandboxRefKind::Markdown);
        let hydrated = hydrate_segments(&text, &refs, None);
        assert!(hydrated.iter().any(|segment| matches!(
            segment,
            MessageSegment::Markdown { content } if content == "**hi**"
        )));

        let template = vec![MessageSegment::platform_specific(
            "qqbot",
            "markdown",
            serde_json::json!({"custom_template_id": "tpl-1"}),
        )];
        let (text, refs) = normalize_segments(&template, &[], &mut assets, 1);
        let hydrated = hydrate_segments(&text, &refs, None);
        assert!(hydrated.iter().any(|segment| matches!(
            segment,
            MessageSegment::PlatformSpecific { kind, payload, .. }
                if kind == "markdown"
                    && payload.get("custom_template_id").and_then(Value::as_str) == Some("tpl-1")
        )));
        assert!(
            !hydrated
                .iter()
                .any(|segment| matches!(segment, MessageSegment::Markdown { .. }))
        );
    }

    fn test_resource(ref_id: &str, hash: &str) -> mutsuki_runtime_contracts::ResourceRef {
        use mutsuki_runtime_contracts::{
            ResourceAccess, ResourceId, ResourceLifetime, ResourceRef, ResourceSealState,
            ResourceSemantic,
        };
        ResourceRef {
            ref_id: ref_id.into(),
            resource_id: ResourceId {
                kind_id: "blob".into(),
                slot_id: hash.into(),
                generation: 1,
                version: 1,
            },
            semantic: ResourceSemantic::FrozenValue,
            provider_id: "test".into(),
            resource_kind: "blob".into(),
            schema: "image/png".into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::Inline,
            size_hint: Some(1),
            content_hash: Some(hash.into()),
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        }
    }
}
