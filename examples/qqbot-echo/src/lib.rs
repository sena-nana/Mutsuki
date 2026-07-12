use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

pub use bot_echo::{ECHO_PLUGIN_ID, ECHO_RUNNER_ID, echo_manifest, echo_runner};
use mutsuki_bot_protocol::{BOT_COMMAND_PARSE_PROTOCOL_ID, BotEventKind, BotEventSubscription};
use mutsuki_plugin_bot_adapter_qqbot::api::{HttpMethod, QqOpenApiError};
use mutsuki_plugin_bot_adapter_qqbot::tasks::{
    QQBOT_ADAPTER_PLUGIN_ID, QQBOT_GATEWAY_FRAME_PROTOCOL_ID, QqGatewayMapRunner, QqOpenApiRunner,
    qqbot_adapter_manifest,
};
use mutsuki_plugin_bot_adapter_qqbot::{
    QqBotClients, QqBotConfig, QqHttpClient, QqHttpRequest, QqHttpResponse, QqIdSource,
    StaticQqCredentials,
};
use mutsuki_plugin_bot_command::{BOT_COMMAND_PLUGIN_ID, BotCommandRunner, bot_command_manifest};
use mutsuki_plugin_bot_event_router::{
    BOT_EVENT_ROUTER_PLUGIN_ID, BotEventRouterRunner, bot_event_router_manifest,
};
use mutsuki_runtime_contracts::{RuntimeProfile, RuntimeProfileMode, Task};
use mutsuki_runtime_core::RuntimeResult;
use mutsuki_runtime_host::RuntimeBootstrapper;
use serde_json::{Value, json};

#[derive(Clone)]
pub struct EchoSmokeConfig {
    pub account_id: String,
    pub app_id: String,
    pub client_secret: String,
    pub group_openid: String,
    pub user_openid: String,
    pub command_text: String,
}

