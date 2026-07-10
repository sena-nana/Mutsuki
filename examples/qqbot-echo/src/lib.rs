use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use mutsuki_bot_protocol::{
    BOT_COMMAND_HANDLE_PROTOCOL_ID, BOT_COMMAND_PARSE_PROTOCOL_ID, BOT_MESSAGE_SEND_PROTOCOL_ID,
    BotCommandEvent, BotEventKind, BotEventSubscription, BotMessage,
};
use mutsuki_plugin_bot_adapter_qqbot::api::{
    HttpMethod, MediaChunk, QqMediaError, QqMediaProvider, QqOpenApiError,
};
use mutsuki_plugin_bot_adapter_qqbot::tasks::{
    QQBOT_ADAPTER_PLUGIN_ID, QQBOT_GATEWAY_FRAME_PROTOCOL_ID, QqGatewayMapRunner, QqOpenApiRunner,
};
use mutsuki_plugin_bot_adapter_qqbot::{
    QqBotClients, QqBotConfig, QqHttpClient, QqHttpRequest, QqHttpResponse, QqIdSource,
};
use mutsuki_plugin_bot_command::{BOT_COMMAND_PLUGIN_ID, BotCommandRunner};
use mutsuki_plugin_bot_event_router::{BOT_EVENT_ROUTER_PLUGIN_ID, BotEventRouterRunner};
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, OrderingRequirement, RunnerBatchCapability,
    RunnerControlCapability, RunnerDescriptor, RunnerMode, RunnerOrderingCapability,
    RunnerPayloadCapability, RunnerPurity, RunnerResourceCapability, RunnerSideEffect,
    RuntimeError, RuntimeProfile, RuntimeProfileMode, ScalarValue, Task, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_host::{RuntimeBootstrapper, runner_manifest};
use mutsuki_runtime_sdk::map_work_batch_entries;
use serde_json::{Value, json};

pub const ECHO_PLUGIN_ID: &str = "example.qqbot.echo";
pub const ECHO_RUNNER_ID: &str = "example.qqbot.echo.command";

#[derive(Clone, Debug)]
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

impl EchoSmokeConfig {
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            account_id: env_or("QQBOT_ACCOUNT_ID", defaults.account_id),
            app_id: env_or("QQBOT_APP_ID", defaults.app_id),
            client_secret: env_or("QQBOT_CLIENT_SECRET", defaults.client_secret),
            group_openid: env_or("QQBOT_GROUP_OPENID", defaults.group_openid),
            user_openid: env_or("QQBOT_USER_OPENID", defaults.user_openid),
            command_text: env_or("QQBOT_ECHO_TEXT", defaults.command_text),
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
            Box::new(NoopMediaProvider),
        ),
        Box::new(SequentialIdSource::new(1000)),
    );
    let adapter_manifest = runner_manifest(
        QQBOT_ADAPTER_PLUGIN_ID,
        vec![
            gateway_runner.descriptor().clone(),
            openapi_runner.descriptor().clone(),
        ],
    );
    bootstrapper.register_manifest(adapter_manifest);
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
    bootstrapper.register_manifest(runner_manifest(
        BOT_EVENT_ROUTER_PLUGIN_ID,
        vec![router_runner.descriptor().clone()],
    ));
    bootstrapper.register_builtin_runner(Box::new(router_runner));

    let command_runner = BotCommandRunner::new(1, vec!["/".into()]);
    bootstrapper.register_manifest(runner_manifest(
        BOT_COMMAND_PLUGIN_ID,
        vec![command_runner.descriptor().clone()],
    ));
    bootstrapper.register_builtin_runner(Box::new(command_runner));

    let echo_runner = EchoCommandRunner::new(1);
    bootstrapper.register_manifest(runner_manifest(
        ECHO_PLUGIN_ID,
        vec![echo_runner.descriptor().clone()],
    ));
    bootstrapper.register_builtin_runner(Box::new(echo_runner));

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
    let mut qqbot = QqBotConfig::new(&config.account_id, &config.app_id, &config.client_secret);
    qqbot.token_url = "https://qqbot.example.invalid/app/getAppAccessToken".into();
    qqbot.openapi_base_url = "https://qqbot.example.invalid".into();
    qqbot.max_retry_attempts = 1;
    qqbot
}

