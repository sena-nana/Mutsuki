use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use url::Url;

use super::{
    BilibiliCredentialStore, BilibiliError, BilibiliItem, BilibiliPollKind, BilibiliProfile,
    BilibiliQrCode, BilibiliQrPoll, BilibiliTransport, ResolvedLinkCard, SharedBilibiliCredential,
    ensure_bilibili_domain,
};

const OFFICIAL_API_BASE: &str = "https://member.bilibili.com";
const OFFICIAL_OAUTH_BASE: &str = "https://api.bilibili.com";
const LIVE_SCOPE: &str = "LIVE_ROOM_DATA";
const VIDEO_SCOPE: &str = "ARC_BASE";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BilibiliOpenPlatformCredential {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    pub scopes: Vec<String>,
}

impl fmt::Debug for BilibiliOpenPlatformCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BilibiliOpenPlatformCredential")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .field("scopes", &self.scopes)
            .finish()
    }
}

impl BilibiliOpenPlatformCredential {
    pub fn parse(raw: &str) -> Result<Self, BilibiliError> {
        let credential: Self = serde_json::from_str(raw).map_err(|_| {
            BilibiliError::OpenPlatformCredentialInvalid(
                "oauth credential secret must be valid JSON".into(),
            )
        })?;
        if credential.access_token.trim().is_empty()
            || credential.refresh_token.trim().is_empty()
            || credential.expires_at <= 0
        {
            return Err(BilibiliError::OpenPlatformCredentialInvalid(
                "oauth credential requires access_token, refresh_token and expires_at".into(),
            ));
        }
        Ok(credential)
    }

