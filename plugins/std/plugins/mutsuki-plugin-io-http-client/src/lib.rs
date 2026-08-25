// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(clippy::unnecessary_map_or)]

use std::collections::BTreeMap;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use mutsuki_protocol_http::{
    EFFECT_REQUEST, HttpErrorCode, HttpMethod, HttpRequest, HttpResponse, HttpResponseMetadata,
    REQUEST, RESPONSE_BODY_SCHEMA,
};
use mutsuki_runtime_contracts::{
    CompletionBatch, DomainEvent, EntryCompletion, ExecutionClass, InvocationMode, ReadPlan,
    RunnerBatchCapability, RunnerConcurrency, RunnerContext, RunnerDescriptor, RunnerMode,
    RunnerPurity, RunnerResult, RunnerSideEffect, RuntimeError, ScalarValue, Task, TaskOutcome,
    WorkBatch,
};
use mutsuki_runtime_core::{
    AsyncBatchHandler, AsyncCompletionFuture, RuntimeFailure, RuntimeResult,
};
use mutsuki_runtime_sdk::{
    PluginBuilder, ProtocolDescriptorBuilder, ResourceRegistryGateway, RunnerDescriptorBuilder,
    RuntimeClientRef, TaskAwaitRunnerAdapter,
};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use url::Url;

pub const PLUGIN_ID: &str = "mutsuki.std.io.http_client";
pub const RUNNER_ID: &str = "mutsuki.std.io.http_client.runner";
pub const EFFECT_RUNNER_ID: &str = "effect.mutsuki.std.io.http_client.runner";
pub const HTTP_REQUEST_PROTOCOL: &str = REQUEST;
pub const EFFECT_HTTP_REQUEST_PROTOCOL: &str = EFFECT_REQUEST;

const DEFAULT_MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;
const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_HEADER_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_IDLE_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_TOTAL_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_MAX_REDIRECTS: u8 = 5;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HttpClientConfig {
    pub response_provider_id: String,
    pub domain_allowlist: Vec<String>,
    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: u64,
    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    #[serde(default = "default_header_timeout_ms")]
    pub header_timeout_ms: u64,
    #[serde(default = "default_idle_timeout_ms")]
    pub idle_timeout_ms: u64,
    #[serde(default = "default_total_timeout_ms")]
    pub total_timeout_ms: u64,
    #[serde(default = "default_max_redirects")]
    pub max_redirects: u8,
}

impl HttpClientConfig {
    /// Validates the resource owner, domain allowlist, size budget, and timeout ordering.
    ///
    /// # Errors
    ///
    /// Returns an error when any network or resource safety limit is absent or inconsistent.
    pub fn validate(&self) -> Result<(), String> {
        if self.response_provider_id.trim().is_empty() {
            return Err("response_provider_id is required".into());
        }
        if self.domain_allowlist.is_empty() {
            return Err("domain_allowlist must contain at least one domain".into());
        }
        for domain in &self.domain_allowlist {
            let normalized = normalize_domain(domain);
            if normalized.is_empty()
                || normalized.contains('/')
                || normalized.contains(':')
                || normalized.parse::<IpAddr>().is_ok()
            {
                return Err(format!("invalid allowlisted domain `{domain}`"));
            }
        }
        if self.max_response_bytes == 0 {
            return Err("max_response_bytes must be greater than zero".into());
        }
        if self.connect_timeout_ms == 0
            || self.header_timeout_ms == 0
            || self.idle_timeout_ms == 0
            || self.total_timeout_ms == 0
        {
            return Err("HTTP timeouts must be greater than zero".into());
        }
        if self.connect_timeout_ms > self.total_timeout_ms
            || self.header_timeout_ms > self.total_timeout_ms
            || self.idle_timeout_ms > self.total_timeout_ms
        {
            return Err("connect/header/idle timeout must not exceed total_timeout_ms".into());
        }
        Ok(())
    }

    fn effective_limits(&self, request: &HttpRequest) -> EffectiveLimits {
        EffectiveLimits {
            max_response_bytes: request
                .limits
                .max_response_bytes
                .unwrap_or(self.max_response_bytes)
                .min(self.max_response_bytes),
            connect_timeout: Duration::from_millis(
                request
                    .limits
                    .connect_timeout_ms
                    .unwrap_or(self.connect_timeout_ms)
                    .min(self.connect_timeout_ms),
            ),
            header_timeout: Duration::from_millis(
                request
                    .limits
                    .header_timeout_ms
                    .unwrap_or(self.header_timeout_ms)
                    .min(self.header_timeout_ms),
            ),
            idle_timeout: Duration::from_millis(
                request
                    .limits
                    .idle_timeout_ms
                    .unwrap_or(self.idle_timeout_ms)
                    .min(self.idle_timeout_ms),
            ),
            total_timeout: Duration::from_millis(
                request
                    .limits
                    .total_timeout_ms
                    .unwrap_or(self.total_timeout_ms)
                    .min(self.total_timeout_ms),
            ),
            max_redirects: request
                .limits
                .max_redirects
                .unwrap_or(self.max_redirects)
                .min(self.max_redirects),
        }
    }
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            response_provider_id: String::new(),
            domain_allowlist: Vec::new(),
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            connect_timeout_ms: DEFAULT_CONNECT_TIMEOUT_MS,
            header_timeout_ms: DEFAULT_HEADER_TIMEOUT_MS,
            idle_timeout_ms: DEFAULT_IDLE_TIMEOUT_MS,
            total_timeout_ms: DEFAULT_TOTAL_TIMEOUT_MS,
            max_redirects: DEFAULT_MAX_REDIRECTS,
        }
    }
}

const fn default_max_response_bytes() -> u64 {
    DEFAULT_MAX_RESPONSE_BYTES
}

