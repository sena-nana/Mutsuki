use thiserror::Error;
use url::Url;

pub const DEFAULT_QQBOT_INTENTS: u64 = 1_325_405_185;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QqBotConfig {
    pub account_id: String,
    pub app_id: String,
    /// Key resolved through `HostEventSourceConfig::secret`; never the secret value.
    pub client_secret_key: String,
    pub token_url: String,
    pub openapi_base_url: String,
    pub gateway_intents: u64,
    pub shard: [u64; 2],
    pub request_timeout_ms: u64,
    pub connect_timeout_ms: u64,
    pub response_body_limit_bytes: usize,
    pub token_refresh_margin_secs: u64,
    pub max_retry_attempts: u8,
    pub retry_base_delay_ms: u64,
    pub retry_max_delay_ms: u64,
    pub gateway_hello_timeout_ms: u64,
    pub gateway_ack_timeout_ms: u64,
    pub gateway_queue_capacity: usize,
    pub gateway_dedup_window: usize,
    pub reconnect_initial_delay_ms: u64,
    pub reconnect_max_delay_ms: u64,
    pub reconnect_jitter_ms: u64,
    pub gateway_rate_limit_delay_ms: u64,
    /// Test/local-only escape hatch. Production defaults require HTTPS and WSS.
    pub allow_insecure_transport: bool,
}

impl QqBotConfig {
    pub fn new(account_id: impl Into<String>, app_id: impl Into<String>) -> Self {
        Self {
            account_id: account_id.into(),
            app_id: app_id.into(),
            client_secret_key: "QQBOT_CLIENT_SECRET".into(),
            token_url: "https://bots.qq.com/app/getAppAccessToken".into(),
            openapi_base_url: "https://api.sgroup.qq.com".into(),
            gateway_intents: DEFAULT_QQBOT_INTENTS,
            shard: [0, 1],
            request_timeout_ms: 15_000,
            connect_timeout_ms: 10_000,
            response_body_limit_bytes: 2 * 1024 * 1024,
            token_refresh_margin_secs: 120,
            max_retry_attempts: 3,
            retry_base_delay_ms: 250,
            retry_max_delay_ms: 5_000,
            gateway_hello_timeout_ms: 15_000,
            gateway_ack_timeout_ms: 10_000,
            gateway_queue_capacity: 128,
            gateway_dedup_window: 2_048,
            reconnect_initial_delay_ms: 500,
            reconnect_max_delay_ms: 30_000,
            reconnect_jitter_ms: 250,
            gateway_rate_limit_delay_ms: 60_000,
            allow_insecure_transport: false,
        }
    }

    pub fn validate(&self) -> Result<(), QqConfigError> {
        required("account_id", &self.account_id)?;
        required("app_id", &self.app_id)?;
        required("client_secret_key", &self.client_secret_key)?;
        validate_http_url("token_url", &self.token_url, self.allow_insecure_transport)?;
        validate_http_url(
            "openapi_base_url",
            &self.openapi_base_url,
            self.allow_insecure_transport,
        )?;
        if self.gateway_intents == 0 {
            return Err(QqConfigError::Invalid(
                "gateway_intents must not be zero".into(),
            ));
        }
        if self.shard[1] == 0 || self.shard[0] >= self.shard[1] {
            return Err(QqConfigError::Invalid(
                "shard must be [index, count] with index < count".into(),
            ));
        }
        for (name, value) in [
            ("request_timeout_ms", self.request_timeout_ms),
            ("connect_timeout_ms", self.connect_timeout_ms),
            ("gateway_hello_timeout_ms", self.gateway_hello_timeout_ms),
            ("gateway_ack_timeout_ms", self.gateway_ack_timeout_ms),
            ("reconnect_max_delay_ms", self.reconnect_max_delay_ms),
            (
                "gateway_rate_limit_delay_ms",
                self.gateway_rate_limit_delay_ms,
            ),
        ] {
            if value == 0 {
                return Err(QqConfigError::Invalid(format!("{name} must be positive")));
            }
        }
        if self.response_body_limit_bytes == 0
            || self.gateway_queue_capacity == 0
            || self.gateway_dedup_window == 0
        {
            return Err(QqConfigError::Invalid(
                "body limit, queue capacity and dedup window must be positive".into(),
            ));
        }
        if self.retry_base_delay_ms > self.retry_max_delay_ms
            || self.reconnect_initial_delay_ms > self.reconnect_max_delay_ms
        {
            return Err(QqConfigError::Invalid(
                "initial retry delays must not exceed maximum delays".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum QqConfigError {
    #[error("missing QQBot config field: {0}")]
    Missing(&'static str),
    #[error("invalid QQBot config: {0}")]
    Invalid(String),
}

pub(crate) fn validate_gateway_url(url: &str, allow_insecure: bool) -> Result<Url, QqConfigError> {
    let parsed = Url::parse(url).map_err(|error| QqConfigError::Invalid(error.to_string()))?;
    let allowed = parsed.scheme() == "wss" || (allow_insecure && parsed.scheme() == "ws");
    if !allowed
        || parsed.host_str().is_none()
        || parsed.username() != ""
        || parsed.password().is_some()
    {
        return Err(QqConfigError::Invalid(
            "Gateway URL must use wss:// with a host and without credentials".into(),
        ));
    }
    Ok(parsed)
}

fn required(name: &'static str, value: &str) -> Result<(), QqConfigError> {
    if value.trim().is_empty() {
        Err(QqConfigError::Missing(name))
    } else {
        Ok(())
    }
}

fn validate_http_url(name: &str, value: &str, allow_insecure: bool) -> Result<(), QqConfigError> {
    let parsed =
        Url::parse(value).map_err(|error| QqConfigError::Invalid(format!("{name}: {error}")))?;
    let allowed = parsed.scheme() == "https" || (allow_insecure && parsed.scheme() == "http");
    if !allowed
        || parsed.host_str().is_none()
        || parsed.username() != ""
        || parsed.password().is_some()
    {
        return Err(QqConfigError::Invalid(format!(
            "{name} must be an absolute HTTPS URL without credentials"
        )));
    }
    Ok(())
}
