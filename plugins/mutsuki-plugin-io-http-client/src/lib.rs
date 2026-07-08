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
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "mutsuki.std.io.http_client";
pub const RUNNER_ID: &str = "mutsuki.std.io.http_client.runner";
pub const EFFECT_RUNNER_ID: &str = "effect.mutsuki.std.io.http_client.runner";
pub const HTTP_REQUEST_PROTOCOL: &str = "mutsuki.http.request";
pub const EFFECT_HTTP_REQUEST_PROTOCOL: &str = "effect.mutsuki.http.request";

#[derive(Clone)]
pub struct HttpFacadeRunner {
    descriptor: RunnerDescriptor,
}

impl HttpFacadeRunner {
    pub fn new() -> Self {
        Self {
            descriptor: facade_runner_descriptor(),
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
        map_work_batch_entries(&batch, facade_result)
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
            ..Default::default()
        })
        .metadata(
            "standard_plugin",
            ScalarValue::String("io_http_client".into()),
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

fn facade_result(task: &Task) -> Result<RunnerResult, RuntimeError> {
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
    let mut effect_task = Task::new(
        format!("{}:effect", task.task_id),
        EFFECT_HTTP_REQUEST_PROTOCOL,
        task.payload.clone(),
    );
    effect_task.runner_hint = Some(EFFECT_RUNNER_ID.into());
    effect_task.trace_id = task.trace_id.clone();
    effect_task.correlation_id = task
        .correlation_id
        .clone()
        .or_else(|| Some(task.task_id.clone()));

    let mut result = RunnerResult::completed(task.task_id.clone());
    result.tasks.push(effect_task);
    result.events.push(DomainEvent {
        event_id: format!("event:{}:queued", task.task_id),
        kind: HTTP_REQUEST_PROTOCOL.into(),
        payload: json!({"effect_task_id": format!("{}:effect", task.task_id)}),
    });
    Ok(result)
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
    }
}
