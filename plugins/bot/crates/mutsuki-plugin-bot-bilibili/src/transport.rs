//! Upstream Bilibili transport: the transport trait, the blocking HTTP client, WBI signing
//! and the domain allowlist.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mutsuki_bot_link_parser::ResolvedLinkCard;
use reqwest::blocking::Client;
use serde_json::Value;
use url::Url;

use crate::parse::parse_poll_items;
use crate::secure_media;
use crate::{
    BilibiliError, BilibiliItem, BilibiliPollKind, BilibiliProfile, BilibiliQrCode, BilibiliQrPoll,
    BilibiliQrStatus, SharedBilibiliCredential,
};

pub trait BilibiliTransport: Send {
    fn poll(
        &mut self,
        kind: &BilibiliPollKind,
        uid: u64,
    ) -> Result<Vec<BilibiliItem>, BilibiliError>;
    fn resolve(&mut self, url: &str) -> Result<ResolvedLinkCard, BilibiliError>;
    fn download(&mut self, url: &str, max_bytes: usize) -> Result<Vec<u8>, BilibiliError>;
    fn qr_start(&mut self) -> Result<BilibiliQrCode, BilibiliError>;
    fn qr_poll(&mut self, key: &str) -> Result<BilibiliQrPoll, BilibiliError>;
    fn profile(&mut self, uid: u64) -> Result<BilibiliProfile, BilibiliError>;
}

/// Blocking HTTP transport for the Bilibili runner.
///
/// The runner reaches this through `TaskAwaitRunnerAdapter`, which the Host polls on its sync
/// worker threads, so a call here occupies one worker and never the async reactor. Every request
/// is bounded by `timeout` to keep that occupancy finite. Console code paths are async and must
/// not call these methods inline; `management::in_blocking_section` marks the hand-off.
pub struct ReqwestBilibiliTransport {
    client: Option<Client>,
    credential: SharedBilibiliCredential,
    timeout: Duration,
    wbi_key: WbiKeyCache,
}

/// Upstream rotates the WBI mixin key about once a day, so re-deriving it per
/// signed request meant every poll spent two HTTP round trips instead of one and
/// hammered `/x/web-interface/nav` at the polling interval times the subscription
/// count. That doubles the rate-limit exposure the `RiskControl352` handling
/// downstream exists to survive.
const WBI_MIXIN_KEY_TTL: Duration = Duration::from_mins(60);

#[derive(Default)]
struct WbiKeyCache {
    entry: Option<(String, Instant)>,
}

impl WbiKeyCache {
    fn fresh(&self, now: Instant) -> Option<String> {
        self.entry
            .as_ref()
            .filter(|(_, fetched_at)| now.duration_since(*fetched_at) < WBI_MIXIN_KEY_TTL)
            .map(|(key, _)| key.clone())
    }

    fn store(&mut self, key: String, now: Instant) {
        self.entry = Some((key, now));
    }

    /// Dropped whenever upstream rejects the credential or trips risk control:
    /// a stale key is one reason for exactly those codes, so retrying with the
    /// same cached value would keep failing until the TTL happened to elapse.
    fn invalidate(&mut self) {
        self.entry = None;
    }
}

impl ReqwestBilibiliTransport {
    pub fn new(credential: SharedBilibiliCredential, timeout: Duration) -> Self {
        Self {
            client: None,
            credential,
            timeout,
            wbi_key: WbiKeyCache::default(),
        }
    }

    fn client(&mut self) -> Result<&Client, BilibiliError> {
        if self.client.is_none() {
            self.client = Some(secure_media::try_media_client(
                self.timeout,
                "Mozilla/5.0 MutsukiBot/0.1",
            )?);
        }
        Ok(self.client.as_ref().expect("client initialized"))
    }

