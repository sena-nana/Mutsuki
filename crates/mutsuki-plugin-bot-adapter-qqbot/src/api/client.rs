use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use serde_json::{Value, json};

use crate::api::{QqMediaProvider, QqOpenApiError};
use crate::config::QqBotConfig;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

#[derive(Clone, PartialEq)]
pub struct QqHttpRequest {
    pub method: HttpMethod,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Value>,
    pub binary_body: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq)]
pub struct QqHttpResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Value,
}

impl std::fmt::Debug for QqHttpRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QqHttpRequest")
            .field("method", &self.method)
            .field("url", &redacted_url(&self.url))
            .field("headers", &redacted_headers(&self.headers))
            .field("body", &self.body.as_ref().map(crate::adapter::redact_json))
            .field(
                "binary_body_bytes",
                &self.binary_body.as_ref().map(Vec::len),
            )
            .finish()
    }
}

impl std::fmt::Debug for QqHttpResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QqHttpResponse")
            .field("status", &self.status)
            .field("headers", &redacted_headers(&self.headers))
            .field("body", &crate::adapter::redact_json(&self.body))
            .finish()
    }
}

pub trait QqHttpClient: Send {
    fn send(&mut self, request: QqHttpRequest) -> Result<QqHttpResponse, QqOpenApiError>;
}

pub trait QqCredentialProvider: Send + Sync {
    fn client_secret(&self) -> Result<String, QqOpenApiError>;
}

#[derive(Clone)]
pub struct StaticQqCredentials {
    client_secret: String,
}

impl StaticQqCredentials {
    pub fn new(client_secret: impl Into<String>) -> Self {
        Self {
            client_secret: client_secret.into(),
        }
    }
}

impl QqCredentialProvider for StaticQqCredentials {
    fn client_secret(&self) -> Result<String, QqOpenApiError> {
        if self.client_secret.is_empty() {
            Err(QqOpenApiError::CredentialsUnavailable)
        } else {
            Ok(self.client_secret.clone())
        }
    }
}

#[derive(Clone, Default)]
pub struct SharedQqCredentials {
    client_secret: Arc<Mutex<Option<String>>>,
}

impl SharedQqCredentials {
    pub fn set_client_secret(&self, secret: String) {
        *self.client_secret.lock().expect("QQBot credential mutex") = Some(secret);
    }

    pub fn clear(&self) {
        *self.client_secret.lock().expect("QQBot credential mutex") = None;
    }
}

impl QqCredentialProvider for SharedQqCredentials {
    fn client_secret(&self) -> Result<String, QqOpenApiError> {
        self.client_secret
            .lock()
            .expect("QQBot credential mutex")
            .clone()
            .filter(|secret| !secret.is_empty())
            .ok_or(QqOpenApiError::CredentialsUnavailable)
    }
}

pub trait QqIdSource: Send {
    fn next_msg_seq(&mut self) -> u64;
}

pub struct QqBotClients {
    pub http: Box<dyn QqHttpClient>,
    pub media: Option<Box<dyn QqMediaProvider>>,
    pub credentials: Arc<dyn QqCredentialProvider>,
}

impl QqBotClients {
    pub fn new(http: Box<dyn QqHttpClient>, credentials: Arc<dyn QqCredentialProvider>) -> Self {
        Self {
            http,
            media: None,
            credentials,
        }
    }

    pub fn with_media_provider(mut self, media: Box<dyn QqMediaProvider>) -> Self {
        self.media = Some(media);
        self
    }