const fn default_connect_timeout_ms() -> u64 {
    DEFAULT_CONNECT_TIMEOUT_MS
}

const fn default_header_timeout_ms() -> u64 {
    DEFAULT_HEADER_TIMEOUT_MS
}

const fn default_idle_timeout_ms() -> u64 {
    DEFAULT_IDLE_TIMEOUT_MS
}

const fn default_total_timeout_ms() -> u64 {
    DEFAULT_TOTAL_TIMEOUT_MS
}

const fn default_max_redirects() -> u8 {
    DEFAULT_MAX_REDIRECTS
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpGatewayError {
    pub code: HttpErrorCode,
    pub message: String,
    pub evidence: BTreeMap<String, String>,
}

impl HttpGatewayError {
    fn new(code: HttpErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            evidence: BTreeMap::new(),
        }
    }

    fn evidence(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.evidence.insert(key.into(), value.into());
        self
    }
}

impl std::fmt::Display for HttpGatewayError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for HttpGatewayError {}

#[derive(Clone, Debug)]
pub struct FetchedHttpResponse {
    pub metadata: HttpResponseMetadata,
    pub body: Vec<u8>,
    pub peak_buffered_bytes: u64,
}

#[async_trait]
pub trait HttpGateway: Send + Sync {
    async fn execute(
        &self,
        request: HttpRequest,
        request_body: Option<Vec<u8>>,
    ) -> Result<FetchedHttpResponse, HttpGatewayError>;
}

#[async_trait]
trait DnsGateway: Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, HttpGatewayError>;
}

struct TokioDnsGateway;

#[async_trait]
impl DnsGateway for TokioDnsGateway {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, HttpGatewayError> {
        let addresses = tokio::net::lookup_host((host, port))
            .await
            .map_err(|error| HttpGatewayError::new(HttpErrorCode::DnsFailed, error.to_string()))?
            .collect::<Vec<_>>();
        if addresses.is_empty() {
            return Err(HttpGatewayError::new(
                HttpErrorCode::DnsFailed,
                "DNS returned no addresses",
            ));
        }
        Ok(addresses)
    }
}

type HttpBodyStream = Pin<Box<dyn Stream<Item = Result<Bytes, HttpGatewayError>> + Send>>;

struct VerifiedHttpRequest {
    method: HttpMethod,
    url: Url,
    address: SocketAddr,
    headers: BTreeMap<String, String>,
    body: Option<Vec<u8>>,
    connect_timeout: Duration,
}

struct HttpHopResponse {
    status: u16,
    headers: BTreeMap<String, Vec<String>>,
    location: Option<String>,
    content_length: Option<u64>,
    body: HttpBodyStream,
}

#[async_trait]
trait HttpHopGateway: Send + Sync {
    async fn send(&self, request: VerifiedHttpRequest)
    -> Result<HttpHopResponse, HttpGatewayError>;
}

struct ReqwestHopGateway;

#[async_trait]
impl HttpHopGateway for ReqwestHopGateway {
    async fn send(
        &self,
        request: VerifiedHttpRequest,
    ) -> Result<HttpHopResponse, HttpGatewayError> {
        let host = request.url.host_str().ok_or_else(|| {
            HttpGatewayError::new(HttpErrorCode::InvalidUrl, "URL host is missing")
        })?;
        let client = reqwest::Client::builder()
            .connect_timeout(request.connect_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .resolve(host, request.address)
            .build()
            .map_err(|error| {
                HttpGatewayError::new(HttpErrorCode::ConnectFailed, error.to_string())
            })?;
        let method =
            reqwest::Method::from_bytes(request.method.as_str().as_bytes()).map_err(|error| {
                HttpGatewayError::new(HttpErrorCode::InvalidRequest, error.to_string())
            })?;
        let headers = request_headers(&request.headers)?;
        let mut builder = client.request(method, request.url).headers(headers);
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
        let response = builder.send().await.map_err(|error| {
            let code = if error.is_connect() {
                HttpErrorCode::ConnectFailed
            } else {
                HttpErrorCode::ResponseFailed
            };
            HttpGatewayError::new(code, error.to_string())
        })?;
        let status = response.status().as_u16();
        let content_length = response.content_length();
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .map(|value| value.to_str().map(ToOwned::to_owned))
            .transpose()
            .map_err(|error| {
                HttpGatewayError::new(HttpErrorCode::RedirectDenied, error.to_string())
            })?;
        let headers = response_headers(response.headers());
        let body = response.bytes_stream().map(|chunk| {
            chunk.map_err(|error| {
                HttpGatewayError::new(HttpErrorCode::ResponseFailed, error.to_string())
            })
        });
        Ok(HttpHopResponse {
            status,
            headers,
            location,
            content_length,
            body: Box::pin(body),
        })
    }
}

pub struct SecureHttpGateway {
    config: HttpClientConfig,
    dns: Arc<dyn DnsGateway>,
    hop: Arc<dyn HttpHopGateway>,
}

impl SecureHttpGateway {
    /// Creates the production HTTPS gateway after validating all policy limits.
    ///
    /// # Errors
    ///
    /// Returns an error when the supplied HTTP policy is invalid.
    pub fn new(config: HttpClientConfig) -> Result<Self, String> {
        config.validate()?;
        Ok(Self {
            config,
            dns: Arc::new(TokioDnsGateway),
            hop: Arc::new(ReqwestHopGateway),
        })
    }

    #[cfg(test)]
    fn with_gateways(
        config: HttpClientConfig,
        dns: Arc<dyn DnsGateway>,
        hop: Arc<dyn HttpHopGateway>,
    ) -> Self {
        config.validate().unwrap();
        Self { config, dns, hop }
    }

