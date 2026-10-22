//! Upstream Bilibili API over the std HTTP effect (`mutsuki.http.request@2`): the transport
//! trait, its HTTP implementation, WBI signing and the domain allowlist.
//!
//! No request carries the login cookie itself: API calls name the Host secret through
//! `secret_headers` and the HTTP effect resolves it when it runs, and QR login stores the
//! returned cookies straight into that secret through `capture_cookies`.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use mutsuki_bot_link_parser::ResolvedLinkCard;
use mutsuki_protocol_http::{
    HttpCookieCapture, HttpRequest, HttpResponse, HttpResponseBody, REQUEST as HTTP_REQUEST,
};
use mutsuki_runtime_contracts::{
    ResourceRef, RuntimeError, Task, TaskBatch, TaskHandle, TaskOutcome,
};
use mutsuki_runtime_sdk::{AsyncRunnerContext, RuntimeClientRef, TaskHandleFuture};
use serde_json::Value;
use url::Url;

use crate::parse::parse_poll_items;
use crate::{
    BilibiliError, BilibiliItem, BilibiliPollKind, BilibiliProfile, BilibiliQrCode,
    BilibiliQrStatus, SharedBilibiliConfig,
};

/// Hosts every Bilibili request may reach; the product grants these to the HTTP client.
pub const HTTP_DOMAINS: &[&str] = &["b23.tv", "bilibili.com", "hdslb.com"];
/// The domain the login cookie secret is sent to and captured from.
pub const COOKIE_DOMAIN: &str = "bilibili.com";
const USER_AGENT: &str = "Mozilla/5.0 MutsukiBot/0.1";
const REQUEST_TIMEOUT_MS: u64 = 15_000;
/// How long a Web Console caller waits for a submitted task; above any HTTP total timeout.
const TOP_LEVEL_TASK_WAIT: Duration = Duration::from_secs(60);
const QR_COOKIES: &[&str] = &[
    "SESSDATA",
    "bili_jct",
    "DedeUserID",
    "DedeUserID__ckMd5",
    "buvid3",
];

/// One round trip through `mutsuki.http.request@2`.
#[async_trait]
pub trait BilibiliHttp: Send + Sync {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, BilibiliError>;
}

/// Inside the runner every request is a child task of the task being run.
pub(crate) struct RunnerHttp<'a>(pub(crate) &'a AsyncRunnerContext);

#[async_trait]
impl BilibiliHttp for RunnerHttp<'_> {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, BilibiliError> {
        let payload = serde_json::to_value(request)
            .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
        let outcome = self
            .0
            .call_raw(HTTP_REQUEST, payload)
            .await
            .map_err(|failure| http_error(failure.error()))?;
        decode_http_outcome(outcome.into_outcome())
    }
}

/// Outside a runner (Web Console management) each request is its own task submitted through
/// the Host client.
pub struct RuntimeBilibiliHttp {
    client: RuntimeClientRef,
    next_task: AtomicU64,
}