    pub fn has_media_provider(&self) -> bool {
        self.media.is_some()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct AccessToken {
    pub token: String,
    pub expires_at_unix_secs: u64,
}

#[derive(Clone, Default)]
pub struct QqAuthManager {
    token: Arc<Mutex<Option<AccessToken>>>,
    refresh: Arc<Mutex<()>>,
}

impl QqAuthManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn invalidate(&self) {
        *self.token.lock().expect("QQBot auth mutex") = None;
    }

    pub fn bearer_token(
        &self,
        config: &QqBotConfig,
        credentials: &dyn QqCredentialProvider,
        client: &mut dyn QqHttpClient,
    ) -> Result<String, QqOpenApiError> {
        self.bearer_token_at(config, credentials, client, unix_now_secs()?)
    }

    pub(crate) fn bearer_token_at(
        &self,
        config: &QqBotConfig,
        credentials: &dyn QqCredentialProvider,
        client: &mut dyn QqHttpClient,
        now_secs: u64,
    ) -> Result<String, QqOpenApiError> {
        if let Some(token) = self.token.lock().expect("QQBot auth mutex").as_ref()
            && token.expires_at_unix_secs > now_secs + config.token_refresh_margin_secs
        {
            return Ok(token.token.clone());
        }
        let _refresh = self.refresh.lock().expect("QQBot auth refresh mutex");
        if let Some(token) = self.token.lock().expect("QQBot auth mutex").as_ref()
            && token.expires_at_unix_secs > now_secs + config.token_refresh_margin_secs
        {
            return Ok(token.token.clone());
        }
        let secret = credentials.client_secret()?;
        let mut response = None;
        let max_attempts = config.max_retry_attempts.max(1);
        for attempt in 1..=max_attempts {
            match client.send(request_json(
                HttpMethod::Post,
                config.token_url.clone(),
                json!({
                    "appId": config.app_id,
                    "clientSecret": secret,
                }),
            )) {
                Ok(candidate) if (200..300).contains(&candidate.status) => {
                    response = Some(candidate);
                    break;
                }
                Ok(candidate) => {
                    let error = QqOpenApiError::HttpStatus {
                        status: candidate.status,
                        headers: candidate.headers,
                        body: candidate.body,
                    };
                    if !error.retryable() || attempt == max_attempts {
                        return Err(error);
                    }
                    retry_sleep(config, attempt, error.retry_after_ms());
                }
                Err(error) => {
                    if !error.retryable() || attempt == max_attempts {
                        return Err(error);
                    }
                    retry_sleep(config, attempt, error.retry_after_ms());
                }
            }
        }
        let response = response
            .ok_or_else(|| QqOpenApiError::InvalidResponse("token retry exhausted".into()))?;
        let token = json_field(&response.body, "access_token")?.to_owned();
        let expires_in = response
            .body
            .get("expires_in")
            .and_then(Value::as_u64)
            .ok_or_else(|| QqOpenApiError::InvalidResponse("expires_in".into()))?;
        *self.token.lock().expect("QQBot auth mutex") = Some(AccessToken {
            token: token.clone(),
            expires_at_unix_secs: now_secs.saturating_add(expires_in),
        });
        Ok(token)
    }
}

/// Production HTTP client. Requests are executed by async reqwest on a dedicated
/// current-thread Tokio runtime so synchronous `Runner::run_batch` remains intact.
pub struct ReqwestQqHttpClient {
    tx: Option<mpsc::Sender<HttpJob>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

struct HttpJob {
    request: QqHttpRequest,
    reply: mpsc::Sender<Result<QqHttpResponse, QqOpenApiError>>,
}

impl ReqwestQqHttpClient {
    pub fn new(config: &QqBotConfig) -> Result<Self, QqOpenApiError> {
        config
            .validate()
            .map_err(|error| QqOpenApiError::InvalidPayload(error.to_string()))?;
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(config.connect_timeout_ms))
            .timeout(Duration::from_millis(config.request_timeout_ms))
            .https_only(!config.allow_insecure_transport)
            .build()
            .map_err(network_error)?;
        let body_limit = config.response_body_limit_bytes;
        let (tx, rx) = mpsc::channel::<HttpJob>();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(network_error)?;
        let worker = std::thread::Builder::new()
            .name(format!("qqbot-http-{}", config.account_id))
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    let result = runtime.block_on(send_reqwest(&client, job.request, body_limit));
                    let _ = job.reply.send(result);
                }
            })
            .map_err(network_error)?;
        Ok(Self {
            tx: Some(tx),
            worker: Some(worker),
        })
    }
}

