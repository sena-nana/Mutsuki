use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use mutsuki_runtime_contracts::{
    CompletionBatch, DomainEvent, ExecutionClass, RunnerBatchCapability, RunnerContext,
    RunnerDescriptor, RunnerMode, RunnerPurity, RunnerResult, RunnerSideEffect, RuntimeError,
    ScalarValue, Task, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RuntimeResult};
use mutsuki_runtime_sdk::{
    LoadedPlugin, Plugin, PluginBuilder, ProtocolDescriptorBuilder, RunnerDescriptorBuilder,
    map_work_batch_entries,
};
use mutsuki_std_effect::{
    EffectObservation, ProtocolPair, ProtocolPairTable, derive_effect_from_pair,
};
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "mutsuki.std.io.http_client";
pub const RUNNER_ID: &str = "mutsuki.std.io.http_client.runner";
pub const EFFECT_RUNNER_ID: &str = "effect.mutsuki.std.io.http_client.runner";
pub const HTTP_REQUEST_PROTOCOL: &str = "mutsuki.http.request";
pub const EFFECT_HTTP_REQUEST_PROTOCOL: &str = "effect.mutsuki.http.request";

const HTTP_PROTOCOL_PAIRS: &[ProtocolPair] = &[ProtocolPair {
    public: HTTP_REQUEST_PROTOCOL,
    effect: EFFECT_HTTP_REQUEST_PROTOCOL,
    queued_event_kind: "mutsuki.effect.http.queued",
}];

pub const HTTP_PROTOCOL_TABLE: ProtocolPairTable = ProtocolPairTable::new(HTTP_PROTOCOL_PAIRS);

#[derive(Clone)]
pub struct HttpFacadeRunner {
    descriptor: RunnerDescriptor,
    observation: EffectObservation,
}

impl HttpFacadeRunner {
    pub fn new() -> Self {
        Self::with_observation(EffectObservation::Quiet)
    }

    pub fn with_observation(observation: EffectObservation) -> Self {
        Self {
            descriptor: facade_runner_descriptor(),
            observation,
        }
    }
}

impl Default for HttpFacadeRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for HttpFacadeRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        let observation = self.observation;
        map_work_batch_entries(&batch, |task| facade_result(task, observation))
    }
}

#[derive(Clone)]
pub struct HttpEffectRunner {
    descriptor: RunnerDescriptor,
}

impl HttpEffectRunner {
    pub fn new() -> Self {
        Self {
            descriptor: effect_runner_descriptor(),
        }
    }
}

impl Default for HttpEffectRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for HttpEffectRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, effect_result)
    }
}

pub fn loaded_plugin() -> LoadedPlugin {
    plugin_builder().build()
}

pub fn plugin() -> impl Plugin {
    plugin_builder()
}

fn plugin_builder() -> PluginBuilder {
    PluginBuilder::new(PLUGIN_ID)
        .runner(Box::new(HttpFacadeRunner::new()))
        .runner(Box::new(HttpEffectRunner::new()))
        .protocol_handler(protocol_descriptor(HTTP_REQUEST_PROTOCOL), RUNNER_ID, "io")
        .protocol_descriptor(protocol_descriptor(EFFECT_HTTP_REQUEST_PROTOCOL))
}

fn facade_runner_descriptor() -> RunnerDescriptor {
    RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID)
        .accepted_protocol(HTTP_REQUEST_PROTOCOL)
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
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::ScalarAdapter,
            side_effect: RunnerSideEffect::External,
            max_inflight_batches: 1,
            preserve_order: true,
            ..Default::default()
        })
        .metadata(
            "standard_plugin",
            ScalarValue::String("io_http_client".into()),
        )
        .metadata(
            "effect_execution",
            ScalarValue::String("blocking_io_isolated".into()),
        )
        .build()
}

fn protocol_descriptor(protocol_id: &str) -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(protocol_id)
        .input_schema(json!({
            "type": "object",
            "required": ["url", "domain_allowlist"],
            "properties": {
                "method": {"type": "string"},
                "url": {"type": "string"},
                "headers": {"type": "object"},
                "body": {"type": "string"},
                "timeout_ms": {"type": "integer"},
                "domain_allowlist": {"type": "array"},
                "deny_private_network": {"type": "boolean"}
            }
        }))
        .output_schema(json!({"type": "object"}))
        .error_schema(json!({"type": "object"}))
        .build()
}

