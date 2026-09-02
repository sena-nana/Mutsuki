// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{LazyLock, Mutex};

use mutsuki_bot_protocol::{BOT_EXT_LINK_URL, BotEvent, BotMessage, MessageSegment};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

pub const MAX_CARD_BYTES: usize = 32 * 1024;
pub const MAX_URLS: usize = 32;
pub const MAX_EXPANSION_DEPTH: usize = 4;
pub const MAX_LINK_CARD_MEDIA_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedLinkCard {
    pub url: String,
    pub title: String,
    pub description: String,
    pub image_url: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LinkParseError {
    #[error("card payload exceeds {MAX_CARD_BYTES} bytes")]
    PayloadTooLarge,
    #[error("card payload is not valid JSON: {0}")]
    InvalidJson(String),
}

/// Compiled once: `expand_card_payload` calls `extract_urls` per candidate string, so rebuilding
/// the automaton here would dominate the cost of parsing a single card.
static URL_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"https?://[^\s<>\"'\]\[()]+"#).expect("static URL regex"));

pub fn extract_urls(text: &str) -> Vec<Url> {
    let mut seen = BTreeSet::new();
    URL_PATTERN
        .find_iter(text)
        .filter_map(|found| Url::parse(found.as_str().trim_end_matches(['.', ',', ';'])).ok())
        .filter(|url| seen.insert(url.as_str().to_owned()))
        .take(MAX_URLS)
        .collect()
}

pub fn expand_card_payload(payload: &str) -> Result<Vec<Url>, LinkParseError> {
    if payload.len() > MAX_CARD_BYTES {
        return Err(LinkParseError::PayloadTooLarge);
    }
    let value: Value = serde_json::from_str(payload)
        .map_err(|error| LinkParseError::InvalidJson(error.to_string()))?;
    Ok(expand_card_value(&value))
}

pub fn expand_card_value(value: &Value) -> Vec<Url> {
    let mut candidates = Vec::new();
    collect_strings(value, 0, &mut candidates);
    dedup_urls(
        candidates
            .into_iter()
            .flat_map(|candidate| extract_urls(&candidate)),
    )
}

fn collect_strings(value: &Value, depth: usize, output: &mut Vec<String>) {
    if depth > MAX_EXPANSION_DEPTH || output.len() >= MAX_URLS * 4 {
        return;
    }
    match value {
        Value::String(value) => {
            output.push(value.clone());
            if value.len() <= MAX_CARD_BYTES
                && let Ok(nested) = serde_json::from_str::<Value>(value)
            {
                collect_strings(&nested, depth + 1, output);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_strings(value, depth + 1, output);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_strings(value, depth + 1, output);
            }
        }
        _ => {}
    }
}

fn dedup_urls(urls: impl IntoIterator<Item = Url>) -> Vec<Url> {
    let mut seen = BTreeSet::new();
    urls.into_iter()
        .filter(|url| seen.insert(url.as_str().to_owned()))
        .take(MAX_URLS)
        .collect()
}

fn urls_from_text(text: &str) -> Vec<Url> {
    let trimmed = text.trim();
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && let Ok(urls) = expand_card_payload(trimmed)
    {
        return urls;
    }
    extract_urls(text)
}

fn urls_from_segment(segment: &MessageSegment) -> Vec<Url> {
    match segment {
        MessageSegment::Text { text } | MessageSegment::Markdown { content: text } => {
            urls_from_text(text)
        }
        MessageSegment::PlatformSpecific { payload, .. } => expand_card_value(payload),
        _ => Vec::new(),
    }
}

fn urls_from_message(message: &BotMessage) -> Vec<Url> {
    dedup_urls(message.segments.iter().flat_map(urls_from_segment))
}

pub fn urls_from_event(event: &BotEvent) -> Vec<Url> {
    event
        .message
        .as_ref()
        .map(urls_from_message)
        .unwrap_or_default()
}

pub fn host_matches<I, S>(url: &Url, patterns: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    patterns.into_iter().any(|pattern| {
        let pattern = pattern
            .as_ref()
            .trim()
            .trim_start_matches("*.")
            .trim()
            .to_ascii_lowercase();
        !pattern.is_empty() && (host == pattern || host.ends_with(&format!(".{pattern}")))
    })
}

pub fn preferred_event_url(event: &BotEvent, hosts: &[&str]) -> Option<String> {
    if let Some(url) = event.ext.get(BOT_EXT_LINK_URL).and_then(Value::as_str)
        && let Ok(parsed) = Url::parse(url)
        && (hosts.is_empty() || host_matches(&parsed, hosts.iter().copied()))
    {
        return Some(url.to_owned());
    }
    urls_from_event(event)
        .into_iter()
        .find(|url| hosts.is_empty() || host_matches(url, hosts.iter().copied()))
        .map(|url| url.to_string())
}

#[derive(Debug, Default)]
pub struct CooldownBook {
    seen: Mutex<BTreeMap<String, u64>>,
}

impl CooldownBook {
    pub fn admit(&self, key: impl Into<String>, now_ms: u64, cooldown_ms: u64) -> bool {
        let key = key.into();
        let mut seen = self.seen.lock().expect("cooldown mutex");
        if seen
            .get(&key)
            .is_some_and(|previous| now_ms.saturating_sub(*previous) < cooldown_ms)
        {
            return false;
        }
        seen.insert(key, now_ms);
        true
    }
}

#[cfg(test)]
mod tests {
    use mutsuki_bot_protocol::{
        BotAccountRef, BotEventKind, BotPlatform, BotTarget, MessageSegment,
    };

    use super::*;

    fn event_with_segments(segments: Vec<MessageSegment>) -> BotEvent {
        BotEvent {
            event_id: "e1".into(),
            platform: BotPlatform::QqBot,
            bot: BotAccountRef {
                account_id: "bot".into(),
                platform: BotPlatform::QqBot,
            },
            kind: BotEventKind::MessageCreated,
            time_ms: 1,
            target: BotTarget::Group {
                group_id: "g1".into(),
            },
            actor: None,
            message: Some(BotMessage {
                message_id: None,
                target: BotTarget::Group {
                    group_id: "g1".into(),
                },
                sender: None,
                segments,
                reply_to: None,
                time_ms: None,
                ext: BTreeMap::default(),
            }),
            raw: None,
            ext: BTreeMap::default(),
        }
    }

    #[test]
    fn expands_nested_card_payload_with_dedup_and_limits() {
        let payload = serde_json::json!({
            "meta": "{\"jumpUrl\":\"https://b23.tv/abc\"}",
            "detail": {"url": "https://www.bilibili.com/video/BV1xx"},
            "again": "https://b23.tv/abc"
        })
        .to_string();
        let urls = expand_card_payload(&payload).unwrap();
        assert_eq!(urls.len(), 2);
    }

    #[test]
    fn cooldown_is_keyed_and_monotonic() {
        let cooldown = CooldownBook::default();
        assert!(cooldown.admit("account:url", 100, 50));
        assert!(!cooldown.admit("account:url", 120, 50));
        assert!(cooldown.admit("account:url", 151, 50));
    }

    #[test]
    fn extracts_urls_from_text_markdown_and_ark_payloads() {
        let event = event_with_segments(vec![
            MessageSegment::text("see https://www.mihuashi.com/profiles/1"),
            MessageSegment::platform_specific(
                "qqbot",
                "ark_data",
                serde_json::json!({"meta": {"jumpUrl": "https://b23.tv/abc"}}),
            ),
        ]);
        let urls = urls_from_event(&event)
            .into_iter()
            .map(|url| url.to_string())
            .collect::<Vec<_>>();
        assert!(urls.contains(&"https://www.mihuashi.com/profiles/1".into()));
        assert!(urls.contains(&"https://b23.tv/abc".into()));
    }

    #[test]
    fn compact_json_text_is_expanded() {
        let event = event_with_segments(vec![MessageSegment::text(
            "{\"qqdocurl\":\"https://b23.tv/mini\"}",
        )]);
        assert_eq!(urls_from_event(&event)[0].as_str(), "https://b23.tv/mini");
    }

    #[test]
    fn host_allowlist_matches_subdomains_and_rejects_lookalikes() {
        let bili = Url::parse("https://www.bilibili.com/video/BV1").unwrap();
        let short = Url::parse("https://b23.tv/abc").unwrap();
        let evil = Url::parse("https://evilbilibili.com/x").unwrap();
        assert!(host_matches(&bili, ["bilibili.com"]));
        assert!(host_matches(&short, ["b23.tv", "bilibili.com"]));
        assert!(!host_matches(&evil, ["bilibili.com"]));
        assert!(!host_matches(&bili, ["mihuashi.com"]));
    }

    #[test]
    fn preferred_url_uses_ext_then_allowlist() {
        let mut event = event_with_segments(vec![MessageSegment::text(
            "https://www.mihuashi.com/profiles/1 https://b23.tv/abc",
        )]);
        assert_eq!(
            preferred_event_url(&event, &["b23.tv", "bilibili.com"]).as_deref(),
            Some("https://b23.tv/abc")
        );
        event.ext.insert(
            BOT_EXT_LINK_URL.into(),
            Value::String("https://b23.tv/picked".into()),
        );
        assert_eq!(
            preferred_event_url(&event, &["b23.tv"]).as_deref(),
            Some("https://b23.tv/picked")
        );
        assert_eq!(
            preferred_event_url(&event, &["mihuashi.com"]).as_deref(),
            Some("https://www.mihuashi.com/profiles/1")
        );
    }
}