impl Default for EchoSmokeConfig {
    fn default() -> Self {
        Self {
            account_id: "example-bot".into(),
            app_id: "APP_ID".into(),
            client_secret: "CLIENT_SECRET".into(),
            group_openid: "GROUP_OPENID".into(),
            user_openid: "USER_OPENID".into(),
            command_text: "/echo hello from qqbot".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct EchoSmokeReport {
    pub runtime_report: mutsuki_runtime_core::RunnerLoopReport,
    pub requests: Vec<QqHttpRequest>,
}

impl EchoSmokeReport {
    pub fn to_json(&self) -> Value {
        json!({
            "runtime_report": {
                "claimed_tasks": self.runtime_report.claimed_tasks,
                "completed_tasks": self.runtime_report.completed_tasks,
            },
            "requests": request_log_json(&self.requests),
        })
    }
}

pub fn run_default_smoke() -> RuntimeResult<EchoSmokeReport> {
    run_smoke(EchoSmokeConfig::default())
}

pub fn run_smoke(config: EchoSmokeConfig) -> RuntimeResult<EchoSmokeReport> {
    let recording = Arc::new(Mutex::new(Vec::new()));
    let gateway_task = qqbot_group_message_task(&config);
    let mut runtime = build_echo_runtime(config, recording.clone())?;
    runtime.submit_task(gateway_task)?;
    let runtime_report = runtime.run_until_idle(16)?;
    let requests = recording.lock().unwrap().clone();

    Ok(EchoSmokeReport {
        runtime_report,
        requests,
    })
}

pub fn build_echo_runtime(
    config: EchoSmokeConfig,
    recording: Arc<Mutex<Vec<QqHttpRequest>>>,
) -> RuntimeResult<mutsuki_runtime_core::CoreRuntime> {
    let (bootstrapper, profile) = build_bootstrapper(config, recording);
    bootstrapper.into_runtime(profile)
}

pub fn build_bootstrapper(
    config: EchoSmokeConfig,
    recording: Arc<Mutex<Vec<QqHttpRequest>>>,
) -> (RuntimeBootstrapper, RuntimeProfile) {
    let mut bootstrapper = RuntimeBootstrapper::new();

    let gateway_runner = QqGatewayMapRunner::new(1, config.account_id.clone());
    let openapi_runner = QqOpenApiRunner::new(
        1,
        qqbot_config(&config),
        QqBotClients::new(
            Box::new(RecordingQqHttpClient::new(recording)),
            Arc::new(StaticQqCredentials::new(&config.client_secret)),
        ),
        Box::new(SequentialIdSource::new(1000)),
    );
    bootstrapper.register_manifest(qqbot_adapter_manifest(1, false));
    bootstrapper.register_builtin_runner(Box::new(gateway_runner));
    bootstrapper.register_builtin_runner(Box::new(openapi_runner));

    let router_runner = BotEventRouterRunner::new(
        1,
        vec![BotEventSubscription {
            subscription_id: "qqbot-message-to-command".into(),
            handler_protocol_id: BOT_COMMAND_PARSE_PROTOCOL_ID.into(),
            handler_binding_id: None,
            platform: Some("qqbot".into()),
            event_kind: Some(BotEventKind::MessageCreated),
        }],
    );
    bootstrapper.register_manifest(bot_event_router_manifest(1));
    bootstrapper.register_builtin_runner(Box::new(router_runner));

    let command_runner = BotCommandRunner::new(1, vec!["/".into()]);
    bootstrapper.register_manifest(bot_command_manifest(1));
    bootstrapper.register_builtin_runner(Box::new(command_runner));

    bootstrapper.register_manifest(echo_manifest(1));
    bootstrapper.register_builtin_runner(echo_runner(1));

    let profile = RuntimeProfile {
        profile_id: "qqbot-echo-smoke".into(),
        mode: RuntimeProfileMode::FullDev,
        enabled_plugins: vec![
            QQBOT_ADAPTER_PLUGIN_ID.into(),
            BOT_EVENT_ROUTER_PLUGIN_ID.into(),
            BOT_COMMAND_PLUGIN_ID.into(),
            ECHO_PLUGIN_ID.into(),
        ],
        bindings: BTreeMap::new(),
        plugin_deployments: BTreeMap::new(),
        allow_dynamic_registration: false,
        allow_hot_reload: false,
    };

    (bootstrapper, profile)
}

pub fn qqbot_group_message_task(config: &EchoSmokeConfig) -> Task {
    Task::new(
        "smoke.qqbot.gateway.group_message",
        QQBOT_GATEWAY_FRAME_PROTOCOL_ID,
        json!({
            "op": 0,
            "s": 1,
            "t": "GROUP_MESSAGE_CREATE",
            "id": "GROUP_MESSAGE_CREATE:smoke",
            "d": {
                "id": "QQBOT_MESSAGE_ID",
                "group_openid": config.group_openid,
                "content": config.command_text,
                "timestamp": 1_725_000_000_000i64,
                "author": {
                    "user_openid": config.user_openid,
                    "username": "smoke-user"
                }
            }
        }),
    )
}

fn qqbot_config(config: &EchoSmokeConfig) -> QqBotConfig {
    let mut qqbot = QqBotConfig::new(&config.account_id, &config.app_id);
    qqbot.token_url = "https://qqbot.example.invalid/app/getAppAccessToken".into();
    qqbot.openapi_base_url = "https://qqbot.example.invalid".into();
    qqbot.max_retry_attempts = 1;
    qqbot
}

struct RecordingQqHttpClient {
    requests: Arc<Mutex<Vec<QqHttpRequest>>>,
    responses: Mutex<VecDeque<Result<QqHttpResponse, QqOpenApiError>>>,
}

impl RecordingQqHttpClient {
    fn new(requests: Arc<Mutex<Vec<QqHttpRequest>>>) -> Self {
        Self {
            requests,
            responses: Mutex::new(VecDeque::from([
                Ok(QqHttpResponse {
                    status: 200,
                    headers: BTreeMap::new(),
                    body: json!({"access_token": "SMOKE_TOKEN", "expires_in": 7200}),
                }),
                Ok(QqHttpResponse {
                    status: 200,
                    headers: BTreeMap::new(),
                    body: json!({"id": "QQBOT_ECHO_REPLY_ID"}),
                }),
                Ok(QqHttpResponse {
                    status: 200,
                    headers: BTreeMap::new(),
                    body: json!({"id": "QQBOT_DIRECT_MESSAGE_ID"}),
                }),
            ])),
        }
    }
}

impl QqHttpClient for RecordingQqHttpClient {
    fn send(&mut self, request: QqHttpRequest) -> Result<QqHttpResponse, QqOpenApiError> {
        self.requests.lock().unwrap().push(request);
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| {
                Ok(QqHttpResponse {
                    status: 200,
                    headers: BTreeMap::new(),
                    body: json!({"ok": true}),
                })
            })
    }
}

struct SequentialIdSource {
    next: u64,
}

impl SequentialIdSource {
    fn new(next: u64) -> Self {
        Self { next }
    }
}

impl QqIdSource for SequentialIdSource {
    fn next_msg_seq(&mut self) -> u64 {
        let next = self.next;
        self.next += 1;
        next
    }
}

pub fn request_log_json(requests: &[QqHttpRequest]) -> Value {
    Value::Array(
        requests
            .iter()
            .map(|request| {
                json!({
                    "method": http_method_name(&request.method),
                    "url": request.url,
                    "headers": redact_headers(&request.headers),
                    "body": request.body.as_ref().map(redact_body),
                    "binary_body_len": request.binary_body.as_ref().map(Vec::len),
                })
            })
            .collect(),
    )
}

fn redact_headers(headers: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    headers
        .iter()
        .map(|(key, value)| {
            if key.eq_ignore_ascii_case("authorization") {
                (key.clone(), "<redacted>".into())
            } else {
                (key.clone(), value.clone())
            }
        })
        .collect()
}

fn redact_body(body: &Value) -> Value {
    let mut redacted = body.clone();
    if let Value::Object(map) = &mut redacted {
        for key in ["clientSecret", "client_secret", "access_token", "token"] {
            if map.contains_key(key) {
                map.insert(key.into(), Value::String("<redacted>".into()));
            }
        }
    }
    redacted
}

fn http_method_name(method: &HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Delete => "DELETE",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_runtime_contracts::PluginManifest;
    use mutsuki_runtime_host::resolve_load_plan;

    #[test]
    fn qqbot_gateway_event_reaches_echo_send_request() {
        let report = run_default_smoke().unwrap();

        assert_eq!(report.requests.len(), 2);
        assert_eq!(report.requests[1].method, HttpMethod::Post);
        assert!(
            report.requests[1]
                .url
                .ends_with("/v2/groups/GROUP_OPENID/messages")
        );
        assert_eq!(
            report.requests[1].body.as_ref().unwrap()["content"],
            "hello from qqbot"
        );
        assert_eq!(report.requests[1].body.as_ref().unwrap()["msg_seq"], 1000);
        assert_eq!(
            report.requests[1].body.as_ref().unwrap()["msg_id"],
            "QQBOT_MESSAGE_ID"
        );
    }

    #[test]
    fn ping_command_replies_pong_through_standard_message_task() {
        let mut config = EchoSmokeConfig::default();
        config.command_text = "/ping".into();
        let report = run_smoke(config).unwrap();

        assert_eq!(report.requests.len(), 2);
        assert_eq!(report.requests[1].body.as_ref().unwrap()["content"], "pong");
    }

    #[test]
    fn generated_plugin_manifests_roundtrip_and_resolve() {
        let manifests = vec![
            qqbot_adapter_manifest(1, false),
            bot_event_router_manifest(1),
            bot_command_manifest(1),
            echo_manifest(1),
        ]
        .into_iter()
        .map(|manifest| {
            let encoded = serde_json::to_value(&manifest).unwrap();
            let decoded: PluginManifest = serde_json::from_value(encoded).unwrap();
            assert_eq!(decoded, manifest);
            assert!(decoded.provides.runners.iter().all(|runner| {
                runner.batch.max_batch_entries > 0
                    && !runner.payload.layouts.is_empty()
                    && runner.ordering.supports_sequence
                    && runner.control.batch_cancel
            }));
            decoded
        })
        .collect::<Vec<_>>();
        let profile = RuntimeProfile {
            profile_id: "manifest-roundtrip".into(),
            mode: RuntimeProfileMode::FullDev,
            enabled_plugins: vec![
                QQBOT_ADAPTER_PLUGIN_ID.into(),
                BOT_EVENT_ROUTER_PLUGIN_ID.into(),
                BOT_COMMAND_PLUGIN_ID.into(),
                ECHO_PLUGIN_ID.into(),
            ],
            bindings: BTreeMap::new(),
            plugin_deployments: BTreeMap::new(),
            allow_dynamic_registration: false,
            allow_hot_reload: false,
        };

        let plan = resolve_load_plan(&manifests, &profile).unwrap();

        assert_eq!(plan.plugins.len(), 4);
        assert_eq!(
            plan.plugins
                .iter()
                .flat_map(|manifest| manifest.provides.runners.iter())
                .count(),
            5
        );
    }
}