struct EchoCommandRunner {
    descriptor: RunnerDescriptor,
}

impl EchoCommandRunner {
    pub fn new(plugin_generation: u64) -> Self {
        Self {
            descriptor: echo_descriptor(plugin_generation),
        }
    }
}

impl Runner for EchoCommandRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            let command: BotCommandEvent = serde_json::from_value(task.payload.clone())
                .map_err(|error| echo_error(format!("echo.command.decode:{error}")))?;
            let mut result =
                mutsuki_runtime_contracts::RunnerResult::completed(task.task_id.clone());
            if command.name == "echo" {
                let text = command.args.join(" ");
                let mut send = Task::new(
                    format!("example.qqbot.echo.send:{}", command.source.event_id),
                    BOT_MESSAGE_SEND_PROTOCOL_ID,
                    serde_json::to_value(BotMessage::text(command.source.target, text))
                        .map_err(|error| echo_error(format!("echo.message.encode:{error}")))?,
                );
                send.registry_generation = ctx.registry_generation;
                result.tasks.push(send);
            }
            Ok(result)
        })
    }
}

fn echo_descriptor(plugin_generation: u64) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: ECHO_RUNNER_ID.into(),
        plugin_id: ECHO_PLUGIN_ID.into(),
        plugin_generation,
        accepted_protocol_ids: vec![BOT_COMMAND_HANDLE_PROTOCOL_ID.into()],
        purity: RunnerPurity::Pure,
        execution_class: ExecutionClass::Orchestration,
        input_schema: json!({
            "type": "object",
            "required": ["source", "name", "args"]
        }),
        output_schema: json!({
            "tasks": [BOT_MESSAGE_SEND_PROTOCOL_ID]
        }),
        batch: RunnerBatchCapability {
            mode: RunnerMode::NativeBatch,
            preferred_batch_size: 16,
            max_batch_entries: 64,
            side_effect: RunnerSideEffect::None,
            ..Default::default()
        },
        payload: RunnerPayloadCapability::default(),
        resources: RunnerResourceCapability {
            requires_resource_plan: false,
            ..Default::default()
        },
        ordering: RunnerOrderingCapability {
            default: OrderingRequirement::PreserveSubmitOrder,
            supports_sequence: true,
            supports_same_resource_order: true,
        },
        control: RunnerControlCapability::default(),
        metadata: BTreeMap::from([(
            "description".into(),
            ScalarValue::String("Example echo command handler".into()),
        )]),
        contract_surfaces: vec![
            format!("runner:{ECHO_RUNNER_ID}"),
            format!("task_protocol:{BOT_COMMAND_HANDLE_PROTOCOL_ID}"),
        ],
    }
}

fn echo_error(route: impl Into<String>) -> RuntimeError {
    RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        ECHO_PLUGIN_ID,
        route,
    )
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
                    body: json!({"access_token": "SMOKE_TOKEN", "expires_in": 7200}),
                }),
                Ok(QqHttpResponse {
                    status: 200,
                    body: json!({"id": "QQBOT_ECHO_REPLY_ID"}),
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
                    body: json!({"ok": true}),
                })
            })
    }
}

struct NoopMediaProvider;

impl QqMediaProvider for NoopMediaProvider {
    fn read_chunks(
        &mut self,
        _resource_ref: &str,
        _block_size: u64,
    ) -> Result<Vec<MediaChunk>, QqMediaError> {
        Ok(Vec::new())
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

fn env_or(key: &str, fallback: String) -> String {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
