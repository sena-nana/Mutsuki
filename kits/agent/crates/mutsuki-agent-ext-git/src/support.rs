//! Shared digest and text truncation helpers used by backends and the service.

use sha2::{Digest, Sha256};

pub(crate) fn digest_bytes(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

pub(crate) fn digest_json(value: &impl serde::Serialize) -> String {
    digest_bytes(&serde_json::to_vec(value).expect("git state must serialize"))
}
pub(crate) fn truncate(input: &str, limit: usize) -> String {
    if input.chars().count() <= limit {
        return input.to_string();
    }
    input.chars().take(limit).collect::<String>() + "…"
}