    async fn execute_inner(
        &self,
        request: HttpRequest,
        request_body: Option<Vec<u8>>,
        limits: EffectiveLimits,
    ) -> Result<FetchedHttpResponse, HttpGatewayError> {
        let request_allowlist = request.limits.domain_allowlist;
        let mut url = Url::parse(&request.url)
            .map_err(|error| HttpGatewayError::new(HttpErrorCode::InvalidUrl, error.to_string()))?;
        let mut method = request.method;
        let mut body = request_body;
        let mut headers = request.headers;
        let mut redirects_followed = 0_u8;
        loop {
            self.validate_url(&url, redirects_followed > 0, request_allowlist.as_deref())?;
            let host = url.host_str().ok_or_else(|| {
                HttpGatewayError::new(HttpErrorCode::InvalidUrl, "URL host is missing")
            })?;
            let port = url.port_or_known_default().ok_or_else(|| {
                HttpGatewayError::new(HttpErrorCode::InvalidUrl, "URL port is missing")
            })?;
            let addresses = self.dns.resolve(host, port).await?;
            let address = validated_address(host, &addresses)?;
            let hop_request = VerifiedHttpRequest {
                method,
                url: url.clone(),
                address,
                headers: headers.clone(),
                body: body.clone(),
                connect_timeout: limits.connect_timeout,
            };
            let mut response =
                tokio::time::timeout(limits.header_timeout, self.hop.send(hop_request))
                    .await
                    .map_err(|_| {
                        HttpGatewayError::new(
                            HttpErrorCode::HeaderTimeout,
                            "response headers exceeded the configured timeout",
                        )
                    })??;
            if is_redirect(response.status) {
                let location = response.location.ok_or_else(|| {
                    HttpGatewayError::new(
                        HttpErrorCode::RedirectDenied,
                        "redirect response omitted Location",
                    )
                })?;
                if redirects_followed >= limits.max_redirects {
                    return Err(HttpGatewayError::new(
                        HttpErrorCode::TooManyRedirects,
                        "redirect limit exceeded",
                    ));
                }
                let next = url.join(&location).map_err(|error| {
                    HttpGatewayError::new(HttpErrorCode::RedirectDenied, error.to_string())
                })?;
                self.validate_url(&next, true, request_allowlist.as_deref())?;
                if next.host_str() != url.host_str() {
                    remove_sensitive_headers(&mut headers);
                }
                if response.status == 303
                    || ((response.status == 301 || response.status == 302)
                        && method == HttpMethod::Post)
                {
                    method = HttpMethod::Get;
                    body = None;
                    remove_body_headers(&mut headers);
                }
                redirects_followed += 1;
                url = next;
                continue;
            }
            if response
                .content_length
                .is_some_and(|length| length > limits.max_response_bytes)
            {
                return Err(HttpGatewayError::new(
                    HttpErrorCode::BodyTooLarge,
                    "Content-Length exceeds the configured response limit",
                )
                .evidence("max_response_bytes", limits.max_response_bytes.to_string()));
            }
            let (bytes, peak_buffered_bytes) =
                collect_response_body(&mut response, &limits).await?;
            return Ok(FetchedHttpResponse {
                metadata: HttpResponseMetadata {
                    status: response.status,
                    final_url: url.to_string(),
                    headers: response.headers,
                    body_bytes: bytes.len() as u64,
                    redirects_followed,
                },
                body: bytes,
                peak_buffered_bytes,
            });
        }
    }

