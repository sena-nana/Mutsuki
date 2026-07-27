use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

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

pub fn extract_urls(text: &str) -> Vec<Url> {
    let regex = Regex::new(r#"https?://[^\s<>\"'\]\[()]+"#).expect("static URL regex");
    let mut seen = BTreeSet::new();
    regex
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
    let mut candidates = Vec::new();
    collect_strings(&value, 0, &mut candidates);
    let mut seen = BTreeSet::new();
    Ok(candidates
        .into_iter()
        .flat_map(|candidate| extract_urls(&candidate))
        .filter(|url| seen.insert(url.as_str().to_owned()))
        .take(MAX_URLS)
        .collect())
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
    use super::*;

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
}