    fn json(&mut self, url: &str) -> Result<Value, BilibiliError> {
        ensure_bilibili_domain(url)?;
        let cookie = self.credential.get()?;
        let response = self
            .client()?
            .get(url)
            .header("Cookie", cookie)
            .send()
            .map_err(|error| BilibiliError::Transport(error.to_string()))?;
        if response.status().as_u16() == 429 {
            return Err(BilibiliError::RateLimited);
        }
        let value: Value = response
            .json()
            .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
        match value.get("code").and_then(Value::as_i64) {
            Some(-101) => {
                self.wbi_key.invalidate();
                Err(BilibiliError::CookieExpired)
            }
            Some(-352 | 352) => {
                self.wbi_key.invalidate();
                Err(BilibiliError::RiskControl352)
            }
            Some(code) if code != 0 => Err(BilibiliError::InvalidResponse(format!("code {code}"))),
            _ => Ok(value),
        }
    }

    fn wbi_mixin_key(&mut self) -> Result<String, BilibiliError> {
        let now = Instant::now();
        if let Some(key) = self.wbi_key.fresh(now) {
            return Ok(key);
        }
        let nav = self.json("https://api.bilibili.com/x/web-interface/nav")?;
        let img = string_field(&nav["data"]["wbi_img"], "img_url")?;
        let sub = string_field(&nav["data"]["wbi_img"], "sub_url")?;
        let key = wbi_mixin_key(&img, &sub)?;
        self.wbi_key.store(key.clone(), now);
        Ok(key)
    }

    fn wbi_url(
        &mut self,
        path: &str,
        params: Vec<(String, String)>,
    ) -> Result<String, BilibiliError> {
        let key = self.wbi_mixin_key()?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| BilibiliError::Transport(error.to_string()))?
            .as_secs() as i64;
        Ok(format!(
            "https://api.bilibili.com{path}?{}",
            sign_wbi_query(&params, &key, now)
        ))
    }
}

impl BilibiliTransport for ReqwestBilibiliTransport {
    fn poll(
        &mut self,
        kind: &BilibiliPollKind,
        uid: u64,
    ) -> Result<Vec<BilibiliItem>, BilibiliError> {
        let url = match kind {
            BilibiliPollKind::Live => self.wbi_url(
                "/x/space/wbi/acc/info",
                vec![("mid".into(), uid.to_string())],
            )?,
            BilibiliPollKind::Dynamic => format!(
                "https://api.bilibili.com/x/polymer/web-dynamic/v1/feed/space?host_mid={uid}"
            ),
            BilibiliPollKind::Video => self.wbi_url(
                "/x/space/wbi/arc/search",
                vec![
                    ("mid".into(), uid.to_string()),
                    ("pn".into(), "1".into()),
                    ("ps".into(), "10".into()),
                ],
            )?,
        };
        parse_poll_items(kind, uid, self.json(&url)?)
    }

    fn resolve(&mut self, url: &str) -> Result<ResolvedLinkCard, BilibiliError> {
        ensure_bilibili_domain(url)?;
        let mut parsed =
            Url::parse(url).map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
        if parsed.host_str() == Some("b23.tv") {
            let client = self.client()?;
            parsed = secure_media::secure_resolve_redirect(client, parsed, allow_bilibili_url)?;
        }
        let path = parsed.path();
        let bvid = path.split('/').find(|part| part.starts_with("BV"));
        let api = bvid
            .map(|bvid| format!("https://api.bilibili.com/x/web-interface/view?bvid={bvid}"))
            .ok_or_else(|| BilibiliError::InvalidResponse("unsupported Bilibili link".into()))?;
        let value = self.json(&api)?;
        let data = &value["data"];
        Ok(ResolvedLinkCard {
            url: parsed.to_string(),
            title: string_field(data, "title")?,
            description: data
                .get("desc")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
            image_url: data
                .get("pic")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        })
    }

    fn download(&mut self, url: &str, max_bytes: usize) -> Result<Vec<u8>, BilibiliError> {
        let client = self.client()?;
        secure_media::secure_media_download(client, url, max_bytes, allow_bilibili_url)
    }