    fn validate_url(
        &self,
        url: &Url,
        redirect: bool,
        request_allowlist: Option<&[String]>,
    ) -> Result<(), HttpGatewayError> {
        let denied_code = if redirect {
            HttpErrorCode::RedirectDenied
        } else {
            HttpErrorCode::DomainDenied
        };
        if url.scheme() != "https" {
            return Err(HttpGatewayError::new(
                if redirect {
                    HttpErrorCode::RedirectDenied
                } else {
                    HttpErrorCode::HttpsRequired
                },
                "only HTTPS requests are allowed",
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(HttpGatewayError::new(
                if redirect {
                    HttpErrorCode::RedirectDenied
                } else {
                    HttpErrorCode::InvalidUrl
                },
                "URL userinfo is not allowed",
            ));
        }
        let host = url.host_str().ok_or_else(|| {
            HttpGatewayError::new(HttpErrorCode::InvalidUrl, "URL host is missing")
        })?;
        if !self.domain_allowed(host, request_allowlist) {
            return Err(HttpGatewayError::new(
                denied_code,
                "URL host is not in the configured allowlist",
            )
            .evidence("host", host));
        }
        Ok(())
    }

    fn domain_allowed(&self, host: &str, request_allowlist: Option<&[String]>) -> bool {
        let host = normalize_domain(host);
        let matches = |allowed: &str| {
            let allowed = normalize_domain(allowed);
            host == allowed
                || host
                    .strip_suffix(&allowed)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        };
        self.config
            .domain_allowlist
            .iter()
            .any(|allowed| matches(allowed))
            && request_allowlist.map_or(true, |list| list.iter().any(|allowed| matches(allowed)))
    }
}

async fn collect_response_body(
    response: &mut HttpHopResponse,
    limits: &EffectiveLimits,
) -> Result<(Vec<u8>, u64), HttpGatewayError> {
    const INITIAL_CAPACITY_LIMIT: u64 = 64 * 1024;
    let planned_capacity = response
        .content_length
        .unwrap_or(0)
        .min(limits.max_response_bytes)
        .min(INITIAL_CAPACITY_LIMIT);
    let capacity = usize::try_from(planned_capacity).unwrap_or(usize::MAX);
    let mut bytes = Vec::with_capacity(capacity);
    let mut peak_buffered_bytes = 0_u64;
    while let Some(chunk) = tokio::time::timeout(limits.idle_timeout, response.body.next())
        .await
        .map_err(|_| {
            HttpGatewayError::new(
                HttpErrorCode::IdleTimeout,
                "response body exceeded the configured idle timeout",
            )
        })?
    {
        let chunk = chunk?;
        let next_len = u64::try_from(bytes.len())
            .unwrap_or(u64::MAX)
            .saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        peak_buffered_bytes = peak_buffered_bytes.max(next_len);
        if next_len > limits.max_response_bytes {
            return Err(HttpGatewayError::new(
                HttpErrorCode::BodyTooLarge,
                "streamed response exceeded the configured response limit",
            )
            .evidence("max_response_bytes", limits.max_response_bytes.to_string())
            .evidence("observed_bytes", next_len.to_string()));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((bytes, peak_buffered_bytes))
}

#[async_trait]
impl HttpGateway for SecureHttpGateway {
    async fn execute(
        &self,
        request: HttpRequest,
        request_body: Option<Vec<u8>>,
    ) -> Result<FetchedHttpResponse, HttpGatewayError> {
        let limits = self.config.effective_limits(&request);
        if limits.max_response_bytes == 0
            || limits.connect_timeout.is_zero()
            || limits.header_timeout.is_zero()
            || limits.idle_timeout.is_zero()
            || limits.total_timeout.is_zero()
        {
            return Err(HttpGatewayError::new(
                HttpErrorCode::InvalidRequest,
                "request limits must be greater than zero",
            ));
        }
        tokio::time::timeout(
            limits.total_timeout,
            self.execute_inner(request, request_body, limits),
        )
        .await
        .map_err(|_| {
            HttpGatewayError::new(
                HttpErrorCode::TotalTimeout,
                "request exceeded the configured total timeout",
            )
        })?
    }
}

#[derive(Clone, Copy)]
struct EffectiveLimits {
    max_response_bytes: u64,
    connect_timeout: Duration,
    header_timeout: Duration,
    idle_timeout: Duration,
    total_timeout: Duration,
    max_redirects: u8,
}

pub struct HttpEffectHandler {
    descriptor: RunnerDescriptor,
    gateway: Arc<dyn HttpGateway>,
    resources: Arc<dyn ResourceRegistryGateway>,
    response_provider_id: String,
}

impl HttpEffectHandler {
    pub fn new(
        gateway: Arc<dyn HttpGateway>,
        resources: Arc<dyn ResourceRegistryGateway>,
        response_provider_id: impl Into<String>,
    ) -> Self {
        Self {
            descriptor: effect_runner_descriptor(),
            gateway,
            resources,
            response_provider_id: response_provider_id.into(),
        }
    }
}

impl AsyncBatchHandler for HttpEffectHandler {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(&self, _ctx: RunnerContext, batch: WorkBatch) -> AsyncCompletionFuture {
        let tasks = match batch.row_payload_tasks() {
            Ok(tasks) => tasks,
            Err(error) => {
                return Box::pin(async move { Ok(CompletionBatch::from_error(&batch, error)) });
            }
        };
        let gateway = self.gateway.clone();
        let resources = self.resources.clone();
        let provider_id = self.response_provider_id.clone();
        Box::pin(async move {
            let mut results = Vec::with_capacity(batch.entries.len());
            for entry in &batch.entries {
                let task = tasks
                    .iter()
                    .find(|task| task.task_id == entry.task_id)
                    .expect("validated batch contains every task")
                    .clone();
                let completion = execute_effect_task(
                    task,
                    gateway.clone(),
                    resources.clone(),
                    provider_id.clone(),
                )
                .await;
                let (result, error) = match completion {
                    Ok(result) => (Some(result), None),
                    Err(error) => (None, Some(error.error().clone())),
                };
                results.push(EntryCompletion {
                    entry_id: entry.entry_id.clone(),
                    task_id: entry.task_id.clone(),
                    result,
                    error,
                });
            }
            Ok(CompletionBatch::from_results(&batch, results))
        })
    }
}

async fn execute_effect_task(
    task: Task,
    gateway: Arc<dyn HttpGateway>,
    resources: Arc<dyn ResourceRegistryGateway>,
    response_provider_id: String,
) -> RuntimeResult<RunnerResult> {
    if task.protocol_id != EFFECT_HTTP_REQUEST_PROTOCOL {
        return Err(task_failure(
            &task,
            HttpErrorCode::InvalidRequest,
            "unsupported HTTP effect protocol",
        ));
    }
    let request: HttpRequest = serde_json::from_value(task.payload.clone().into())
        .map_err(|error| task_failure(&task, HttpErrorCode::InvalidRequest, error.to_string()))?;
    let request_body = request
        .body
        .as_ref()
        .map(|resource| {
            resources.collect_read_plan(&ReadPlan {
                plan_id: format!("http.request.body.{}", task.task_id),
                resource: resource.clone(),
                operation: "collect".into(),
                args: serde_json::Value::Null,
            })
        })
        .transpose()
        .map_err(|error| {
            task_failure(&task, HttpErrorCode::RequestBodyFailed, error.to_string())
        })?;
    let fetched = gateway
        .execute(request, request_body)
        .await
        .map_err(|error| gateway_failure(&task, error))?;
    let body = if fetched.body.is_empty() {
        None
    } else {
        Some(resources.create_blob_resource(
            &response_provider_id,
            RESPONSE_BODY_SCHEMA,
            fetched.body,
        )?)
    };
    let response = HttpResponse {
        metadata: fetched.metadata,
        body,
    };
    let payload = serde_json::to_value(&response)
        .map_err(|error| task_failure(&task, HttpErrorCode::ResponseFailed, error.to_string()))?;
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.output = Some(payload.clone());
    result.events.push(DomainEvent {
        event_id: format!("event:{}.http_response", task.task_id),
        kind: HTTP_REQUEST_PROTOCOL.into(),
        payload,
    });
    Ok(result)
}

pub fn facade_runner(client: RuntimeClientRef) -> Box<dyn mutsuki_runtime_core::Runner> {
    let factory = Box::new(
        move |ctx: mutsuki_runtime_sdk::AsyncRunnerContext, task: Task| {
            Box::pin(run_facade_task(ctx, task))
                as Pin<Box<dyn Future<Output = RuntimeResult<RunnerResult>> + Send>>
        },
    );
    Box::new(
        TaskAwaitRunnerAdapter::new(facade_runner_descriptor(), client, factory)
            .with_self_call_policy(false),
    )
}

async fn run_facade_task(
    ctx: mutsuki_runtime_sdk::AsyncRunnerContext,
    task: Task,
) -> RuntimeResult<RunnerResult> {
    let request: HttpRequest = serde_json::from_value(task.payload.clone().into())
        .map_err(|error| task_failure(&task, HttpErrorCode::InvalidRequest, error.to_string()))?;
    let outcome = ctx
        .call_raw(
            EFFECT_HTTP_REQUEST_PROTOCOL,
            serde_json::to_value(request).map_err(|error| {
                task_failure(&task, HttpErrorCode::InvalidRequest, error.to_string())
            })?,
        )
        .await?
        .into_outcome();
    let output = match outcome {
        TaskOutcome::Completed {
            output: Some(output),
            ..
        } => output,
        TaskOutcome::Completed { output: None, .. } => {
            return Err(task_failure(
                &task,
                HttpErrorCode::ResponseFailed,
                "HTTP effect completed without a response",
            ));
        }
        TaskOutcome::Failed { error, .. } => return Err(RuntimeFailure::new(error)),
        TaskOutcome::Cancelled { .. } => {
            return Err(task_failure(
                &task,
                HttpErrorCode::ResponseFailed,
                "HTTP effect was cancelled",
            ));
        }
        TaskOutcome::Expired { .. } => {
            return Err(task_failure(
                &task,
                HttpErrorCode::TotalTimeout,
                "HTTP effect expired",
            ));
        }
        TaskOutcome::DeadLetter { .. } => {
            return Err(task_failure(
                &task,
                HttpErrorCode::ResponseFailed,
                "HTTP effect was dead-lettered",
            ));
        }
    };
    let mut result = RunnerResult::completed(task.task_id);
    result.output = Some(output);
    Ok(result)
}

#[must_use]
pub fn manifest() -> mutsuki_runtime_contracts::PluginManifest {
    PluginBuilder::new(PLUGIN_ID)
        .runner_descriptor(facade_runner_descriptor())
        .runner_descriptor(effect_runner_descriptor())
        .protocol_handler(protocol_descriptor(HTTP_REQUEST_PROTOCOL), RUNNER_ID, "io")
        .protocol_descriptor(protocol_descriptor(EFFECT_HTTP_REQUEST_PROTOCOL))
        .build()
        .manifest
}

fn facade_runner_descriptor() -> RunnerDescriptor {
    RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID)
        .accepted_protocol(HTTP_REQUEST_PROTOCOL)
        .requires_protocol(EFFECT_HTTP_REQUEST_PROTOCOL)
        .purity(RunnerPurity::Pure)
        .execution_class(ExecutionClass::Orchestration)
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::ScalarAdapter,
            side_effect: RunnerSideEffect::None,
            ..Default::default()
        })
        .metadata(
            "standard_plugin",
            ScalarValue::String("io_http_client".into()),
        )
        .build()
}

fn effect_runner_descriptor() -> RunnerDescriptor {
    RunnerDescriptorBuilder::new(EFFECT_RUNNER_ID, PLUGIN_ID)
        .accepted_protocol(EFFECT_HTTP_REQUEST_PROTOCOL)
        .purity(RunnerPurity::Effectful)
        .execution_class(ExecutionClass::Io)
        .invocation_mode(InvocationMode::AsyncReentrant)
        .concurrency(RunnerConcurrency::Reentrant {
            max_inflight_batches: 16,
            max_inflight_entries: 64,
        })
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::NativeBatch,
            preferred_batch_size: 8,
            max_batch_entries: 64,
            max_inflight_batches: 16,
            max_entry_concurrency: 64,
            side_effect: RunnerSideEffect::External,
            ..Default::default()
        })
        .metadata(
            "standard_plugin",
            ScalarValue::String("io_http_client".into()),
        )
        .metadata(
            "effect_execution",
            ScalarValue::String("host_async_executor".into()),
        )
        .build()
}