impl RuntimeBilibiliHttp {
    #[must_use]
    pub fn new(client: RuntimeClientRef) -> Self {
        Self {
            client,
            next_task: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl BilibiliHttp for RuntimeBilibiliHttp {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, BilibiliError> {
        let sequence = self.next_task.fetch_add(1, Ordering::Relaxed) + 1;
        let epoch_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let task_id = format!("bilibili.http.{epoch_nanos}.{sequence}");
        let task = Task::new(
            task_id.clone(),
            HTTP_REQUEST,
            serde_json::to_value(request)
                .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?,
        );
        let handle = self
            .client
            .submit_batch(TaskBatch::one(format!("{task_id}.batch"), task))
            .map_err(|failure| http_error(failure.error()))?
            .into_iter()
            .next()
            .ok_or_else(|| {
                BilibiliError::Transport("HTTP request returned no task handle".into())
            })?;
        decode_http_outcome(await_top_level_task(self.client.clone(), handle).await?)
    }
}

/// Waits for a task submitted outside a runner (console RPC, polling). The Host client parks the
/// future's waker with its actor, so the await holds no executor or blocking-pool thread.
pub(crate) async fn await_top_level_task(
    client: RuntimeClientRef,
    handle: TaskHandle,
) -> Result<TaskOutcome, BilibiliError> {
    tokio::time::timeout(TOP_LEVEL_TASK_WAIT, TaskHandleFuture::new(client, handle))
        .await
        .map_err(|_| BilibiliError::Transport("task did not finish in time".into()))?
        .map_err(|failure| http_error(failure.error()))
}

fn decode_http_outcome(outcome: TaskOutcome) -> Result<HttpResponse, BilibiliError> {
    match outcome {
        TaskOutcome::Completed {
            output: Some(output),
            ..
        } => serde_json::from_value(output)
            .map_err(|error| BilibiliError::InvalidResponse(error.to_string())),
        TaskOutcome::Completed { output: None, .. } => Err(BilibiliError::InvalidResponse(
            "HTTP request completed without a response".into(),
        )),
        TaskOutcome::Failed { error, .. } => Err(http_error(&error)),
        outcome => Err(BilibiliError::Transport(format!(
            "HTTP request did not complete: {outcome:?}"
        ))),
    }
}

fn http_error(error: &RuntimeError) -> BilibiliError {
    let detail = format!("{}: {}", error.code, error.route);
    match error.code.as_str() {
        "http.secret_unavailable" => BilibiliError::CookieExpired,
        "http.domain_denied"
        | "http.redirect_denied"
        | "http.https_required"
        | "http.invalid_url"
        | "http.private_address" => BilibiliError::DomainDenied(detail),
        _ => BilibiliError::Transport(detail),
    }
}

/// The Bilibili API surface the runner and the management service use. `http` is the
/// caller's channel to the HTTP effect, so one transport (and its WBI key cache) serves
/// both the runner and the Web Console.
#[async_trait]
pub trait BilibiliTransport: Send + Sync {
    async fn poll(
        &self,
        http: &dyn BilibiliHttp,
        kind: &BilibiliPollKind,
        uid: u64,
    ) -> Result<Vec<BilibiliItem>, BilibiliError>;
    async fn resolve(
        &self,
        http: &dyn BilibiliHttp,
        url: &str,
    ) -> Result<ResolvedLinkCard, BilibiliError>;
    /// The downloaded body as the resource the HTTP effect stored it in.
    async fn download(
        &self,
        http: &dyn BilibiliHttp,
        url: &str,
        max_bytes: usize,
    ) -> Result<ResourceRef, BilibiliError>;
    async fn qr_start(&self, http: &dyn BilibiliHttp) -> Result<BilibiliQrCode, BilibiliError>;
    /// On `Confirmed` the login cookies are already stored in the cookie secret.
    async fn qr_poll(
        &self,
        http: &dyn BilibiliHttp,
        key: &str,
    ) -> Result<BilibiliQrStatus, BilibiliError>;
    async fn profile(
        &self,
        http: &dyn BilibiliHttp,
        uid: u64,
    ) -> Result<BilibiliProfile, BilibiliError>;
}

/// The production transport. The cookie secret key is read from the shared config per
/// request, so a config change applies to the next request.
pub struct HttpBilibiliTransport {
    config: SharedBilibiliConfig,
    wbi_key: Mutex<WbiKeyCache>,
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

impl HttpBilibiliTransport {
    #[must_use]
    pub fn new(config: SharedBilibiliConfig) -> Self {
        Self {
            config,
            wbi_key: Mutex::new(WbiKeyCache::default()),
        }
    }

    fn cookie_secret_key(&self) -> Result<String, BilibiliError> {
        self.config
            .current()
            .backend
            .cookie_secret_key()
            .map(ToOwned::to_owned)
            .ok_or(BilibiliError::CookieExpired)
    }

    fn wbi_cache(&self) -> std::sync::MutexGuard<'_, WbiKeyCache> {
        self.wbi_key.lock().expect("Bilibili WBI key mutex")
    }

    /// An authenticated API call; the cookie is resolved by the HTTP effect.
    async fn json(&self, http: &dyn BilibiliHttp, url: &str) -> Result<Value, BilibiliError> {
        let mut request = bilibili_request(url, json_body())?;
        request
            .secret_headers
            .insert("Cookie".into(), self.cookie_secret_key()?);
        let response = http.send(request).await?;
        if response.metadata.status == 429 {
            return Err(BilibiliError::RateLimited);
        }
        let value = response_json(response)?;
        match value.get("code").and_then(Value::as_i64) {
            Some(-101) => {
                self.wbi_cache().invalidate();
                Err(BilibiliError::CookieExpired)
            }
            Some(-352 | 352) => {
                self.wbi_cache().invalidate();
                Err(BilibiliError::RiskControl352)
            }
            Some(code) if code != 0 => Err(BilibiliError::InvalidResponse(format!("code {code}"))),
            _ => Ok(value),
        }
    }

    async fn wbi_mixin_key(&self, http: &dyn BilibiliHttp) -> Result<String, BilibiliError> {
        let now = Instant::now();
        if let Some(key) = self.wbi_cache().fresh(now) {
            return Ok(key);
        }
        let nav = self
            .json(http, "https://api.bilibili.com/x/web-interface/nav")
            .await?;
        let img = string_field(&nav["data"]["wbi_img"], "img_url")?;
        let sub = string_field(&nav["data"]["wbi_img"], "sub_url")?;
        let key = wbi_mixin_key(&img, &sub)?;
        self.wbi_cache().store(key.clone(), now);
        Ok(key)
    }

    async fn wbi_url(
        &self,
        http: &dyn BilibiliHttp,
        path: &str,
        params: Vec<(String, String)>,
    ) -> Result<String, BilibiliError> {
        let key = self.wbi_mixin_key(http).await?;
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

fn json_body() -> HttpResponseBody {
    HttpResponseBody::Json { select: Vec::new() }
}

/// A GET restricted to the Bilibili hosts, without credentials.
fn bilibili_request(url: &str, body: HttpResponseBody) -> Result<HttpRequest, BilibiliError> {
    ensure_bilibili_domain(url)?;
    let mut request = HttpRequest::get(url);
    request
        .headers
        .insert("User-Agent".into(), USER_AGENT.into());
    request.response_body = body;
    request.limits.total_timeout_ms = Some(REQUEST_TIMEOUT_MS);
    request.limits.domain_allowlist =
        Some(HTTP_DOMAINS.iter().map(|&domain| domain.into()).collect());
    Ok(request)
}

fn response_json(response: HttpResponse) -> Result<Value, BilibiliError> {
    let status = response.metadata.status;
    response
        .json
        .ok_or_else(|| BilibiliError::InvalidResponse(format!("HTTP {status} without a JSON body")))
}

#[async_trait]
impl BilibiliTransport for HttpBilibiliTransport {
    async fn poll(
        &self,
        http: &dyn BilibiliHttp,
        kind: &BilibiliPollKind,
        uid: u64,
    ) -> Result<Vec<BilibiliItem>, BilibiliError> {
        let url = match kind {
            BilibiliPollKind::Live => {
                self.wbi_url(
                    http,
                    "/x/space/wbi/acc/info",
                    vec![("mid".into(), uid.to_string())],
                )
                .await?
            }
            BilibiliPollKind::Dynamic => format!(
                "https://api.bilibili.com/x/polymer/web-dynamic/v1/feed/space?host_mid={uid}"
            ),
            BilibiliPollKind::Video => {
                self.wbi_url(
                    http,
                    "/x/space/wbi/arc/search",
                    vec![
                        ("mid".into(), uid.to_string()),
                        ("pn".into(), "1".into()),
                        ("ps".into(), "10".into()),
                    ],
                )
                .await?
            }
        };
        parse_poll_items(kind, uid, self.json(http, &url).await?)
    }

    async fn resolve(
        &self,
        http: &dyn BilibiliHttp,
        url: &str,
    ) -> Result<ResolvedLinkCard, BilibiliError> {
        ensure_bilibili_domain(url)?;
        let mut parsed =
            Url::parse(url).map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
        if parsed.host_str() == Some("b23.tv") {
            // Only the redirect target matters; the effect revalidates every hop.
            let response = http
                .send(bilibili_request(url, HttpResponseBody::Discard)?)
                .await?;
            ensure_bilibili_domain(&response.metadata.final_url)?;
            parsed = Url::parse(&response.metadata.final_url)
                .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
        }
        let path = parsed.path();
        let bvid = path.split('/').find(|part| part.starts_with("BV"));
        let api = bvid
            .map(|bvid| format!("https://api.bilibili.com/x/web-interface/view?bvid={bvid}"))
            .ok_or_else(|| BilibiliError::InvalidResponse("unsupported Bilibili link".into()))?;
        let value = self.json(http, &api).await?;
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

    async fn download(
        &self,
        http: &dyn BilibiliHttp,
        url: &str,
        max_bytes: usize,
    ) -> Result<ResourceRef, BilibiliError> {
        let mut request = bilibili_request(url, HttpResponseBody::Resource)?;
        request.limits.max_response_bytes = Some(max_bytes as u64);
        let response = http.send(request).await?;
        if !(200..300).contains(&response.metadata.status) {
            return Err(BilibiliError::Transport(format!(
                "media download returned HTTP {}",
                response.metadata.status
            )));
        }
        ensure_bilibili_domain(&response.metadata.final_url)?;
        response
            .body
            .ok_or_else(|| BilibiliError::InvalidResponse("media response body is empty".into()))
    }

    async fn qr_start(&self, http: &dyn BilibiliHttp) -> Result<BilibiliQrCode, BilibiliError> {
        let response = http
            .send(bilibili_request(
                "https://passport.bilibili.com/x/passport-login/web/qrcode/generate",
                json_body(),
            )?)
            .await?;
        let value = response_json(response)?;
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

    async fn qr_poll(
        &self,
        http: &dyn BilibiliHttp,
        key: &str,
    ) -> Result<BilibiliQrStatus, BilibiliError> {
        let mut url = Url::parse("https://passport.bilibili.com/x/passport-login/web/qrcode/poll")
            .expect("static Bilibili QR URL");
        url.query_pairs_mut()
            .append_pair("qrcode_key", key)
            .append_pair("source", "main-fe-header");
        // The confirmed response repeats the cookies in `data.url`; selecting only the status
        // keeps them out of the task output, and the effect stores the Set-Cookie values.
        let mut request = bilibili_request(
            url.as_str(),
            HttpResponseBody::Json {
                select: vec![QR_STATUS_POINTER.into()],
            },
        )?;
        request.capture_cookies = Some(HttpCookieCapture {
            secret_key: self.cookie_secret_key()?,
            cookies: QR_COOKIES.iter().map(|&name| name.into()).collect(),
            required: vec!["SESSDATA".into()],
        });
        let response = http.send(request).await?;
        let captured = response.captured_secret.is_some();
        let code = response_json(response)?[QR_STATUS_POINTER]
            .as_i64()
            .ok_or_else(|| BilibiliError::InvalidResponse("QR status code".into()))?;
        let status = match code {
            86101 => BilibiliQrStatus::Pending,
            86090 => BilibiliQrStatus::Scanned,
            86038 => BilibiliQrStatus::Expired,
            0 if captured => BilibiliQrStatus::Confirmed,
            0 => {
                return Err(BilibiliError::InvalidResponse(
                    "confirmed QR login did not set the SESSDATA cookie".into(),
                ));
            }
            _ => {
                return Err(BilibiliError::InvalidResponse(format!(
                    "QR status code {code}"
                )));
            }
        };
        Ok(status)
    }

    async fn profile(
        &self,
        http: &dyn BilibiliHttp,
        uid: u64,
    ) -> Result<BilibiliProfile, BilibiliError> {
        let url = self
            .wbi_url(
                http,
                "/x/space/wbi/acc/info",
                vec![("mid".into(), uid.to_string())],
            )
            .await?;
        let value = self.json(http, &url).await?;
        Ok(BilibiliProfile {
            name: string_field(&value["data"], "name")?,
            signature: value["data"]["sign"].as_str().unwrap_or_default().into(),
        })
    }
}

const QR_STATUS_POINTER: &str = "/data/code";

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
}