impl QqHttpClient for ReqwestQqHttpClient {
    fn send(&mut self, request: QqHttpRequest) -> Result<QqHttpResponse, QqOpenApiError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .as_ref()
            .ok_or_else(|| QqOpenApiError::Network("HTTP worker is stopped".into()))?
            .send(HttpJob {
                request,
                reply: reply_tx,
            })
            .map_err(network_error)?;
        reply_rx.recv().map_err(network_error)?
    }
}

impl Drop for ReqwestQqHttpClient {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

async fn send_reqwest(
    client: &reqwest::Client,
    request: QqHttpRequest,
    body_limit: usize,
) -> Result<QqHttpResponse, QqOpenApiError> {
    let method = match request.method {
        HttpMethod::Get => reqwest::Method::GET,
        HttpMethod::Post => reqwest::Method::POST,
        HttpMethod::Put => reqwest::Method::PUT,
        HttpMethod::Delete => reqwest::Method::DELETE,
    };
    let mut builder = client.request(method, &request.url);
    for (key, value) in request.headers {
        builder = builder.header(&key, value);
    }
    if let Some(binary) = request.binary_body {
        builder = builder.body(binary);
    } else if let Some(body) = request.body {
        builder = builder.json(&body);
    }
    let response = builder.send().await.map_err(network_error)?;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(key, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (key.as_str().to_owned(), value.to_owned()))
        })
        .collect::<BTreeMap<_, _>>();
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(network_error)?;
        if bytes.len().saturating_add(chunk.len()) > body_limit {
            return Err(QqOpenApiError::ResponseTooLarge { limit: body_limit });
        }
        bytes.extend_from_slice(&chunk);
    }
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    Ok(QqHttpResponse {
        status,
        headers,
        body,
    })
}

pub fn authorization_header(token: &str) -> String {
    format!("QQBot {token}")
}

pub fn request_json(method: HttpMethod, url: impl Into<String>, body: Value) -> QqHttpRequest {
    QqHttpRequest {
        method,
        url: url.into(),
        headers: BTreeMap::from([("Content-Type".into(), "application/json".into())]),
        body: Some(body),
        binary_body: None,
    }
}

pub fn request_empty(method: HttpMethod, url: impl Into<String>) -> QqHttpRequest {
    QqHttpRequest {
        method,
        url: url.into(),
        headers: BTreeMap::new(),
        body: None,
        binary_body: None,
    }
}

pub fn json_field<'a>(body: &'a Value, field: &str) -> Result<&'a str, QqOpenApiError> {
    body.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| QqOpenApiError::InvalidResponse(field.into()))
}

fn unix_now_secs() -> Result<u64, QqOpenApiError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| QqOpenApiError::Clock(error.to_string()))
}

fn network_error(error: impl std::fmt::Display) -> QqOpenApiError {
    QqOpenApiError::Network(crate::adapter::redact_urls(&error.to_string()))
}

fn redacted_headers(headers: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    headers
        .iter()
        .map(|(key, value)| {
            if key.eq_ignore_ascii_case("authorization")
                || key.to_ascii_lowercase().contains("token")
                || key.to_ascii_lowercase().contains("secret")
            {
                (key.clone(), "<redacted>".into())
            } else {
                (key.clone(), value.clone())
            }
        })
        .collect()
}

fn redacted_url(value: &str) -> String {
    url::Url::parse(value)
        .ok()
        .and_then(|url| {
            Some(format!(
                "{}://{}/<path:redacted>",
                url.scheme(),
                url.host_str()?
            ))
        })
        .unwrap_or_else(|| "<url:redacted>".into())
}