fn protocol_descriptor(protocol_id: &str) -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(protocol_id)
        .input_schema(mutsuki_protocol_http::input_schema(protocol_id).unwrap_or_default())
        .output_schema(mutsuki_protocol_http::output_schema(protocol_id).unwrap_or_default())
        .error_schema(mutsuki_protocol_http::error_schema(protocol_id).unwrap_or_default())
        .build()
}

fn gateway_failure(task: &Task, error: HttpGatewayError) -> RuntimeFailure {
    let mut failure =
        RuntimeError::new(error.code.as_str(), "runtime.io_http_client", error.message);
    failure.evidence.insert(
        "task_id".into(),
        ScalarValue::String(task.task_id.to_string()),
    );
    for (key, value) in error.evidence {
        failure.evidence.insert(key, ScalarValue::String(value));
    }
    RuntimeFailure::new(failure)
}

fn task_failure(task: &Task, code: HttpErrorCode, message: impl Into<String>) -> RuntimeFailure {
    gateway_failure(task, HttpGatewayError::new(code, message))
}

fn request_headers(headers: &BTreeMap<String, String>) -> Result<HeaderMap, HttpGatewayError> {
    let mut result = HeaderMap::new();
    for (name, value) in headers {
        let normalized = name.to_ascii_lowercase();
        if matches!(
            normalized.as_str(),
            "host" | "content-length" | "transfer-encoding" | "connection"
        ) {
            return Err(HttpGatewayError::new(
                HttpErrorCode::InvalidHeader,
                format!("header `{name}` is managed by the HTTP gateway"),
            ));
        }
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
            HttpGatewayError::new(HttpErrorCode::InvalidHeader, error.to_string())
        })?;
        let value = HeaderValue::from_str(value).map_err(|error| {
            HttpGatewayError::new(HttpErrorCode::InvalidHeader, error.to_string())
        })?;
        result.insert(name, value);
    }
    Ok(result)
}