fn facade_result(
    task: &Task,
    observation: EffectObservation,
) -> Result<RunnerResult, RuntimeError> {
    let request = parse_request(task)?;
    ensure_domain_allowed(task, &request.host)?;
    if task
        .payload
        .get("deny_private_network")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && is_private_host(&request.host)
    {
        return Err(http_error(
            task,
            "mutsuki.http.private_network_denied",
            "private_network.denied",
        ));
    }
    derive_effect_from_pair(task, &HTTP_PROTOCOL_TABLE, EFFECT_RUNNER_ID, observation).map_err(
        |_| {
            RuntimeError::new(
                mutsuki_runtime_contracts::ERR_TASK_UNSUPPORTED,
                "runtime.io_http_client",
                format!("io_http.protocol.{}", task.protocol_id),
            )
        },
    )
}

fn effect_result(task: &Task) -> Result<RunnerResult, RuntimeError> {
    if task.protocol_id != EFFECT_HTTP_REQUEST_PROTOCOL {
        return Err(RuntimeError::new(
            mutsuki_runtime_contracts::ERR_TASK_UNSUPPORTED,
            "runtime.io_http_client",
            format!("io_http.protocol.{}", task.protocol_id),
        ));
    }
    let request = parse_request(task)?;
    ensure_domain_allowed(task, &request.host)?;
    let response = execute_http(task, &request)?;
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.events.push(DomainEvent {
        event_id: format!("event:{}.http_request", task.task_id),
        kind: HTTP_REQUEST_PROTOCOL.into(),
        payload: response,
    });
    Ok(result)
}

struct HttpRequest {
    method: String,
    host: String,
    port: u16,
    path: String,
    body: String,
    timeout_ms: u64,
}

fn parse_request(task: &Task) -> Result<HttpRequest, RuntimeError> {
    let url = task
        .payload
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| http_error(task, "mutsuki.http.invalid_url", "url.missing"))?;
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| http_error(task, "mutsuki.http.invalid_url", "scheme.unsupported"))?;
    let (authority, path) = rest
        .split_once('/')
        .map(|(authority, path)| (authority, format!("/{path}")))
        .unwrap_or((rest, "/".into()));
    let (host, port) = authority
        .rsplit_once(':')
        .and_then(|(host, port)| port.parse::<u16>().ok().map(|port| (host, port)))
        .unwrap_or((authority, 80));
    let method = task
        .payload
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET")
        .to_ascii_uppercase();
    if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE") {
        return Err(http_error(
            task,
            "mutsuki.http.invalid_method",
            format!("method.{method}"),
        ));
    }
    Ok(HttpRequest {
        method,
        host: host.to_string(),
        port,
        path,
        body: task
            .payload
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        timeout_ms: task
            .payload
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(5000),
    })
}

fn ensure_domain_allowed(task: &Task, host: &str) -> Result<(), RuntimeError> {
    let allowlist = task
        .payload
        .get("domain_allowlist")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            http_error(
                task,
                "mutsuki.http.domain_denied",
                "domain_allowlist.missing",
            )
        })?;
    if allowlist.iter().filter_map(Value::as_str).any(|allowed| {
        allowed == host
            || host
                .strip_suffix(allowed)
                .is_some_and(|prefix| prefix.ends_with('.'))
    }) {
        return Ok(());
    }
    Err(http_error(
        task,
        "mutsuki.http.domain_denied",
        format!("domain.{host}"),
    ))
}

fn execute_http(task: &Task, request: &HttpRequest) -> Result<Value, RuntimeError> {
    let mut stream = TcpStream::connect((request.host.as_str(), request.port))
        .map_err(|error| io_error(task, "mutsuki.http.connect_failed", error))?;
    let timeout = Some(Duration::from_millis(request.timeout_ms));
    stream
        .set_read_timeout(timeout)
        .map_err(|error| io_error(task, "mutsuki.http.timeout_failed", error))?;
    stream
        .set_write_timeout(timeout)
        .map_err(|error| io_error(task, "mutsuki.http.timeout_failed", error))?;
    let headers = header_lines(task);
    let request_text = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: {}\r\n{}\r\n{}",
        request.method,
        request.path,
        request.host,
        request.body.len(),
        headers,
        request.body
    );
    stream
        .write_all(request_text.as_bytes())
        .map_err(|error| io_error(task, "mutsuki.http.write_failed", error))?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| io_error(task, "mutsuki.http.read_failed", error))?;
    let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .unwrap_or(0);
    Ok(json!({
        "status": status,
        "body": body,
        "raw_headers": head,
    }))
}