fn retry_sleep(config: &QqBotConfig, attempt: u8, server_delay_ms: Option<u64>) {
    let exponent = u32::from(attempt.saturating_sub(1)).min(20);
    let configured = config
        .retry_base_delay_ms
        .saturating_mul(1_u64 << exponent)
        .min(config.retry_max_delay_ms);
    let delay = server_delay_ms
        .unwrap_or(configured)
        .min(config.retry_max_delay_ms);
    if delay > 0 {
        std::thread::sleep(Duration::from_millis(delay));
    }
}

#[cfg(test)]
mod production_tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn production_http_enforces_response_body_limit() {
        let (url, server) = serve_once(|mut stream| {
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 32\r\nConnection: close\r\n\r\n01234567890123456789012345678901",
                )
                .unwrap();
        });
        let mut config = local_config(&url);
        config.response_body_limit_bytes = 8;
        let mut client = ReqwestQqHttpClient::new(&config).unwrap();

        let error = client
            .send(request_empty(HttpMethod::Get, url))
            .unwrap_err();

        assert!(matches!(
            error,
            QqOpenApiError::ResponseTooLarge { limit: 8 }
        ));
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn production_http_reports_request_timeout_as_network_error() {
        let (url, server) = serve_once(|mut stream| {
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).unwrap();
            std::thread::sleep(Duration::from_millis(200));
        });
        let mut config = local_config(&url);
        config.request_timeout_ms = 25;
        let mut client = ReqwestQqHttpClient::new(&config).unwrap();

        let error = client
            .send(request_empty(HttpMethod::Get, url))
            .unwrap_err();

        assert!(matches!(error, QqOpenApiError::Network(_)));
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn shared_auth_manager_singleflights_concurrent_token_refresh() {
        let auth = QqAuthManager::new();
        let credentials = Arc::new(StaticQqCredentials::new("CLIENT_SECRET"));
        let config = QqBotConfig::new("singleflight", "APP_ID");
        let requests = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = std::sync::mpsc::channel();

        let first = {
            let auth = auth.clone();
            let credentials = credentials.clone();
            let config = config.clone();
            let requests = requests.clone();
            std::thread::spawn(move || {
                let mut client = SlowTokenClient {
                    requests,
                    started: Some(started_tx),
                };
                auth.bearer_token_at(&config, credentials.as_ref(), &mut client, 1_000)
                    .unwrap()
            })
        };
        started_rx.recv().unwrap();
        let second = {
            let auth = auth.clone();
            let credentials = credentials.clone();
            let config = config.clone();
            let requests = requests.clone();
            std::thread::spawn(move || {
                let mut client = SlowTokenClient {
                    requests,
                    started: None,
                };
                auth.bearer_token_at(&config, credentials.as_ref(), &mut client, 1_000)
                    .unwrap()
            })
        };

        assert_eq!(first.join().unwrap(), "SINGLEFLIGHT_TOKEN");
        assert_eq!(second.join().unwrap(), "SINGLEFLIGHT_TOKEN");
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    struct SlowTokenClient {
        requests: Arc<AtomicUsize>,
        started: Option<std::sync::mpsc::Sender<()>>,
    }

    impl QqHttpClient for SlowTokenClient {
        fn send(&mut self, _request: QqHttpRequest) -> Result<QqHttpResponse, QqOpenApiError> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            if let Some(started) = self.started.take() {
                let _ = started.send(());
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(QqHttpResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: json!({
                    "access_token": "SINGLEFLIGHT_TOKEN",
                    "expires_in": 7200
                }),
            })
        }
    }

    fn local_config(url: &str) -> QqBotConfig {
        let mut config = QqBotConfig::new("production-http-test", "APP_ID");
        config.allow_insecure_transport = true;
        config.token_url = url.into();
        config.openapi_base_url = url.into();
        config.connect_timeout_ms = 500;
        config
    }

    fn serve_once(
        handler: impl FnOnce(std::net::TcpStream) + Send + 'static,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handler(stream);
        });
        (format!("http://{address}/test"), server)
    }
}
