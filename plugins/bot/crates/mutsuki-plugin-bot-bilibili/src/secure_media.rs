//! Bilibili-side adapter over the shared allowlisted fetch.
//!
//! The redirect/limit/allowlist loop itself lives in `mutsuki-bot-secure-fetch`; this
//! only maps the shared error onto `BilibiliError` so call sites keep their types.

use mutsuki_bot_secure_fetch::SecureFetchError;
use reqwest::blocking::Client;
use url::Url;

use super::BilibiliError;

fn to_bilibili(error: SecureFetchError) -> BilibiliError {
    match error {
        SecureFetchError::Denied(message) => BilibiliError::DomainDenied(message),
        SecureFetchError::Transport(message) => BilibiliError::Transport(message),
        SecureFetchError::InvalidResponse(message) => BilibiliError::InvalidResponse(message),
    }
}

fn shared_allow(
    allow: impl Fn(&Url) -> Result<(), BilibiliError>,
) -> impl Fn(&Url) -> Result<(), SecureFetchError> {
    move |url| {
        allow(url).map_err(|error| match error {
            // Carry the payload, not the rendered sentence: `to_bilibili` re-wraps it
            // in the same variant, so stringifying here would nest the prefix and
            // report "Bilibili domain is not allowed: Bilibili domain is not
            // allowed: <host>".
            BilibiliError::DomainDenied(host) => SecureFetchError::Denied(host),
            other => SecureFetchError::Denied(other.to_string()),
        })
    }
}

pub(crate) fn try_media_client(
    timeout: std::time::Duration,
    user_agent: &str,
) -> Result<Client, BilibiliError> {
    mutsuki_bot_secure_fetch::media_client(timeout, user_agent).map_err(to_bilibili)
}

/// Download body with hop-by-hop allowlist and a hard streamed byte budget.
pub(crate) fn secure_media_download(
    client: &Client,
    url: &str,
    max_bytes: usize,
    allow: impl Fn(&Url) -> Result<(), BilibiliError>,
) -> Result<Vec<u8>, BilibiliError> {
    mutsuki_bot_secure_fetch::download(client, url, max_bytes, shared_allow(allow))
        .map(|(_url, body)| body)
        .map_err(to_bilibili)
}

/// Follow redirects only (e.g. `b23.tv`), revalidating each hop.
pub(crate) fn secure_resolve_redirect(
    client: &Client,
    url: Url,
    allow: impl Fn(&Url) -> Result<(), BilibiliError>,
) -> Result<Url, BilibiliError> {
    mutsuki_bot_secure_fetch::resolve_redirect(client, url, shared_allow(allow))
        .map_err(to_bilibili)
}

#[cfg(test)]
mod tests {
    use url::Url;

    use crate::allow_bilibili_url;

    /// The transport loop itself is covered in `mutsuki-bot-secure-fetch`; what is
    /// Bilibili-specific is which hosts the allowlist admits.
    /// A denial must render once. The adapter maps `BilibiliError` into the shared
    /// error and back, so a careless `to_string()` in either direction doubles the
    /// "Bilibili domain is not allowed:" prefix.
    #[test]
    fn denied_domain_renders_its_prefix_once() {
        let error = super::secure_media_download(
            &super::try_media_client(std::time::Duration::from_secs(1), "mutsuki-test").unwrap(),
            "https://evil.example/x",
            16,
            allow_bilibili_url,
        )
        .unwrap_err();
        let rendered = error.to_string();
        assert_eq!(
            rendered.matches("Bilibili domain is not allowed").count(),
            1,
            "{rendered}"
        );
        assert!(rendered.contains("evil.example"), "{rendered}");
    }

    #[test]
    fn production_allowlist_rejects_http_and_foreign_hosts() {
        assert!(allow_bilibili_url(&Url::parse("http://i0.hdslb.com/x").unwrap()).is_err());
        assert!(allow_bilibili_url(&Url::parse("https://evil.example/x").unwrap()).is_err());
        assert!(allow_bilibili_url(&Url::parse("https://i0.hdslb.com/x").unwrap()).is_ok());
    }
}