    fn require_scope(&self, scope: &str) -> Result<(), BilibiliError> {
        if self.scopes.iter().any(|candidate| candidate == scope) {
            Ok(())
        } else {
            Err(BilibiliError::OpenPlatformPermissionDenied {
                code: 127011,
                scope: scope.into(),
                request_id: None,
            })
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenPlatformHttpMethod {
    Get,
    Post,
}

pub struct BilibiliOpenPlatformHttpRequest {
    pub method: OpenPlatformHttpMethod,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<String>,
    pub form: Option<BTreeMap<String, String>>,
}

pub struct BilibiliOpenPlatformHttpResponse {
    pub status: u16,
    pub body: Value,
}

pub trait BilibiliOpenPlatformHttpClient: Send {
    fn execute(
        &mut self,
        request: BilibiliOpenPlatformHttpRequest,
    ) -> Result<BilibiliOpenPlatformHttpResponse, BilibiliError>;

    fn download(&mut self, url: &str, max_bytes: usize) -> Result<Vec<u8>, BilibiliError>;
}

pub trait BilibiliOpenPlatformRequestContext: Send {
    fn timestamp_seconds(&mut self) -> Result<i64, BilibiliError>;
    fn nonce(&mut self) -> String;
}

struct SystemRequestContext;

impl BilibiliOpenPlatformRequestContext for SystemRequestContext {
    fn timestamp_seconds(&mut self) -> Result<i64, BilibiliError> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .map_err(|error| BilibiliError::Transport(error.to_string()))
    }

    fn nonce(&mut self) -> String {
        format!("{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..))
    }
}

struct ReqwestOpenPlatformHttpClient {
    client: Option<Client>,
    timeout: Duration,
}

impl ReqwestOpenPlatformHttpClient {
    fn new(timeout: Duration) -> Self {
        Self {
            client: None,
            timeout,
        }
    }

    fn client(&mut self) -> Result<&Client, BilibiliError> {
        if self.client.is_none() {
            self.client = Some(
                Client::builder()
                    .timeout(self.timeout)
                    .user_agent("MutsukiBot/0.1 BilibiliOpenPlatform")
                    .build()
                    .map_err(|error| BilibiliError::Transport(error.to_string()))?,
            );
        }
        Ok(self.client.as_ref().expect("client initialized"))
    }
}

impl BilibiliOpenPlatformHttpClient for ReqwestOpenPlatformHttpClient {
    fn execute(
        &mut self,
        request: BilibiliOpenPlatformHttpRequest,
    ) -> Result<BilibiliOpenPlatformHttpResponse, BilibiliError> {
        let mut builder = match request.method {
            OpenPlatformHttpMethod::Get => self.client()?.get(&request.url),
            OpenPlatformHttpMethod::Post => self.client()?.post(&request.url),
        };
        for (name, value) in request.headers {
            builder = builder.header(name, value);
        }
        if let Some(form) = request.form {
            let mut encoded = url::form_urlencoded::Serializer::new(String::new());
            for (name, value) in form {
                encoded.append_pair(&name, &value);
            }
            builder = builder.body(encoded.finish());
        } else if let Some(body) = request.body {
            builder = builder.body(body);
        }
        let response = builder
            .send()
            .map_err(|error| BilibiliError::Transport(error.to_string()))?;
        let status = response.status().as_u16();
        let body = response.json().map_err(|_| {
            BilibiliError::InvalidResponse("Open Platform returned non-JSON".into())
        })?;
        Ok(BilibiliOpenPlatformHttpResponse { status, body })
    }

    fn download(&mut self, url: &str, max_bytes: usize) -> Result<Vec<u8>, BilibiliError> {
        ensure_bilibili_domain(url)?;
        let bytes = self
            .client()?
            .get(url)
            .send()
            .map_err(|error| BilibiliError::Transport(error.to_string()))?
            .bytes()
            .map_err(|error| BilibiliError::Transport(error.to_string()))?;
        if bytes.len() > max_bytes {
            return Err(BilibiliError::InvalidResponse(
                "media exceeds configured limit".into(),
            ));
        }
        Ok(bytes.to_vec())
    }
}

pub struct ReqwestBilibiliOpenPlatformTransport {
    client_id: String,
    authorized_uid: u64,
    app_secret: SharedBilibiliCredential,
    oauth_credential: SharedBilibiliCredential,
    oauth_credential_key: String,
    credential_store: Arc<dyn BilibiliCredentialStore>,
    http: Box<dyn BilibiliOpenPlatformHttpClient>,
    request_context: Box<dyn BilibiliOpenPlatformRequestContext>,
    api_base: String,
    oauth_base: String,
}

impl fmt::Debug for ReqwestBilibiliOpenPlatformTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReqwestBilibiliOpenPlatformTransport")
            .field("client_id", &self.client_id)
            .field("authorized_uid", &self.authorized_uid)
            .field("credentials", &"[REDACTED]")
            .finish()
    }
}

impl ReqwestBilibiliOpenPlatformTransport {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client_id: impl Into<String>,
        authorized_uid: u64,
        app_secret: SharedBilibiliCredential,
        oauth_credential: SharedBilibiliCredential,
        oauth_credential_key: impl Into<String>,
        credential_store: Arc<dyn BilibiliCredentialStore>,
        timeout: Duration,
    ) -> Self {
        Self {
            client_id: client_id.into(),
            authorized_uid,
            app_secret,
            oauth_credential,
            oauth_credential_key: oauth_credential_key.into(),
            credential_store,
            http: Box::new(ReqwestOpenPlatformHttpClient::new(timeout)),
            request_context: Box::new(SystemRequestContext),
            api_base: OFFICIAL_API_BASE.into(),
            oauth_base: OFFICIAL_OAUTH_BASE.into(),
        }
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    fn with_test_boundaries(
        client_id: impl Into<String>,
        authorized_uid: u64,
        app_secret: SharedBilibiliCredential,
        oauth_credential: SharedBilibiliCredential,
        oauth_credential_key: impl Into<String>,
        credential_store: Arc<dyn BilibiliCredentialStore>,
        http: Box<dyn BilibiliOpenPlatformHttpClient>,
        request_context: Box<dyn BilibiliOpenPlatformRequestContext>,
    ) -> Self {
        Self {
            client_id: client_id.into(),
            authorized_uid,
            app_secret,
            oauth_credential,
            oauth_credential_key: oauth_credential_key.into(),
            credential_store,
            http,
            request_context,
            api_base: "https://member.bilibili.test".into(),
            oauth_base: "https://api.bilibili.test".into(),
        }
    }

    fn credential(&self) -> Result<BilibiliOpenPlatformCredential, BilibiliError> {
        BilibiliOpenPlatformCredential::parse(&self.oauth_credential.get_named("oauth_credential")?)
    }

    fn refresh(
        &mut self,
        current: &BilibiliOpenPlatformCredential,
        now: i64,
    ) -> Result<BilibiliOpenPlatformCredential, BilibiliError> {
        let app_secret = self.app_secret.get_named("app_secret")?;
        let mut form = BTreeMap::new();
        form.insert("client_id".into(), self.client_id.clone());
        form.insert("client_secret".into(), app_secret);
        form.insert("grant_type".into(), "refresh_token".into());
        form.insert("refresh_token".into(), current.refresh_token.clone());
        let response = self.http.execute(BilibiliOpenPlatformHttpRequest {
            method: OpenPlatformHttpMethod::Post,
            url: join_url(&self.oauth_base, "/x/account-oauth2/v1/refresh_token")?,
            headers: BTreeMap::from([(
                "content-type".into(),
                "application/x-www-form-urlencoded".into(),
            )]),
            body: None,
            form: Some(form),
        })?;
        let data = success_data(response, "OAUTH_REFRESH")?;
        let access_token = required_string(&data, "access_token")?;
        let refresh_token = required_string(&data, "refresh_token")?;
        let expires_in = data
            .get("expires_in")
            .and_then(Value::as_i64)
            .ok_or_else(|| BilibiliError::InvalidResponse("expires_in".into()))?;
        let expires_at = if expires_in > now.saturating_add(300) {
            expires_in
        } else {
            now.saturating_add(expires_in)
        };
        let next = BilibiliOpenPlatformCredential {
            access_token,
            refresh_token,
            expires_at,
            scopes: data
                .get("scopes")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToOwned::to_owned)
                        .collect()
                })
                .unwrap_or_else(|| current.scopes.clone()),
        };
        let encoded = serde_json::to_string(&next)
            .map_err(|error| BilibiliError::OpenPlatformCredentialInvalid(error.to_string()))?;
        self.credential_store
            .rotate(&self.oauth_credential_key, encoded.clone())
            .map_err(|_| {
                BilibiliError::OpenPlatformCredentialInvalid(
                    "Host credential rotation failed".into(),
                )
            })?;
        self.oauth_credential.set(encoded);
        Ok(next)
    }

    fn credential_for_scope(
        &mut self,
        scope: &str,
    ) -> Result<BilibiliOpenPlatformCredential, BilibiliError> {
        let now = self.request_context.timestamp_seconds()?;
        let mut credential = self.credential()?;
        credential.require_scope(scope)?;
        if credential.expires_at <= now.saturating_add(60) {
            credential = self.refresh(&credential, now)?;
            credential.require_scope(scope)?;
        }
        Ok(credential)
    }

    fn signed_request(
        &mut self,
        method: OpenPlatformHttpMethod,
        path_and_query: &str,
        body: &str,
        scope: &str,
    ) -> Result<Value, BilibiliError> {
        let mut credential = self.credential_for_scope(scope)?;
        let mut retried = false;
        loop {
            let timestamp = self.request_context.timestamp_seconds()?;
            let nonce = self.request_context.nonce();
            let app_secret = self.app_secret.get_named("app_secret")?;
            let headers = open_platform_signed_headers(
                &self.client_id,
                &app_secret,
                &credential.access_token,
                body,
                timestamp,
                &nonce,
            )?;
            let response = self.http.execute(BilibiliOpenPlatformHttpRequest {
                method,
                url: join_url(&self.api_base, path_and_query)?,
                headers,
                body: (method == OpenPlatformHttpMethod::Post).then(|| body.to_owned()),
                form: None,
            })?;
            match success_data(response, scope) {
                Ok(data) => return Ok(data),
                Err(BilibiliError::OpenPlatformOAuthExpired { .. }) if !retried => {
                    credential = self.refresh(&credential, timestamp)?;
                    credential.require_scope(scope)?;
                    retried = true;
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn poll_live(&mut self) -> Result<Vec<BilibiliItem>, BilibiliError> {
        let data = self.signed_request(
            OpenPlatformHttpMethod::Post,
            "/arcopen/fn/live/room/detail",
            "",
            LIVE_SCOPE,
        )?;
        let info = data
            .get("info")
            .ok_or_else(|| BilibiliError::InvalidResponse("data.info".into()))?;
        let show = &info["show"];
        let room_id = show
            .get("room_id")
            .and_then(Value::as_u64)
            .ok_or_else(|| BilibiliError::InvalidResponse("show.room_id".into()))?;
        let live = info["status"]["live_status"].as_i64().unwrap_or_default() == 1;
        Ok(vec![BilibiliItem {
            id: live.to_string(),
            title: show["title"].as_str().unwrap_or("直播").into(),
            url: format!("https://live.bilibili.com/{room_id}"),
            image_url: show["cover"]
                .as_str()
                .or_else(|| show["key_frame"].as_str())
                .and_then(normalize_https_url),
        }])
    }

    fn poll_video(&mut self) -> Result<Vec<BilibiliItem>, BilibiliError> {
        let data = self.signed_request(
            OpenPlatformHttpMethod::Get,
            "/arcopen/fn/archive/viewlist?pn=1&ps=50&status=pubed",
            "",
            VIDEO_SCOPE,
        )?;
        Ok(data["list"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|item| {
                let id = item.get("resource_id")?.as_str()?.to_owned();
                let url = item["video_info"]["share_url"]
                    .as_str()
                    .and_then(normalize_https_url)
                    .unwrap_or_else(|| format!("https://www.bilibili.com/video/{id}"));
                Some(BilibiliItem {
                    id,
                    title: item["title"].as_str().unwrap_or("新投稿").into(),
                    url,
                    image_url: item["cover"].as_str().and_then(normalize_https_url),
                })
            })
            .collect())
    }
}

impl BilibiliTransport for ReqwestBilibiliOpenPlatformTransport {
    fn poll(
        &mut self,
        kind: &BilibiliPollKind,
        uid: u64,
    ) -> Result<Vec<BilibiliItem>, BilibiliError> {
        if uid != self.authorized_uid {
            return Err(BilibiliError::OpenPlatformPermissionDenied {
                code: 127003,
                scope: "AUTHORIZED_ACCOUNT".into(),
                request_id: None,
            });
        }
        match kind {
            BilibiliPollKind::Live => self.poll_live(),
            BilibiliPollKind::Video => self.poll_video(),
            BilibiliPollKind::Dynamic => Err(BilibiliError::OpenPlatformUnsupported(
                "poll/dynamic".into(),
            )),
        }
    }

    fn resolve(&mut self, _url: &str) -> Result<ResolvedLinkCard, BilibiliError> {
        Err(BilibiliError::OpenPlatformUnsupported(
            "link/resolve".into(),
        ))
    }

    fn download(&mut self, url: &str, max_bytes: usize) -> Result<Vec<u8>, BilibiliError> {
        self.http.download(url, max_bytes)
    }

    fn qr_start(&mut self) -> Result<BilibiliQrCode, BilibiliError> {
        Err(BilibiliError::OpenPlatformUnsupported(
            "Cookie QR login".into(),
        ))
    }

    fn qr_poll(&mut self, _key: &str) -> Result<BilibiliQrPoll, BilibiliError> {
        Err(BilibiliError::OpenPlatformUnsupported(
            "Cookie QR login".into(),
        ))
    }

    fn profile(&mut self, _uid: u64) -> Result<BilibiliProfile, BilibiliError> {
        Err(BilibiliError::OpenPlatformUnsupported(
            "Cookie signature verification".into(),
        ))
    }
}

pub fn open_platform_signed_headers(
    client_id: &str,
    app_secret: &str,
    access_token: &str,
    body: &str,
    timestamp: i64,
    nonce: &str,
) -> Result<BTreeMap<String, String>, BilibiliError> {
    let content_md5 = format!("{:x}", md5::compute(body.as_bytes()));
    let signed = BTreeMap::from([
        ("x-bili-accesskeyid".into(), client_id.to_owned()),
        ("x-bili-content-md5".into(), content_md5),
        ("x-bili-signature-method".into(), "HMAC-SHA256".into()),
        ("x-bili-signature-nonce".into(), nonce.to_owned()),
        ("x-bili-signature-version".into(), "2.0".into()),
        ("x-bili-timestamp".into(), timestamp.to_string()),
    ]);
    let signable = signed
        .iter()
        .map(|(name, value)| format!("{name}:{value}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut mac = Hmac::<Sha256>::new_from_slice(app_secret.as_bytes()).map_err(|_| {
        BilibiliError::OpenPlatformCredentialInvalid("app_secret is invalid".into())
    })?;
    mac.update(signable.as_bytes());
    let authorization = hex_lower(&mac.finalize().into_bytes());
    let mut headers = signed;
    headers.insert("accept".into(), "application/json".into());
    headers.insert("content-type".into(), "application/json".into());
    headers.insert("access-token".into(), access_token.to_owned());
    headers.insert("authorization".into(), authorization);
    Ok(headers)
}

fn success_data(
    response: BilibiliOpenPlatformHttpResponse,
    scope: &str,
) -> Result<Value, BilibiliError> {
    let request_id = response.body["request_id"].as_str().map(ToOwned::to_owned);
    let code = response.body["code"].as_i64().ok_or_else(|| {
        BilibiliError::InvalidResponse(format!(
            "Open Platform HTTP {} omitted code",
            response.status
        ))
    })?;
    if response.status == 429 || matches!(code, 127009 | 127306) {
        return Err(BilibiliError::RateLimited);
    }
    match code {
        0 => Ok(response.body.get("data").cloned().unwrap_or(Value::Null)),
        127001 => Err(BilibiliError::OpenPlatformOAuthExpired { request_id }),
        4002 | 4003 | 4004 | 4005 | 4006 | 4008 | 127002 | 127010 => {
            Err(BilibiliError::OpenPlatformSignatureRejected { code, request_id })
        }
        127000 | 127003..=127008 | 127011 | 127304 | 127305 => {
            Err(BilibiliError::OpenPlatformPermissionDenied {
                code,
                scope: scope.into(),
                request_id,
            })
        }
        122007 => Err(BilibiliError::OpenPlatformOAuthExpired { request_id }),
        _ => Err(BilibiliError::OpenPlatformApi {
            code,
            message: "official API rejected the request".into(),
            request_id,
        }),
    }
}

fn required_string(value: &Value, field: &str) -> Result<String, BilibiliError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| BilibiliError::InvalidResponse(field.into()))
}

fn join_url(base: &str, path_and_query: &str) -> Result<String, BilibiliError> {
    let mut url =
        Url::parse(base).map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
    let (path, query) = path_and_query
        .split_once('?')
        .map_or((path_and_query, None), |(path, query)| (path, Some(query)));
    url.set_path(path);
    url.set_query(query);
    Ok(url.into())
}

fn normalize_https_url(value: &str) -> Option<String> {
    if value.starts_with("https://") {
        Some(value.into())
    } else if let Some(value) = value.strip_prefix("http://") {
        Some(format!("https://{value}"))
    } else if value.starts_with("//") {
        Some(format!("https:{value}"))
    } else {
        None
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use super::*;

    #[derive(Default)]
    struct RecordingStore(Mutex<Vec<(String, String)>>);

    impl BilibiliCredentialStore for RecordingStore {
        fn rotate(&self, key: &str, credential: String) -> Result<(), String> {
            self.0.lock().unwrap().push((key.into(), credential));
            Ok(())
        }
    }

    struct FixedContext {
        timestamps: VecDeque<i64>,
        sequence: u64,
    }

    impl BilibiliOpenPlatformRequestContext for FixedContext {
        fn timestamp_seconds(&mut self) -> Result<i64, BilibiliError> {
            Ok(self.timestamps.pop_front().unwrap_or(1_700_000_000))
        }

        fn nonce(&mut self) -> String {
            self.sequence += 1;
            format!("nonce-{}", self.sequence)
        }
    }

    #[derive(Default)]
    struct FakeHttpState {
        requests: Vec<BilibiliOpenPlatformHttpRequest>,
        responses: VecDeque<BilibiliOpenPlatformHttpResponse>,
    }

    struct FakeHttp(Arc<Mutex<FakeHttpState>>);

    impl BilibiliOpenPlatformHttpClient for FakeHttp {
        fn execute(
            &mut self,
            request: BilibiliOpenPlatformHttpRequest,
        ) -> Result<BilibiliOpenPlatformHttpResponse, BilibiliError> {
            let mut state = self.0.lock().unwrap();
            state.requests.push(request);
            state
                .responses
                .pop_front()
                .ok_or_else(|| BilibiliError::Transport("fake response exhausted".into()))
        }

        fn download(&mut self, _url: &str, _max_bytes: usize) -> Result<Vec<u8>, BilibiliError> {
            Ok(Vec::new())
        }
    }

    fn shared(value: &str) -> SharedBilibiliCredential {
        let shared = SharedBilibiliCredential::default();
        shared.set(value.into());
        shared
    }

    fn credential(expires_at: i64) -> String {
        serde_json::to_string(&BilibiliOpenPlatformCredential {
            access_token: "ACCESS_TOKEN_SECRET".into(),
            refresh_token: "REFRESH_TOKEN_SECRET".into(),
            expires_at,
            scopes: vec![LIVE_SCOPE.into(), VIDEO_SCOPE.into()],
        })
        .unwrap()
    }

    #[test]
    fn signer_matches_stable_hmac_vector_and_redacts_debug() {
        let headers =
            open_platform_signed_headers("client", "secret", "token", "", 1_700_000_000, "nonce")
                .unwrap();
        assert_eq!(
            headers["authorization"],
            "067cb46ddf6763508214215066d004ad617c757193fac465dc9b498234b51f97"
        );
        assert_eq!(
            headers["x-bili-content-md5"],
            "d41d8cd98f00b204e9800998ecf8427e"
        );
        let debug = format!(
            "{:?}",
            BilibiliOpenPlatformCredential::parse(&credential(2)).unwrap()
        );
        assert!(!debug.contains("ACCESS_TOKEN_SECRET"));
        assert!(!debug.contains("REFRESH_TOKEN_SECRET"));
    }

    #[test]
    fn official_live_and_video_reuse_only_supported_poll_protocols() {
        let state = Arc::new(Mutex::new(FakeHttpState {
            requests: Vec::new(),
            responses: VecDeque::from([
                BilibiliOpenPlatformHttpResponse {
                    status: 200,
                    body: json!({"code":0,"request_id":"live-request","data":{"info":{"show":{"cover":"http://i0.hdslb.com/cover.jpg","room_id":42,"title":"Live"},"status":{"live_status":1}}}}),
                },
                BilibiliOpenPlatformHttpResponse {
                    status: 200,
                    body: json!({"code":0,"request_id":"video-request","data":{"list":[{"resource_id":"BV1TEST","title":"Video","cover":"//i0.hdslb.com/video.jpg","video_info":{"share_url":"https://www.bilibili.com/video/BV1TEST"}}]}}),
                },
            ]),
        }));
        let mut transport = ReqwestBilibiliOpenPlatformTransport::with_test_boundaries(
            "client",
            7,
            shared("APP_SECRET"),
            shared(&credential(1_800_000_000)),
            "OAUTH",
            Arc::new(RecordingStore::default()),
            Box::new(FakeHttp(state.clone())),
            Box::new(FixedContext {
                timestamps: VecDeque::from([
                    1_700_000_000,
                    1_700_000_001,
                    1_700_000_002,
                    1_700_000_003,
                ]),
                sequence: 0,
            }),
        );
        let live = transport.poll(&BilibiliPollKind::Live, 7).unwrap();
        let videos = transport.poll(&BilibiliPollKind::Video, 7).unwrap();
        assert_eq!(live[0].id, "true");
        assert_eq!(live[0].url, "https://live.bilibili.com/42");
        assert_eq!(videos[0].id, "BV1TEST");
        assert!(matches!(
            transport.poll(&BilibiliPollKind::Dynamic, 7),
            Err(BilibiliError::OpenPlatformUnsupported(_))
        ));
        let state = state.lock().unwrap();
        assert_eq!(state.requests.len(), 2);
        assert_eq!(state.requests[0].method, OpenPlatformHttpMethod::Post);
        assert!(
            state.requests[0]
                .url
                .ends_with("/arcopen/fn/live/room/detail")
        );
        assert!(state.requests[1].url.contains("status=pubed"));
        for request in &state.requests {
            assert_eq!(request.headers["access-token"], "ACCESS_TOKEN_SECRET");
            assert!(!request.headers.values().any(|value| value == "APP_SECRET"));
            assert!(
                !request
                    .headers
                    .values()
                    .any(|value| value == "REFRESH_TOKEN_SECRET")
            );
        }
    }

    #[test]
    fn expired_oauth_bundle_refreshes_atomically_before_signed_request() {
        let state = Arc::new(Mutex::new(FakeHttpState {
            requests: Vec::new(),
            responses: VecDeque::from([
                BilibiliOpenPlatformHttpResponse {
                    status: 200,
                    body: json!({"code":0,"data":{"access_token":"NEW_ACCESS","refresh_token":"NEW_REFRESH","expires_in":7200}}),
                },
                BilibiliOpenPlatformHttpResponse {
                    status: 200,
                    body: json!({"code":0,"request_id":"video-request","data":{"list":[]}}),
                },
            ]),
        }));
        let store = Arc::new(RecordingStore::default());
        let mut transport = ReqwestBilibiliOpenPlatformTransport::with_test_boundaries(
            "client",
            7,
            shared("APP_SECRET"),
            shared(&credential(1_699_999_999)),
            "OAUTH",
            store.clone(),
            Box::new(FakeHttp(state.clone())),
            Box::new(FixedContext {
                timestamps: VecDeque::from([1_700_000_000, 1_700_000_001]),
                sequence: 0,
            }),
        );
        transport.poll(&BilibiliPollKind::Video, 7).unwrap();
        let rotations = store.0.lock().unwrap();
        assert_eq!(rotations.len(), 1);
        assert_eq!(rotations[0].0, "OAUTH");
        let rotated = BilibiliOpenPlatformCredential::parse(&rotations[0].1).unwrap();
        assert_eq!(rotated.access_token, "NEW_ACCESS");
        assert_eq!(rotated.expires_at, 1_700_007_200);
        let state = state.lock().unwrap();
        assert_eq!(
            state.requests[0].form.as_ref().unwrap()["refresh_token"],
            "REFRESH_TOKEN_SECRET"
        );
        assert_eq!(state.requests[1].headers["access-token"], "NEW_ACCESS");
    }

    #[test]
    fn permission_and_signature_failures_keep_request_id_and_distinct_models() {
        let permission = success_data(
            BilibiliOpenPlatformHttpResponse {
                status: 200,
                body: json!({"code":127011,"message":"not granted","request_id":"permission-id"}),
            },
            VIDEO_SCOPE,
        )
        .unwrap_err();
        assert!(
            matches!(permission, BilibiliError::OpenPlatformPermissionDenied { request_id: Some(id), .. } if id == "permission-id")
        );
        let signature = success_data(
            BilibiliOpenPlatformHttpResponse {
                status: 200,
                body: json!({"code":4002,"message":"bad sign","request_id":"signature-id"}),
            },
            VIDEO_SCOPE,
        )
        .unwrap_err();
        assert!(
            matches!(signature, BilibiliError::OpenPlatformSignatureRejected { request_id: Some(id), .. } if id == "signature-id")
        );
    }
}