    fn qr_start(&mut self) -> Result<BilibiliQrCode, BilibiliError> {
        let value: Value = self
            .client()?
            .get("https://passport.bilibili.com/x/passport-login/web/qrcode/generate")
            .send()
            .map_err(|error| BilibiliError::Transport(error.to_string()))?
            .json()
            .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
        if value.get("code").and_then(Value::as_i64) != Some(0) {
            return Err(BilibiliError::InvalidResponse(
                "QR generation failed".into(),
            ));
        }
        Ok(BilibiliQrCode {
            url: string_field(&value["data"], "url")?,
            key: string_field(&value["data"], "qrcode_key")?,
        })
    }

    fn qr_poll(&mut self, key: &str) -> Result<BilibiliQrPoll, BilibiliError> {
        let mut url = Url::parse("https://passport.bilibili.com/x/passport-login/web/qrcode/poll")
            .expect("static Bilibili QR URL");
        url.query_pairs_mut()
            .append_pair("qrcode_key", key)
            .append_pair("source", "main-fe-header");
        let value: Value = self
            .client()?
            .get(url)
            .send()
            .map_err(|error| BilibiliError::Transport(error.to_string()))?
            .json()
            .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
        let code = value["data"]["code"]
            .as_i64()
            .ok_or_else(|| BilibiliError::InvalidResponse("QR status code".into()))?;
        let status = match code {
            86101 => BilibiliQrStatus::Pending,
            86090 => BilibiliQrStatus::Scanned,
            86038 => BilibiliQrStatus::Expired,
            0 => BilibiliQrStatus::Confirmed,
            _ => {
                return Err(BilibiliError::InvalidResponse(format!(
                    "QR status code {code}"
                )));
            }
        };
        let credential = if status == BilibiliQrStatus::Confirmed {
            Some(credential_from_redirect(
                string_field(&value["data"], "url")?.as_str(),
            )?)
        } else {
            None
        };
        Ok(BilibiliQrPoll { status, credential })
    }

    fn profile(&mut self, uid: u64) -> Result<BilibiliProfile, BilibiliError> {
        let url = self.wbi_url(
            "/x/space/wbi/acc/info",
            vec![("mid".into(), uid.to_string())],
        )?;
        let value = self.json(&url)?;
        Ok(BilibiliProfile {
            name: string_field(&value["data"], "name")?,
            signature: value["data"]["sign"].as_str().unwrap_or_default().into(),
        })
    }
}

pub(crate) fn ensure_bilibili_domain(value: &str) -> Result<(), BilibiliError> {
    let url = Url::parse(value).map_err(|error| BilibiliError::DomainDenied(error.to_string()))?;
    allow_bilibili_url(&url)
}

pub(crate) fn allow_bilibili_url(url: &Url) -> Result<(), BilibiliError> {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let allowed = host == "b23.tv"
        || host == "bilibili.com"
        || host.ends_with(".bilibili.com")
        || host == "hdslb.com"
        || host.ends_with(".hdslb.com");
    if url.scheme() == "https" && allowed && url.username().is_empty() && url.password().is_none() {
        Ok(())
    } else {
        Err(BilibiliError::DomainDenied(host))
    }
}

fn credential_from_redirect(value: &str) -> Result<String, BilibiliError> {
    let url =
        Url::parse(value).map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
    let allowed = [
        "SESSDATA",
        "bili_jct",
        "DedeUserID",
        "DedeUserID__ckMd5",
        "buvid3",
    ];
    let values = url
        .query_pairs()
        .filter(|(key, value)| allowed.contains(&key.as_ref()) && !value.trim().is_empty())
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>();
    if !values.iter().any(|value| value.starts_with("SESSDATA=")) {
        return Err(BilibiliError::InvalidResponse(
            "QR login response did not contain SESSDATA".into(),
        ));
    }
    Ok(values.join("; "))
}