fn header_lines(task: &Task) -> String {
    task.payload
        .get("headers")
        .and_then(Value::as_object)
        .map(|headers| {
            headers
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| format!("{key}: {value}\r\n"))
                })
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn is_private_host(host: &str) -> bool {
    host == "localhost"
        || host.starts_with("127.")
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        || host.starts_with("172.16.")
}

fn http_error(task: &Task, code: impl Into<String>, message: impl Into<String>) -> RuntimeError {
    let mut error = RuntimeError::new(code, "runtime.io_http_client", message);
    error
        .evidence
        .insert("task_id".into(), ScalarValue::String(task.task_id.clone()));
    error
}

fn io_error(task: &Task, code: impl Into<String>, error: std::io::Error) -> RuntimeError {
    http_error(task, code, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_runtime_contracts::{
        BatchEntry, BatchPayload, DispatchLane, OrderingRequirement, RunnerContext,
        WorkResourcePlan,
    };
    use mutsuki_runtime_core::Runner;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn work_batch(runner_id: &str, task: Task) -> WorkBatch {
        WorkBatch {
            batch_id: format!("batch-{}", task.task_id),
            tick_id: "tick-1".into(),
            batch_key: runner_id.into(),
            entries: vec![BatchEntry {
                entry_id: task.task_id.clone(),
                task_id: task.task_id.clone(),
                trace_id: None,
                parent_id: None,
                payload_index: 0,
                resource_requirement_indices: Vec::new(),
                cancel_index: Some(0),
                deadline_tick: None,
                priority: 0,
                lane: DispatchLane::Normal,
                ordering: OrderingRequirement::None,
            }],
            payload: BatchPayload::from_local_tasks(vec![task]),
            resource_plan: WorkResourcePlan::empty(),
            task_leases: Vec::new(),
        }
    }

    fn ctx() -> RunnerContext {
        RunnerContext::new(1, 1, "executor-1", None, "invoke-1")
    }

    #[test]
    fn loaded_plugin_declares_http_protocols_and_runners() {
        let plugin = loaded_plugin();
        assert_eq!(plugin.manifest.plugin_id, PLUGIN_ID);
        assert_eq!(plugin.manifest.provides.runners[0].runner_id, RUNNER_ID);
        assert_eq!(
            plugin.manifest.provides.runners[1].runner_id,
            EFFECT_RUNNER_ID
        );
        assert_eq!(plugin.manifest.provides.protocols.len(), 2);
        assert_eq!(plugin.manifest.provides.handler_bindings.len(), 1);
        assert_eq!(
            plugin.manifest.provides.runners[0].purity,
            RunnerPurity::Pure
        );
        assert_eq!(
            plugin.manifest.provides.runners[1].purity,
            RunnerPurity::Effectful
        );
        HTTP_PROTOCOL_TABLE.validate_unique().unwrap();
    }

    #[test]
    fn facade_shares_payload_without_queued_event() {
        let task = Task::new(
            "http-1",
            HTTP_REQUEST_PROTOCOL,
            json!({
                "url": "http://127.0.0.1:9/ping",
                "domain_allowlist": ["127.0.0.1"],
                "body": "x".repeat(4096),
            }),
        );
        let mut facade = HttpFacadeRunner::new();
        let completion = facade
            .run_batch(ctx(), work_batch(RUNNER_ID, task.clone()))
            .unwrap();
        let derived = completion.results[0].result.as_ref().unwrap();
        assert!(derived.events.is_empty());
        assert_eq!(derived.tasks[0].protocol_id, EFFECT_HTTP_REQUEST_PROTOCOL);
        assert_eq!(task.payload.strong_count(), 2);
        assert_eq!(derived.tasks[0].payload.strong_count(), 2);
    }

    #[test]
    fn effect_runner_hits_loopback_fixture() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\nX-Fixture: v1\r\n\r\nmutsuki-fixed",
                )
                .unwrap();
        });
        let task = Task::new(
            "http-effect",
            EFFECT_HTTP_REQUEST_PROTOCOL,
            json!({
                "method": "GET",
                "url": format!("http://127.0.0.1:{}/fixture", address.port()),
                "domain_allowlist": ["127.0.0.1"],
                "deny_private_network": false
            }),
        );
        let mut effect = HttpEffectRunner::new();
        let done = effect
            .run_batch(ctx(), work_batch(EFFECT_RUNNER_ID, task))
            .unwrap();
        assert!(
            done.results[0].result.is_some(),
            "effect failed: {:?}",
            done.results[0].error
        );
        server.join().unwrap();
    }
}