fn response_headers(headers: &HeaderMap) -> BTreeMap<String, Vec<String>> {
    let mut result = BTreeMap::<String, Vec<String>>::new();
    for (name, value) in headers {
        if let Ok(value) = value.to_str() {
            result
                .entry(name.as_str().to_ascii_lowercase())
                .or_default()
                .push(value.to_owned());
        }
    }
    result
}

fn normalize_domain(domain: &str) -> String {
    domain.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn validated_address(host: &str, addresses: &[SocketAddr]) -> Result<SocketAddr, HttpGatewayError> {
    if let Some(address) = addresses.iter().find(|address| forbidden_ip(address.ip())) {
        return Err(HttpGatewayError::new(
            HttpErrorCode::PrivateAddress,
            "DNS resolved to a private or reserved address",
        )
        .evidence("host", host)
        .evidence("address", address.ip().to_string()));
    }
    addresses
        .first()
        .copied()
        .ok_or_else(|| HttpGatewayError::new(HttpErrorCode::DnsFailed, "DNS returned no addresses"))
}

fn forbidden_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => forbidden_ipv4(address),
        IpAddr::V6(address) => forbidden_ipv6(address),
    }
}

fn forbidden_ipv4(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    address.is_unspecified()
        || address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_multicast()
        || address.is_broadcast()
        || address.is_documentation()
        || octets[0] == 0
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        || (octets[0] == 198 && (octets[1] == 18 || octets[1] == 19))
        || octets[0] >= 240
}