fn string_field(value: &Value, field: &str) -> Result<String, BilibiliError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| BilibiliError::InvalidResponse(field.into()))
}

pub fn sign_wbi_query(params: &[(String, String)], mixin_key: &str, unix_seconds: i64) -> String {
    let mut params = params.to_vec();
    params.push(("wts".into(), unix_seconds.to_string()));
    params.sort_by(|left, right| left.0.cmp(&right.0));
    let encoded = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(
            params
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        )
        .finish();
    let signature = format!("{:x}", md5::compute(format!("{encoded}{mixin_key}")));
    format!("{encoded}&w_rid={signature}")
}

fn wbi_mixin_key(img_url: &str, sub_url: &str) -> Result<String, BilibiliError> {
    const TABLE: [usize; 64] = [
        46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19,
        29, 28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4,
        22, 25, 54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
    ];
    let filename = |value: &str| {
        Url::parse(value)
            .ok()?
            .path_segments()?
            .next_back()?
            .split('.')
            .next()
            .map(ToOwned::to_owned)
    };
    let source = format!(
        "{}{}",
        filename(img_url).ok_or_else(|| BilibiliError::InvalidResponse("wbi img key".into()))?,
        filename(sub_url).ok_or_else(|| BilibiliError::InvalidResponse("wbi sub key".into()))?
    );
    let chars = source.chars().collect::<Vec<_>>();
    if chars.len() < 64 {
        return Err(BilibiliError::InvalidResponse("wbi key length".into()));
    }
    Ok(TABLE.iter().take(32).map(|index| chars[*index]).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Re-deriving the WBI key per signed request doubled the HTTP per poll and
    /// pointed the extra request at the same endpoint upstream rate-limits.
    #[test]
    fn wbi_key_is_reused_within_its_ttl_and_refetched_after_it() {
        let now = Instant::now();
        let mut cache = WbiKeyCache::default();
        assert_eq!(cache.fresh(now), None, "an empty cache must force a fetch");

        cache.store("mixin-key".into(), now);
        assert_eq!(cache.fresh(now).as_deref(), Some("mixin-key"));
        assert_eq!(
            cache
                .fresh(
                    (now + WBI_MIXIN_KEY_TTL)
                        .checked_sub(Duration::from_secs(1))
                        .expect("one second before the TTL elapses"),
                )
                .as_deref(),
            Some("mixin-key"),
            "still inside the TTL"
        );
        assert_eq!(
            cache.fresh(now + WBI_MIXIN_KEY_TTL),
            None,
            "the key rotates upstream, so the TTL must expire it"
        );
    }

    /// `-101` and `-352` are exactly the codes a stale key produces, so keeping
    /// the cached value would make every retry fail until the TTL elapsed.
    #[test]
    fn wbi_key_is_dropped_when_upstream_rejects_the_request() {
        let now = Instant::now();
        let mut cache = WbiKeyCache::default();
        cache.store("mixin-key".into(), now);
        cache.invalidate();
        assert_eq!(cache.fresh(now), None);
    }

    #[test]
    fn wbi_signature_is_deterministic_with_fixed_clock() {
        let signed = sign_wbi_query(&[("mid".into(), "1".into())], "secret", 1_700_000_000);
        assert_eq!(
            signed,
            sign_wbi_query(&[("mid".into(), "1".into())], "secret", 1_700_000_000)
        );
        assert!(signed.contains("wts=1700000000&w_rid="));
    }

    #[test]
    fn qr_redirect_extracts_only_cookie_fields_and_requires_sessdata() {
        let credential = credential_from_redirect(
            "https://www.bilibili.com/?SESSDATA=abc%3D%3D&bili_jct=csrf&ignored=value",
        )
        .unwrap();
        assert_eq!(credential, "SESSDATA=abc==; bili_jct=csrf");
        assert!(credential_from_redirect("https://www.bilibili.com/?bili_jct=x").is_err());
    }
}