fn forbidden_ipv6(address: Ipv6Addr) -> bool {
    let segments = address.segments();
    address.is_unspecified()
        || address.is_loopback()
        || address.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
        || address.to_ipv4_mapped().is_some_and(forbidden_ipv4)
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn remove_sensitive_headers(headers: &mut BTreeMap<String, String>) {
    headers.retain(|name, _| {
        !matches!(
            name.to_ascii_lowercase().as_str(),
            "authorization" | "cookie" | "proxy-authorization"
        )
    });
}

fn remove_body_headers(headers: &mut BTreeMap<String, String>) {
    headers.retain(|name, _| {
        !matches!(
            name.to_ascii_lowercase().as_str(),
            "content-type" | "content-encoding"
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn config() -> HttpClientConfig {
        HttpClientConfig {
            response_provider_id: "memory".into(),
            domain_allowlist: vec!["example.com".into()],
            max_response_bytes: 10,
            connect_timeout_ms: 50,
            header_timeout_ms: 50,
            idle_timeout_ms: 50,
            total_timeout_ms: 100,
            max_redirects: 2,
        }
    }

    struct FakeDns {
        answers: Mutex<VecDeque<Vec<SocketAddr>>>,
        calls: AtomicUsize,
    }

    impl FakeDns {
        fn new(answers: impl IntoIterator<Item = Vec<SocketAddr>>) -> Self {
            Self {
                answers: Mutex::new(answers.into_iter().collect()),
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl DnsGateway for FakeDns {
        async fn resolve(
            &self,
            _host: &str,
            _port: u16,
        ) -> Result<Vec<SocketAddr>, HttpGatewayError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.answers.lock().unwrap().pop_front().ok_or_else(|| {
                HttpGatewayError::new(HttpErrorCode::DnsFailed, "no fake DNS answer")
            })
        }
    }

    struct FakeHop {
        replies: Mutex<VecDeque<FakeReply>>,
        addresses: Mutex<Vec<SocketAddr>>,
    }

    struct FakeReply {
        status: u16,
        location: Option<String>,
        content_length: Option<u64>,
        chunks: Vec<Vec<u8>>,
        header_delay: Duration,
    }

    impl FakeHop {
        fn new(replies: impl IntoIterator<Item = FakeReply>) -> Self {
            Self {
                replies: Mutex::new(replies.into_iter().collect()),
                addresses: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl HttpHopGateway for FakeHop {
        async fn send(
            &self,
            request: VerifiedHttpRequest,
        ) -> Result<HttpHopResponse, HttpGatewayError> {
            self.addresses.lock().unwrap().push(request.address);
            let reply = self.replies.lock().unwrap().pop_front().unwrap();
            tokio::time::sleep(reply.header_delay).await;
            Ok(HttpHopResponse {
                status: reply.status,
                headers: BTreeMap::new(),
                location: reply.location,
                content_length: reply.content_length,
                body: Box::pin(stream::iter(
                    reply.chunks.into_iter().map(|chunk| Ok(Bytes::from(chunk))),
                )),
            })
        }
    }

    fn reply(status: u16, chunks: &[&[u8]]) -> FakeReply {
        FakeReply {
            status,
            location: None,
            content_length: None,
            chunks: chunks.iter().map(|chunk| chunk.to_vec()).collect(),
            header_delay: Duration::ZERO,
        }
    }

    fn redirect(location: &str) -> FakeReply {
        FakeReply {
            status: 302,
            location: Some(location.into()),
            content_length: None,
            chunks: Vec::new(),
            header_delay: Duration::ZERO,
        }
    }

    fn public_address() -> SocketAddr {
        "93.184.216.34:443".parse().unwrap()
    }

    #[test]
    fn manifest_declares_typed_public_and_effect_protocols() {
        let manifest = manifest();
        assert_eq!(manifest.plugin_id, PLUGIN_ID);
        assert_eq!(manifest.provides.runners.len(), 2);
        assert_eq!(manifest.provides.protocols.len(), 2);
        assert_eq!(manifest.provides.handler_bindings.len(), 1);
        assert_eq!(
            manifest.provides.runners[1].invocation_mode,
            InvocationMode::AsyncReentrant
        );
    }

    #[tokio::test]
    async fn private_or_reserved_dns_answer_is_rejected_before_connect() {
        let dns = Arc::new(FakeDns::new([vec!["127.0.0.1:443".parse().unwrap()]]));
        let hop = Arc::new(FakeHop::new([]));
        let gateway = SecureHttpGateway::with_gateways(config(), dns, hop.clone());
        let error = gateway
            .execute(HttpRequest::get("https://example.com/image"), None)
            .await
            .unwrap_err();
        assert_eq!(error.code, HttpErrorCode::PrivateAddress);
        assert!(hop.addresses.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn verified_address_is_bound_without_a_second_dns_resolution() {
        let dns = Arc::new(FakeDns::new([
            vec![public_address()],
            vec!["127.0.0.1:443".parse().unwrap()],
        ]));
        let hop = Arc::new(FakeHop::new([reply(200, &[b"ok"])]));
        let gateway = SecureHttpGateway::with_gateways(config(), dns.clone(), hop.clone());
        let response = gateway
            .execute(HttpRequest::get("https://example.com/image"), None)
            .await
            .unwrap();
        assert_eq!(response.body, b"ok");
        assert!(response.peak_buffered_bytes <= config().max_response_bytes + 2);
        assert_eq!(dns.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            hop.addresses.lock().unwrap().as_slice(),
            &[public_address()]
        );
    }

    #[tokio::test]
    async fn cross_domain_redirect_is_revalidated_and_denied() {
        let dns = Arc::new(FakeDns::new([vec![public_address()]]));
        let hop = Arc::new(FakeHop::new([redirect("https://blocked.invalid/next")]));
        let gateway = SecureHttpGateway::with_gateways(config(), dns, hop);
        let error = gateway
            .execute(HttpRequest::get("https://example.com/start"), None)
            .await
            .unwrap_err();
        assert_eq!(error.code, HttpErrorCode::RedirectDenied);
    }

    #[tokio::test]
    async fn streamed_body_is_rejected_before_crossing_the_hard_limit() {
        let dns = Arc::new(FakeDns::new([vec![public_address()]]));
        let hop = Arc::new(FakeHop::new([reply(200, &[b"123456", b"789012"])]));
        let gateway = SecureHttpGateway::with_gateways(config(), dns, hop);
        let error = gateway
            .execute(HttpRequest::get("https://example.com/large"), None)
            .await
            .unwrap_err();
        assert_eq!(error.code, HttpErrorCode::BodyTooLarge);
        assert_eq!(
            error.evidence.get("observed_bytes").map(String::as_str),
            Some("12")
        );
    }

    #[tokio::test]
    async fn header_timeout_is_distinct_from_total_timeout() {
        let dns = Arc::new(FakeDns::new([vec![public_address()]]));
        let hop = Arc::new(FakeHop::new([FakeReply {
            status: 200,
            location: None,
            content_length: None,
            chunks: Vec::new(),
            header_delay: Duration::from_millis(75),
        }]));
        let gateway = SecureHttpGateway::with_gateways(config(), dns, hop);
        let error = gateway
            .execute(HttpRequest::get("https://example.com/slow"), None)
            .await
            .unwrap_err();
        assert_eq!(error.code, HttpErrorCode::HeaderTimeout);
    }

    struct SlowBodyHop;

    #[async_trait]
    impl HttpHopGateway for SlowBodyHop {
        async fn send(
            &self,
            _request: VerifiedHttpRequest,
        ) -> Result<HttpHopResponse, HttpGatewayError> {
            let body = stream::once(async {
                tokio::time::sleep(Duration::from_millis(75)).await;
                Ok(Bytes::from_static(b"late"))
            });
            Ok(HttpHopResponse {
                status: 200,
                headers: BTreeMap::new(),
                location: None,
                content_length: None,
                body: Box::pin(body),
            })
        }
    }

    #[tokio::test]
    async fn idle_timeout_applies_to_each_stream_chunk() {
        let dns = Arc::new(FakeDns::new([vec![public_address()]]));
        let gateway = SecureHttpGateway::with_gateways(config(), dns, Arc::new(SlowBodyHop));
        let error = gateway
            .execute(HttpRequest::get("https://example.com/slow-body"), None)
            .await
            .unwrap_err();
        assert_eq!(error.code, HttpErrorCode::IdleTimeout);
    }

    struct SlowDns;

    #[async_trait]
    impl DnsGateway for SlowDns {
        async fn resolve(
            &self,
            _host: &str,
            _port: u16,
        ) -> Result<Vec<SocketAddr>, HttpGatewayError> {
            tokio::time::sleep(Duration::from_millis(125)).await;
            Ok(vec![public_address()])
        }
    }

    #[tokio::test]
    async fn total_timeout_covers_dns_and_all_redirect_hops() {
        let gateway = SecureHttpGateway::with_gateways(
            config(),
            Arc::new(SlowDns),
            Arc::new(FakeHop::new([])),
        );
        let error = gateway
            .execute(HttpRequest::get("https://example.com/slow-dns"), None)
            .await
            .unwrap_err();
        assert_eq!(error.code, HttpErrorCode::TotalTimeout);
    }

    struct PendingDns {
        dropped: Arc<AtomicBool>,
    }

    struct DropMarker(Arc<AtomicBool>);

    impl Drop for DropMarker {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl DnsGateway for PendingDns {
        async fn resolve(
            &self,
            _host: &str,
            _port: u16,
        ) -> Result<Vec<SocketAddr>, HttpGatewayError> {
            let _marker = DropMarker(self.dropped.clone());
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn dropping_request_future_propagates_cancellation_to_gateway_work() {
        let dropped = Arc::new(AtomicBool::new(false));
        let gateway = Arc::new(SecureHttpGateway::with_gateways(
            config(),
            Arc::new(PendingDns {
                dropped: dropped.clone(),
            }),
            Arc::new(FakeHop::new([])),
        ));
        let task = tokio::spawn({
            let gateway = gateway.clone();
            async move {
                gateway
                    .execute(HttpRequest::get("https://example.com/pending"), None)
                    .await
            }
        });
        tokio::task::yield_now().await;
        task.abort();
        let _ = task.await;
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn request_domain_allowlist_rejects_hosts_outside_the_request_scope() {
        let mut cfg = config();
        cfg.domain_allowlist = vec!["example.com".into(), "cdn.example.net".into()];
        let dns = Arc::new(FakeDns::new([vec![public_address()]]));
        let hop = Arc::new(FakeHop::new([redirect("https://cdn.example.net/image")]));
        let gateway = SecureHttpGateway::with_gateways(cfg, dns, hop);
        let mut request = HttpRequest::get("https://example.com/start");
        request.limits.domain_allowlist = Some(vec!["example.com".into()]);
        let error = gateway.execute(request, None).await.unwrap_err();
        assert_eq!(error.code, HttpErrorCode::RedirectDenied);
        assert_eq!(
            error.evidence.get("host").map(String::as_str),
            Some("cdn.example.net")
        );
    }

    #[tokio::test]
    async fn http_redirect_target_is_denied() {
        let dns = Arc::new(FakeDns::new([vec![public_address()]]));
        let hop = Arc::new(FakeHop::new([redirect("http://example.com/insecure")]));
        let gateway = SecureHttpGateway::with_gateways(config(), dns, hop);
        let error = gateway
            .execute(HttpRequest::get("https://example.com/start"), None)
            .await
            .unwrap_err();
        assert_eq!(error.code, HttpErrorCode::RedirectDenied);
    }

    #[tokio::test]
    async fn redirect_loop_exceeding_max_redirects_is_rejected() {
        let dns = Arc::new(FakeDns::new([
            vec![public_address()],
            vec![public_address()],
            vec![public_address()],
        ]));
        let hop = Arc::new(FakeHop::new([
            redirect("https://example.com/a"),
            redirect("https://example.com/b"),
            redirect("https://example.com/c"),
        ]));
        let gateway = SecureHttpGateway::with_gateways(config(), dns, hop);
        let error = gateway
            .execute(HttpRequest::get("https://example.com/start"), None)
            .await
            .unwrap_err();
        assert_eq!(error.code, HttpErrorCode::TooManyRedirects);
    }

    #[tokio::test]
    async fn private_dns_on_redirect_target_is_rejected() {
        let dns = Arc::new(FakeDns::new([
            vec![public_address()],
            vec!["127.0.0.1:443".parse().unwrap()],
        ]));
        let hop = Arc::new(FakeHop::new([redirect("https://example.com/private")]));
        let gateway = SecureHttpGateway::with_gateways(config(), dns, hop);
        let error = gateway
            .execute(HttpRequest::get("https://example.com/start"), None)
            .await
            .unwrap_err();
        assert_eq!(error.code, HttpErrorCode::PrivateAddress);
    }

    #[tokio::test]
    async fn forged_content_length_still_stops_when_stream_exceeds_limit() {
        let dns = Arc::new(FakeDns::new([vec![public_address()]]));
        let hop = Arc::new(FakeHop::new([FakeReply {
            status: 200,
            location: None,
            content_length: Some(4),
            chunks: vec![b"123456".to_vec(), b"789012".to_vec()],
            header_delay: Duration::ZERO,
        }]));
        let gateway = SecureHttpGateway::with_gateways(config(), dns, hop);
        let error = gateway
            .execute(HttpRequest::get("https://example.com/forged-length"), None)
            .await
            .unwrap_err();
        assert_eq!(error.code, HttpErrorCode::BodyTooLarge);
        assert_eq!(
            error.evidence.get("observed_bytes").map(String::as_str),
            Some("12")
        );
    }

    #[tokio::test]
    async fn oversized_chunked_body_does_not_buffer_the_full_response() {
        let dns = Arc::new(FakeDns::new([vec![public_address()]]));
        let large = vec![b'x'; 64];
        let hop = Arc::new(FakeHop::new([FakeReply {
            status: 200,
            location: None,
            content_length: None,
            chunks: vec![large.clone(), large.clone(), large],
            header_delay: Duration::ZERO,
        }]));
        let mut cfg = config();
        cfg.max_response_bytes = 80;
        let gateway = SecureHttpGateway::with_gateways(cfg, dns, hop);
        let error = gateway
            .execute(HttpRequest::get("https://example.com/chunked"), None)
            .await
            .unwrap_err();
        assert_eq!(error.code, HttpErrorCode::BodyTooLarge);
        let observed = error
            .evidence
            .get("observed_bytes")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap();
        // Fail on the second 64-byte chunk (80 limit); never buffer all three chunks (192).
        assert!(observed <= 128, "observed_bytes={observed}");
    }
}
