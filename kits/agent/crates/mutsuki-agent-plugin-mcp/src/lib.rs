use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use mutsuki_agent_contracts::{
    AgentError, AgentKitPluginDescriptor, AgentPluginStateKind, AgentPromptFragment,
    AgentServiceDescriptor, AgentToolDescriptor, ContextItemRef, ContextPriority,
    ContextProvenance, ContextProviderRequest, ContextProviderResult, McpCallResult, McpCatalog,
    McpPromptArgument, McpPromptGetResult, McpPromptInfo, McpResourceInfo, McpResourceReadResult,
    McpServerManifest, McpServerState, McpServerStatus, McpServiceRequest, McpServiceResponse,
    McpToolAnnotations, McpToolInfo, McpTransportKind, ToolSideEffect, mcp_namespaced_name,
};
use mutsuki_agent_plugin_api::{AgentPluginRegistrar, AgentService, ContextProvider, ToolProvider};
use mutsuki_agent_runtime::AgentResourceStore;
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.mcp";
pub const SERVICE_ID: &str = "mutsuki.agent.service.mcp";
pub const CONTEXT_PROVIDER_ID: &str = "mutsuki.agent.context.mcp";
pub const PROTOCOL_VERSION: &str = "2024-11-05";
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
pub const INLINE_LIMIT: usize = 64 * 1024;

pub trait McpTransport: Send {
    fn send(&mut self, value: &Value) -> Result<(), AgentError>;
    fn receive(&mut self, timeout: Duration) -> Result<Option<Value>, AgentError>;
    fn is_alive(&mut self) -> Result<bool, AgentError>;
    fn terminate(&mut self) -> Result<(), AgentError>;
}

pub trait McpTransportFactory: Send + Sync {
    fn open(&self, manifest: &McpServerManifest) -> Result<Box<dyn McpTransport>, AgentError>;
}

pub trait McpHttpClient: Send + Sync {
    fn post_json(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &Value,
        timeout: Duration,
    ) -> Result<Value, AgentError>;
}

#[derive(Default)]
pub struct StdioMcpTransportFactory;

impl McpTransportFactory for StdioMcpTransportFactory {
    fn open(&self, manifest: &McpServerManifest) -> Result<Box<dyn McpTransport>, AgentError> {
        if manifest.transport != McpTransportKind::Stdio {
            return Err(AgentError::invalid_input(
                "StdioMcpTransportFactory requires stdio transport",
            ));
        }
        let command = manifest
            .command
            .as_deref()
            .ok_or_else(|| AgentError::invalid_input("stdio MCP manifest requires command"))?;
        let mut process = Command::new(command);
        process
            .args(&manifest.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env_clear();
        for (key, value) in &manifest.env_allowlist {
            process.env(key, value);
        }
        let mut child = process
            .spawn()
            .map_err(|error| AgentError::new("agent.mcp.spawn_failed", error.to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AgentError::new("agent.mcp.spawn_failed", "stdin is unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AgentError::new("agent.mcp.spawn_failed", "stdout is unavailable"))?;
        let (messages, receiver) = mpsc::sync_channel(64);
        let server_id = manifest.server_id.clone();
        thread::Builder::new()
            .name(format!("mutsuki-mcp-reader-{server_id}"))
            .spawn(move || {
                let mut stdout = BufReader::new(stdout);
                loop {
                    let result = read_mcp_frame(&mut stdout);
                    let stop = result.is_err();
                    if messages.send(result).is_err() || stop {
                        break;
                    }
                }
            })
            .map_err(|error| AgentError::new("agent.mcp.spawn_failed", error.to_string()))?;
        Ok(Box::new(StdioMcpTransport {
            child,
            stdin: BufWriter::new(stdin),
            receiver,
        }))
    }
}

struct StdioMcpTransport {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    receiver: Receiver<Result<Value, AgentError>>,
}

impl McpTransport for StdioMcpTransport {
    fn send(&mut self, value: &Value) -> Result<(), AgentError> {
        let body = serde_json::to_vec(value)
            .map_err(|error| AgentError::new("agent.mcp.encode_failed", error.to_string()))?;
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len())
            .and_then(|_| self.stdin.write_all(&body))
            .and_then(|_| self.stdin.flush())
            .map_err(|error| AgentError::new("agent.mcp.write_failed", error.to_string()))
    }

    fn receive(&mut self, timeout: Duration) -> Result<Option<Value>, AgentError> {
        match self.receiver.recv_timeout(timeout) {
            Ok(result) => result.map(Some),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(AgentError::new(
                "agent.mcp.closed",
                "MCP server response stream closed",
            )),
        }
    }

    fn is_alive(&mut self) -> Result<bool, AgentError> {
        self.child
            .try_wait()
            .map(|status| status.is_none())
            .map_err(|error| AgentError::new("agent.mcp.crashed", error.to_string()))
    }

    fn terminate(&mut self) -> Result<(), AgentError> {
        if self
            .child
            .try_wait()
            .map_err(|error| AgentError::new("agent.mcp.crashed", error.to_string()))?
            .is_none()
        {
            self.child
                .kill()
                .map_err(|error| AgentError::new("agent.mcp.crashed", error.to_string()))?;
        }
        let _ = self.child.wait();
        Ok(())
    }
}

fn read_mcp_frame(stdout: &mut BufReader<ChildStdout>) -> Result<Value, AgentError> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        stdout
            .read_line(&mut line)
            .map_err(|error| AgentError::new("agent.mcp.read_failed", error.to_string()))?;
        if line.is_empty() {
            return Err(AgentError::new(
                "agent.mcp.closed",
                "MCP server closed stdout",
            ));
        }
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = Some(value.trim().parse::<usize>().map_err(|_| {
                AgentError::new("agent.mcp.invalid_frame", "invalid Content-Length")
            })?);
        }
    }
    let length = content_length
        .ok_or_else(|| AgentError::new("agent.mcp.invalid_frame", "Content-Length is missing"))?;
    if length > 16 * 1024 * 1024 {
        return Err(AgentError::new(
            "agent.mcp.frame_too_large",
            "MCP frame exceeds 16 MiB",
        ));
    }
    let mut body = vec![0; length];
    stdout
        .read_exact(&mut body)
        .map_err(|error| AgentError::new("agent.mcp.read_failed", error.to_string()))?;
    serde_json::from_slice(&body)
        .map_err(|error| AgentError::new("agent.mcp.encode_failed", error.to_string()))
}

#[derive(Default)]
pub struct ReqwestMcpHttpClient;

impl McpHttpClient for ReqwestMcpHttpClient {
    fn post_json(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &Value,
        timeout: Duration,
    ) -> Result<Value, AgentError> {
        let mut request = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|error| AgentError::new("agent.mcp.request_failed", error.to_string()))?
            .post(url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .json(body);
        for (key, value) in headers {
            request = request.header(key, value);
        }
        let response = request
            .send()
            .map_err(|error| AgentError::new("agent.mcp.request_failed", error.to_string()))?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let text = response
            .text()
            .map_err(|error| AgentError::new("agent.mcp.read_failed", error.to_string()))?;
        if !(200..300).contains(&status) {
            return Err(AgentError::new(
                "agent.mcp.request_failed",
                format!("HTTP {status}: {text}"),
            ));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        parse_http_mcp_body(&content_type, &text)
    }
}

fn parse_http_mcp_body(content_type: &str, text: &str) -> Result<Value, AgentError> {
    if content_type.contains("text/event-stream") {
        for line in text.lines() {
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<Value>(data) {
                return Ok(value);
            }
        }
        return Err(AgentError::new(
            "agent.mcp.request_failed",
            "SSE response did not contain JSON-RPC data",
        ));
    }
    serde_json::from_str(text)
        .map_err(|error| AgentError::new("agent.mcp.encode_failed", error.to_string()))
}

pub struct HttpMcpTransport {
    url: String,
    headers: Vec<(String, String)>,
    client: Arc<dyn McpHttpClient>,
    pending: VecDeque<Value>,
    closed: bool,
}

impl HttpMcpTransport {
    pub fn new(
        url: impl Into<String>,
        headers: Vec<(String, String)>,
        client: Arc<dyn McpHttpClient>,
    ) -> Self {
        Self {
            url: url.into(),
            headers,
            client,
            pending: VecDeque::new(),
            closed: false,
        }
    }
}

impl McpTransport for HttpMcpTransport {
    fn send(&mut self, value: &Value) -> Result<(), AgentError> {
        if self.closed {
            return Err(AgentError::new(
                "agent.mcp.closed",
                "HTTP MCP transport is closed",
            ));
        }
        if value.get("id").is_none() {
            // notifications are fire-and-forget over streamable HTTP
            let _ =
                self.client
                    .post_json(&self.url, &self.headers, value, DEFAULT_REQUEST_TIMEOUT)?;
            return Ok(());
        }
        let response =
            self.client
                .post_json(&self.url, &self.headers, value, DEFAULT_REQUEST_TIMEOUT)?;
        self.pending.push_back(response);
        Ok(())
    }

    fn receive(&mut self, timeout: Duration) -> Result<Option<Value>, AgentError> {
        if let Some(value) = self.pending.pop_front() {
            return Ok(Some(value));
        }
        thread::sleep(timeout.min(Duration::from_millis(1)));
        Ok(None)
    }

    fn is_alive(&mut self) -> Result<bool, AgentError> {
        Ok(!self.closed)
    }

    fn terminate(&mut self) -> Result<(), AgentError> {
        self.closed = true;
        self.pending.clear();
        Ok(())
    }
}

pub struct HttpMcpTransportFactory {
    client: Arc<dyn McpHttpClient>,
}

impl HttpMcpTransportFactory {
    pub fn new(client: Arc<dyn McpHttpClient>) -> Self {
        Self { client }
    }
}

impl McpTransportFactory for HttpMcpTransportFactory {
    fn open(&self, manifest: &McpServerManifest) -> Result<Box<dyn McpTransport>, AgentError> {
        match manifest.transport {
            McpTransportKind::StreamableHttp | McpTransportKind::Sse => {}
            McpTransportKind::Stdio => {
                return Err(AgentError::invalid_input(
                    "HttpMcpTransportFactory requires streamable HTTP or SSE",
                ));
            }
        }
        let url = manifest
            .url
            .as_deref()
            .ok_or_else(|| AgentError::invalid_input("HTTP MCP manifest requires url"))?;
        Ok(Box::new(HttpMcpTransport::new(
            url,
            manifest.headers.clone(),
            self.client.clone(),
        )))
    }
}

pub struct CompositeMcpTransportFactory {
    stdio: StdioMcpTransportFactory,
    http: HttpMcpTransportFactory,
}

impl CompositeMcpTransportFactory {
    pub fn new(http_client: Arc<dyn McpHttpClient>) -> Self {
        Self {
            stdio: StdioMcpTransportFactory,
            http: HttpMcpTransportFactory::new(http_client),
        }
    }
}

impl McpTransportFactory for CompositeMcpTransportFactory {
    fn open(&self, manifest: &McpServerManifest) -> Result<Box<dyn McpTransport>, AgentError> {
        match manifest.transport {
            McpTransportKind::Stdio => self.stdio.open(manifest),
            McpTransportKind::StreamableHttp | McpTransportKind::Sse => self.http.open(manifest),
        }
    }
}

#[derive(Clone, Default)]
pub struct McpCancellation {
    cancelled: Arc<AtomicBool>,
}

impl McpCancellation {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
pub struct McpRequestControl {
    pub timeout: Duration,
    pub cancellation: McpCancellation,
}

impl Default for McpRequestControl {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_REQUEST_TIMEOUT,
            cancellation: McpCancellation::default(),
        }
    }
}

struct McpSession {
    manifest: McpServerManifest,
    transport: Box<dyn McpTransport>,
    next_request_id: u64,
    live_catalog: McpCatalog,
    state: McpServerState,
    restart_count: u64,
    last_error: Option<String>,
    pending: BTreeSet<u64>,
}

impl McpSession {
    fn request(
        &mut self,
        method: &str,
        params: Value,
        control: &McpRequestControl,
    ) -> Result<Value, AgentError> {
        if !self.transport.is_alive()? {
            self.state = McpServerState::Failed;
            self.last_error = Some("MCP server exited".into());
            self.fail_pending("agent.mcp.crashed", "MCP server process exited");
            return Err(AgentError::new(
                "agent.mcp.crashed",
                "MCP server process exited",
            ));
        }
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.pending.insert(id);
        self.transport.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;
        let deadline = Instant::now() + control.timeout;
        loop {
            if control.cancellation.is_cancelled() {
                let _ = self.cancel_request(id);
                self.pending.remove(&id);
                return Err(AgentError::new(
                    "agent.mcp.cancelled",
                    "MCP request was cancelled",
                ));
            }
            let now = Instant::now();
            if now >= deadline {
                let _ = self.cancel_request(id);
                self.pending.remove(&id);
                return Err(AgentError::new(
                    "agent.mcp.timeout",
                    format!("MCP request `{method}` timed out"),
                ));
            }
            let Some(message) = self
                .transport
                .receive((deadline - now).min(Duration::from_millis(25)))?
            else {
                continue;
            };
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                self.pending.remove(&id);
                if let Some(error) = message.get("error") {
                    return Err(AgentError::new(
                        "agent.mcp.request_failed",
                        error.to_string(),
                    ));
                }
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
            self.handle_server_message(message)?;
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), AgentError> {
        self.transport.send(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    fn cancel_request(&mut self, id: u64) -> Result<(), AgentError> {
        self.notify(
            "notifications/cancelled",
            json!({"requestId": id, "reason": "cancelled"}),
        )
    }

    fn fail_pending(&mut self, _code: &str, _message: &str) {
        self.pending.clear();
    }

    fn handle_server_message(&mut self, message: Value) -> Result<(), AgentError> {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return Ok(());
        };
        match method {
            "notifications/tools/list_changed"
            | "notifications/resources/list_changed"
            | "notifications/prompts/list_changed" => {
                self.refresh_catalog()?;
            }
            _ => {
                if let Some(id) = message.get("id").cloned() {
                    self.transport.send(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": Value::Null,
                    }))?;
                }
            }
        }
        Ok(())
    }

    fn refresh_catalog(&mut self) -> Result<(), AgentError> {
        let tools = self.request("tools/list", json!({}), &McpRequestControl::default())?;
        let resources = self.request("resources/list", json!({}), &McpRequestControl::default())?;
        let prompts = self.request("prompts/list", json!({}), &McpRequestControl::default())?;
        self.live_catalog = parse_catalog(
            &self.manifest.server_id,
            self.live_catalog.generation.saturating_add(1).max(1),
            tools,
            resources,
            prompts,
        )?;
        Ok(())
    }

    fn status(&mut self) -> McpServerStatus {
        if self.state == McpServerState::Ready {
            match self.transport.is_alive() {
                Ok(true) => {}
                Ok(false) => {
                    self.state = McpServerState::Failed;
                    self.last_error = Some("MCP server exited".into());
                    self.fail_pending("agent.mcp.crashed", "MCP server exited");
                }
                Err(error) => {
                    self.state = McpServerState::Failed;
                    self.last_error = Some(error.to_string());
                    self.fail_pending("agent.mcp.crashed", &error.to_string());
                }
            }
        }
        McpServerStatus {
            server_id: self.manifest.server_id.clone(),
            state: self.state.clone(),
            catalog_generation: self.live_catalog.generation,
            tool_count: self.live_catalog.tools.len(),
            resource_count: self.live_catalog.resources.len(),
            prompt_count: self.live_catalog.prompts.len(),
            restart_count: self.restart_count,
            last_error: self.last_error.clone(),
        }
    }
}

fn parse_catalog(
    server_id: &str,
    generation: u64,
    tools: Value,
    resources: Value,
    prompts: Value,
) -> Result<McpCatalog, AgentError> {
    let tool_items = tools
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut parsed_tools = Vec::new();
    for tool in tool_items {
        let name = tool
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::invalid_input("MCP tool missing name"))?
            .to_string();
        let annotations = tool
            .get("annotations")
            .cloned()
            .map(|value| serde_json::from_value::<McpToolAnnotations>(value).unwrap_or_default())
            .unwrap_or_default();
        parsed_tools.push(McpToolInfo {
            server_id: server_id.into(),
            namespaced_name: mcp_namespaced_name(server_id, &name),
            name,
            description: tool
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            input_schema: tool
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({})),
            annotations,
        });
    }

    let resource_items = resources
        .get("resources")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut parsed_resources = Vec::new();
    for resource in resource_items {
        let uri = resource
            .get("uri")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::invalid_input("MCP resource missing uri"))?
            .to_string();
        let name = resource
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(uri.as_str())
            .to_string();
        parsed_resources.push(McpResourceInfo {
            server_id: server_id.into(),
            uri,
            name,
            description: resource
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string),
            mime_type: resource
                .get("mimeType")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }

    let prompt_items = prompts
        .get("prompts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut parsed_prompts = Vec::new();
    for prompt in prompt_items {
        let name = prompt
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::invalid_input("MCP prompt missing name"))?
            .to_string();
        let arguments = prompt
            .get("arguments")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|argument| {
                let name = argument.get("name")?.as_str()?.to_string();
                Some(McpPromptArgument {
                    name,
                    description: argument
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    required: argument
                        .get("required")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
            })
            .collect();
        parsed_prompts.push(McpPromptInfo {
            server_id: server_id.into(),
            namespaced_name: mcp_namespaced_name(server_id, &name),
            name,
            description: prompt
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string),
            arguments,
        });
    }

    Ok(McpCatalog {
        generation,
        tools: parsed_tools,
        resources: parsed_resources,
        prompts: parsed_prompts,
    })
}

pub struct SharedMcpService {
    descriptor: AgentServiceDescriptor,
    factory: Arc<dyn McpTransportFactory>,
    resources: AgentResourceStore,
    sessions: Mutex<BTreeMap<String, McpSession>>,
    pinned_generation: AtomicU64,
    pinned_catalog: Mutex<McpCatalog>,
}

impl SharedMcpService {
    pub fn new(factory: Arc<dyn McpTransportFactory>) -> Self {
        Self::with_resources(factory, AgentResourceStore::default())
    }

    pub fn with_resources(
        factory: Arc<dyn McpTransportFactory>,
        resources: AgentResourceStore,
    ) -> Self {
        Self {
            descriptor: AgentServiceDescriptor {
                service_id: SERVICE_ID.into(),
                version: "1".into(),
                request_schema: "mutsuki.agent.mcp.request@1".into(),
                response_schema: "mutsuki.agent.mcp.response@1".into(),
                state: AgentPluginStateKind::Stateful,
                affinity: Some("session".into()),
            },
            factory,
            resources,
            sessions: Mutex::new(BTreeMap::new()),
            pinned_generation: AtomicU64::new(0),
            pinned_catalog: Mutex::new(McpCatalog::default()),
        }
    }

    pub fn plugin_descriptor(generation: u64) -> Result<AgentKitPluginDescriptor, AgentError> {
        let mut registrar = AgentPluginRegistrar::new(PLUGIN_ID, generation)
            .service(AgentServiceDescriptor {
                service_id: SERVICE_ID.into(),
                version: "1".into(),
                request_schema: "mutsuki.agent.mcp.request@1".into(),
                response_schema: "mutsuki.agent.mcp.response@1".into(),
                state: AgentPluginStateKind::Stateful,
                affinity: Some("session".into()),
            })
            .context_provider(CONTEXT_PROVIDER_ID)
            .require_capability("process.spawn")
            .require_capability("network.http")
            .require_service(SERVICE_ID);
        let mut tool = AgentToolDescriptor::new(
            "mcp.call",
            "mutsuki.agent.tool.mcp.call@1",
            "Call a namespaced MCP tool",
        );
        tool.side_effect = ToolSideEffect::ExternalWrite;
        tool.requires_approval = true;
        registrar = registrar.tool(tool);
        registrar.build()
    }

    pub fn connect(&self, manifest: McpServerManifest) -> Result<McpServerStatus, AgentError> {
        validate_manifest(&manifest)?;
        let mut sessions = self.sessions.lock().expect("mcp sessions mutex");
        if sessions.contains_key(&manifest.server_id) {
            return Ok(sessions
                .get_mut(&manifest.server_id)
                .expect("checked")
                .status());
        }
        let session = self.spawn_session(manifest, 0)?;
        let status = McpServerStatus {
            server_id: session.manifest.server_id.clone(),
            state: session.state.clone(),
            catalog_generation: session.live_catalog.generation,
            tool_count: session.live_catalog.tools.len(),
            resource_count: session.live_catalog.resources.len(),
            prompt_count: session.live_catalog.prompts.len(),
            restart_count: session.restart_count,
            last_error: session.last_error.clone(),
        };
        sessions.insert(session.manifest.server_id.clone(), session);
        drop(sessions);
        self.recompute_live_pin_if_unpinned();
        Ok(status)
    }

    fn spawn_session(
        &self,
        manifest: McpServerManifest,
        restart_count: u64,
    ) -> Result<McpSession, AgentError> {
        let transport = self.factory.open(&manifest)?;
        let mut session = McpSession {
            manifest: manifest.clone(),
            transport,
            next_request_id: 1,
            live_catalog: McpCatalog::default(),
            state: McpServerState::Connecting,
            restart_count,
            last_error: None,
            pending: BTreeSet::new(),
        };
        let timeout = Duration::from_millis(manifest.request_timeout_ms.unwrap_or(10_000).max(1));
        let control = McpRequestControl {
            timeout,
            cancellation: McpCancellation::default(),
        };
        session.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {
                    "roots": {"listChanged": false},
                    "sampling": {},
                },
                "clientInfo": {
                    "name": "mutsuki-agent-plugin-mcp",
                    "version": "0.1.0",
                },
            }),
            &control,
        )?;
        session.notify("notifications/initialized", json!({}))?;
        session.refresh_catalog()?;
        session.live_catalog.generation = session.live_catalog.generation.max(1);
        session.state = McpServerState::Ready;
        Ok(session)
    }

    pub fn disconnect(&self, server_id: &str) -> Result<(), AgentError> {
        let mut session = self
            .sessions
            .lock()
            .expect("mcp sessions mutex")
            .remove(server_id)
            .ok_or_else(|| AgentError::not_found("MCP server is not connected"))?;
        session.state = McpServerState::Draining;
        session.fail_pending("agent.mcp.closed", "MCP server disconnected");
        let _ = session.notify("notifications/cancelled", json!({"reason": "disconnect"}));
        session.transport.terminate()?;
        self.recompute_live_pin_if_unpinned();
        Ok(())
    }

    pub fn reload(&self, server_id: &str) -> Result<McpServerStatus, AgentError> {
        let (manifest, restart_count) = {
            let mut sessions = self.sessions.lock().expect("mcp sessions mutex");
            let mut session = sessions
                .remove(server_id)
                .ok_or_else(|| AgentError::not_found("MCP server is not connected"))?;
            session.state = McpServerState::Draining;
            session.fail_pending("agent.mcp.closed", "MCP server reloading");
            let _ = session.transport.terminate();
            (session.manifest, session.restart_count.saturating_add(1))
        };
        let session = self.spawn_session(manifest, restart_count)?;
        let status = McpServerStatus {
            server_id: session.manifest.server_id.clone(),
            state: session.state.clone(),
            catalog_generation: session.live_catalog.generation,
            tool_count: session.live_catalog.tools.len(),
            resource_count: session.live_catalog.resources.len(),
            prompt_count: session.live_catalog.prompts.len(),
            restart_count: session.restart_count,
            last_error: session.last_error.clone(),
        };
        self.sessions
            .lock()
            .expect("mcp sessions mutex")
            .insert(session.manifest.server_id.clone(), session);
        self.recompute_live_pin_if_unpinned();
        Ok(status)
    }

    pub fn status(&self, server_id: &str) -> Result<McpServerStatus, AgentError> {
        self.sessions
            .lock()
            .expect("mcp sessions mutex")
            .get_mut(server_id)
            .map(McpSession::status)
            .ok_or_else(|| AgentError::not_found("MCP server is not connected"))
    }

    pub fn list_servers(&self) -> Vec<McpServerStatus> {
        self.sessions
            .lock()
            .expect("mcp sessions mutex")
            .values_mut()
            .map(McpSession::status)
            .collect()
    }

    pub fn catalog(
        &self,
        server_id: Option<&str>,
        generation: Option<u64>,
    ) -> Result<McpCatalog, AgentError> {
        if let Some(generation) = generation {
            let pinned = self.pinned_catalog.lock().expect("mcp pin mutex");
            if self.pinned_generation.load(Ordering::Acquire) == generation {
                return Ok(filter_catalog(&pinned, server_id));
            }
        }
        let live = self.live_catalog();
        if let Some(generation) = generation
            && live.generation != generation
        {
            return Err(AgentError::new(
                "agent.plugin.generation_changed",
                "requested MCP catalog generation is not current",
            ));
        }
        Ok(filter_catalog(&live, server_id))
    }

    pub fn pin_turn(&self, generation: u64) -> Result<McpCatalog, AgentError> {
        if generation == 0 {
            return Err(AgentError::invalid_input(
                "MCP turn generation must be non-zero",
            ));
        }
        let live = self.live_catalog();
        let mut pinned = live.clone();
        pinned.generation = generation;
        *self.pinned_catalog.lock().expect("mcp pin mutex") = pinned.clone();
        self.pinned_generation.store(generation, Ordering::Release);
        Ok(pinned)
    }

    pub fn active_server_count(&self) -> usize {
        self.sessions.lock().expect("mcp sessions mutex").len()
    }

    pub fn call_tool(
        &self,
        namespaced_name: &str,
        arguments: Value,
        control: &McpRequestControl,
    ) -> Result<McpCallResult, AgentError> {
        let (server_id, tool_name) = split_namespaced(namespaced_name)?;
        let mut sessions = self.sessions.lock().expect("mcp sessions mutex");
        let session = sessions
            .get_mut(server_id)
            .ok_or_else(|| AgentError::not_found("MCP server is not connected"))?;
        if !session
            .live_catalog
            .tools
            .iter()
            .any(|tool| tool.name == tool_name)
        {
            // Allow pinned tools that still exist by namespaced name even if live moved on.
            let pinned = self.pinned_catalog.lock().expect("mcp pin mutex");
            if !pinned
                .tools
                .iter()
                .any(|tool| tool.namespaced_name == namespaced_name && tool.server_id == server_id)
            {
                return Err(AgentError::new(
                    "agent.mcp.tool_not_found",
                    format!("MCP tool `{namespaced_name}` was not found"),
                ));
            }
        }
        let result = session.request(
            "tools/call",
            json!({"name": tool_name, "arguments": arguments}),
            control,
        )?;
        let is_error = result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.pack_call_result(result, is_error)
    }

    pub fn read_resource(
        &self,
        server_id: &str,
        uri: &str,
    ) -> Result<McpResourceReadResult, AgentError> {
        if !(uri.starts_with("mcp://")
            || uri.starts_with("resource://")
            || uri.starts_with("file://")
            || uri.starts_with("https://")
            || uri.starts_with("http://"))
        {
            return Err(AgentError::new(
                "agent.mcp.resource_denied",
                "resource URI scheme is not allowed",
            ));
        }
        let mut sessions = self.sessions.lock().expect("mcp sessions mutex");
        let session = sessions
            .get_mut(server_id)
            .ok_or_else(|| AgentError::not_found("MCP server is not connected"))?;
        if !session
            .live_catalog
            .resources
            .iter()
            .any(|resource| resource.uri == uri)
        {
            return Err(AgentError::new(
                "agent.mcp.resource_denied",
                "resource URI is not declared by the MCP server",
            ));
        }
        let result = session.request(
            "resources/read",
            json!({"uri": uri}),
            &McpRequestControl::default(),
        )?;
        let details = self.resources.put_json(
            SERVICE_ID,
            "mutsuki.agent.mcp.resource",
            "mutsuki.agent.mcp.resource@1",
            1,
            &result,
        )?;
        let summary = format!("MCP resource `{uri}` from `{server_id}`");
        let item = ContextItemRef {
            item_id: format!("{server_id}:{uri}"),
            category: "mcp.resource".into(),
            summary: summary.clone(),
            details: Some(details.clone()),
            priority: ContextPriority::Normal,
            estimated_tokens: (summary.len() as u64).div_ceil(4),
            estimated_bytes: summary.len() as u64,
            provenance: ContextProvenance {
                provider_id: CONTEXT_PROVIDER_ID.into(),
                source_kind: "mcp.resource".into(),
                source_id: uri.into(),
                source_version: session.live_catalog.generation.to_string(),
            },
            required: false,
        };
        Ok(McpResourceReadResult {
            uri: uri.into(),
            summary,
            item: Some(item),
            details: Some(details),
            mime_type: session
                .live_catalog
                .resources
                .iter()
                .find(|resource| resource.uri == uri)
                .and_then(|resource| resource.mime_type.clone()),
        })
    }

    pub fn get_prompt(
        &self,
        namespaced_name: &str,
        arguments: Value,
    ) -> Result<McpPromptGetResult, AgentError> {
        let (server_id, prompt_name) = split_namespaced(namespaced_name)?;
        let mut sessions = self.sessions.lock().expect("mcp sessions mutex");
        let session = sessions
            .get_mut(server_id)
            .ok_or_else(|| AgentError::not_found("MCP server is not connected"))?;
        let prompt = session
            .live_catalog
            .prompts
            .iter()
            .find(|prompt| prompt.name == prompt_name)
            .cloned()
            .ok_or_else(|| AgentError::not_found("MCP prompt was not found"))?;
        let result = session.request(
            "prompts/get",
            json!({"name": prompt_name, "arguments": arguments}),
            &McpRequestControl::default(),
        )?;
        let messages = result.get("messages").cloned().unwrap_or_else(|| json!([]));
        let details = self.resources.put_json(
            SERVICE_ID,
            "mutsuki.agent.mcp.prompt",
            "mutsuki.agent.mcp.prompt@1",
            1,
            &messages,
        )?;
        let content = messages
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|message| {
                message
                    .pointer("/content/text")
                    .and_then(Value::as_str)
                    .or_else(|| message.get("content").and_then(Value::as_str))
                    .map(str::to_string)
            })
            .collect::<Vec<_>>()
            .join("\n");
        Ok(McpPromptGetResult {
            prompt,
            fragments: vec![AgentPromptFragment {
                fragment_id: namespaced_name.into(),
                content,
                priority: 0,
            }],
            details: Some(details),
        })
    }

    pub fn cancel(&self, server_id: &str, request_id: u64) -> Result<(), AgentError> {
        let mut sessions = self.sessions.lock().expect("mcp sessions mutex");
        let session = sessions
            .get_mut(server_id)
            .ok_or_else(|| AgentError::not_found("MCP server is not connected"))?;
        session.cancel_request(request_id)?;
        session.pending.remove(&request_id);
        Ok(())
    }

    pub fn inject_notification_for_tests(
        &self,
        server_id: &str,
        message: Value,
    ) -> Result<(), AgentError> {
        let mut sessions = self.sessions.lock().expect("mcp sessions mutex");
        let session = sessions
            .get_mut(server_id)
            .ok_or_else(|| AgentError::not_found("MCP server is not connected"))?;
        session.handle_server_message(message)
    }

    fn pack_call_result(&self, result: Value, is_error: bool) -> Result<McpCallResult, AgentError> {
        let bytes = serde_json::to_vec(&result)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let summary = format!(
            "MCP tool call completed with {} response bytes",
            bytes.len()
        );
        if bytes.len() <= INLINE_LIMIT {
            return Ok(McpCallResult {
                summary,
                inline: Some(result),
                details: None,
                is_error,
            });
        }
        let details = self.resources.put_json(
            SERVICE_ID,
            "mutsuki.agent.mcp.tool_result",
            "mutsuki.agent.mcp.tool_result@1",
            1,
            &result,
        )?;
        Ok(McpCallResult {
            summary,
            inline: None,
            details: Some(details),
            is_error,
        })
    }

    fn live_catalog(&self) -> McpCatalog {
        let sessions = self.sessions.lock().expect("mcp sessions mutex");
        let mut catalog = McpCatalog {
            generation: sessions
                .values()
                .map(|session| session.live_catalog.generation)
                .max()
                .unwrap_or(0),
            tools: Vec::new(),
            resources: Vec::new(),
            prompts: Vec::new(),
        };
        for session in sessions.values() {
            catalog.tools.extend(session.live_catalog.tools.clone());
            catalog
                .resources
                .extend(session.live_catalog.resources.clone());
            catalog.prompts.extend(session.live_catalog.prompts.clone());
        }
        catalog
    }

    fn effective_tools(&self) -> Vec<AgentToolDescriptor> {
        let pinned_generation = self.pinned_generation.load(Ordering::Acquire);
        let catalog = if pinned_generation > 0 {
            self.pinned_catalog.lock().expect("mcp pin mutex").clone()
        } else {
            self.live_catalog()
        };
        catalog
            .tools
            .into_iter()
            .map(|tool| {
                let mut descriptor = AgentToolDescriptor::new(
                    tool.namespaced_name,
                    format!("mutsuki.agent.tool.mcp.{}@1", tool.name),
                    tool.description,
                );
                descriptor.input_schema = tool.input_schema;
                descriptor.side_effect = tool.annotations.side_effect();
                descriptor.requires_approval = tool.annotations.requires_approval();
                descriptor
            })
            .collect()
    }

    fn recompute_live_pin_if_unpinned(&self) {
        if self.pinned_generation.load(Ordering::Acquire) == 0 {
            let live = self.live_catalog();
            *self.pinned_catalog.lock().expect("mcp pin mutex") = live;
        }
    }
}

fn validate_manifest(manifest: &McpServerManifest) -> Result<(), AgentError> {
    if manifest.server_id.trim().is_empty() || manifest.source.trim().is_empty() {
        return Err(AgentError::invalid_input(
            "MCP manifest requires server_id and source",
        ));
    }
    match manifest.transport {
        McpTransportKind::Stdio => {
            if manifest
                .command
                .as_ref()
                .is_none_or(|value| value.trim().is_empty())
            {
                return Err(AgentError::invalid_input(
                    "stdio MCP manifest requires command",
                ));
            }
        }
        McpTransportKind::StreamableHttp | McpTransportKind::Sse => {
            if manifest
                .url
                .as_ref()
                .is_none_or(|value| value.trim().is_empty())
            {
                return Err(AgentError::invalid_input("HTTP MCP manifest requires url"));
            }
        }
    }
    Ok(())
}

fn split_namespaced(name: &str) -> Result<(&str, &str), AgentError> {
    name.split_once('/')
        .filter(|(server, tool)| !server.is_empty() && !tool.is_empty())
        .ok_or_else(|| AgentError::invalid_input("MCP tool/prompt name must be `server_id/name`"))
}

fn filter_catalog(catalog: &McpCatalog, server_id: Option<&str>) -> McpCatalog {
    let Some(server_id) = server_id else {
        return catalog.clone();
    };
    McpCatalog {
        generation: catalog.generation,
        tools: catalog
            .tools
            .iter()
            .filter(|tool| tool.server_id == server_id)
            .cloned()
            .collect(),
        resources: catalog
            .resources
            .iter()
            .filter(|resource| resource.server_id == server_id)
            .cloned()
            .collect(),
        prompts: catalog
            .prompts
            .iter()
            .filter(|prompt| prompt.server_id == server_id)
            .cloned()
            .collect(),
    }
}

impl AgentService for SharedMcpService {
    fn descriptor(&self) -> &AgentServiceDescriptor {
        &self.descriptor
    }

    fn call(&self, request: Value) -> Result<Value, AgentError> {
        let request: McpServiceRequest = serde_json::from_value(request)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let response = match request {
            McpServiceRequest::Connect { manifest } => {
                McpServiceResponse::Status(self.connect(manifest)?)
            }
            McpServiceRequest::Disconnect { server_id } => {
                self.disconnect(&server_id)?;
                McpServiceResponse::Ack
            }
            McpServiceRequest::Reload { server_id } => {
                McpServiceResponse::Status(self.reload(&server_id)?)
            }
            McpServiceRequest::Status { server_id } => {
                McpServiceResponse::Status(self.status(&server_id)?)
            }
            McpServiceRequest::ListServers => McpServiceResponse::Servers(self.list_servers()),
            McpServiceRequest::Catalog {
                server_id,
                generation,
            } => McpServiceResponse::Catalog(self.catalog(server_id.as_deref(), generation)?),
            McpServiceRequest::PinTurn { generation } => {
                McpServiceResponse::Catalog(self.pin_turn(generation)?)
            }
            McpServiceRequest::CallTool {
                namespaced_name,
                arguments,
                timeout_ms,
            } => {
                let control = McpRequestControl {
                    timeout: Duration::from_millis(timeout_ms.unwrap_or(10_000).max(1)),
                    cancellation: McpCancellation::default(),
                };
                McpServiceResponse::Call(self.call_tool(&namespaced_name, arguments, &control)?)
            }
            McpServiceRequest::ReadResource { server_id, uri } => {
                McpServiceResponse::Resource(self.read_resource(&server_id, &uri)?)
            }
            McpServiceRequest::GetPrompt {
                namespaced_name,
                arguments,
            } => McpServiceResponse::Prompt(self.get_prompt(&namespaced_name, arguments)?),
            McpServiceRequest::Cancel {
                server_id,
                request_id,
            } => {
                self.cancel(&server_id, request_id)?;
                McpServiceResponse::Ack
            }
        };
        serde_json::to_value(response).map_err(|error| AgentError::invalid_input(error.to_string()))
    }

    fn drain(&self) -> Result<(), AgentError> {
        for session in self
            .sessions
            .lock()
            .expect("mcp sessions mutex")
            .values_mut()
        {
            session.state = McpServerState::Draining;
            session.fail_pending("agent.mcp.closed", "MCP plugin draining");
        }
        Ok(())
    }

    fn dispose(&self) -> Result<(), AgentError> {
        let server_ids = self
            .sessions
            .lock()
            .expect("mcp sessions mutex")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut first_error = None;
        for server_id in server_ids {
            if let Err(error) = self.disconnect(&server_id)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl ToolProvider for SharedMcpService {
    fn tools(&self) -> Vec<AgentToolDescriptor> {
        self.effective_tools()
    }
}

impl ContextProvider for SharedMcpService {
    fn provider_id(&self) -> &str {
        CONTEXT_PROVIDER_ID
    }

    fn collect(
        &self,
        request: ContextProviderRequest,
    ) -> Result<ContextProviderResult, AgentError> {
        let servers = self.list_servers();
        let summary = format!(
            "MCP connected servers={} tools={}",
            servers.len(),
            self.effective_tools().len()
        );
        let details = self.resources.put_json(
            SERVICE_ID,
            "mutsuki.agent.mcp.context",
            "mutsuki.agent.mcp.context@1",
            1,
            &json!({
                "servers": servers,
                "pinned_generation": self.pinned_generation.load(Ordering::Acquire),
            }),
        )?;
        Ok(ContextProviderResult {
            provider_id: request.provider_id,
            summary,
            details: Some(details),
            estimated_tokens: 32,
            estimated_bytes: 128,
            priority: 0,
            required: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet, VecDeque};
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use mutsuki_agent_contracts::{McpTransportKind, mcp_namespaced_name};
    use mutsuki_agent_plugin_api::AgentPluginGenerationSet;
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    struct MockFactory {
        responses: BTreeMap<String, Value>,
        stalled: BTreeSet<String>,
        alive: Arc<AtomicBool>,
        sent: Arc<Mutex<Vec<Value>>>,
        list_tools_extra: Arc<Mutex<Option<Value>>>,
    }

    impl MockFactory {
        fn new() -> Self {
            Self {
                responses: BTreeMap::new(),
                stalled: BTreeSet::new(),
                alive: Arc::new(AtomicBool::new(true)),
                sent: Arc::new(Mutex::new(Vec::new())),
                list_tools_extra: Arc::new(Mutex::new(None)),
            }
        }
    }

    impl McpTransportFactory for MockFactory {
        fn open(&self, _manifest: &McpServerManifest) -> Result<Box<dyn McpTransport>, AgentError> {
            Ok(Box::new(MockTransport {
                responses: self.responses.clone(),
                stalled: self.stalled.clone(),
                alive: self.alive.clone(),
                sent: self.sent.clone(),
                list_tools_extra: self.list_tools_extra.clone(),
                pending: VecDeque::new(),
                tools_list_count: 0,
            }))
        }
    }

    struct MockTransport {
        responses: BTreeMap<String, Value>,
        stalled: BTreeSet<String>,
        alive: Arc<AtomicBool>,
        sent: Arc<Mutex<Vec<Value>>>,
        list_tools_extra: Arc<Mutex<Option<Value>>>,
        pending: VecDeque<Value>,
        tools_list_count: u32,
    }

    impl McpTransport for MockTransport {
        fn send(&mut self, value: &Value) -> Result<(), AgentError> {
            self.sent.lock().unwrap().push(value.clone());
            let Some(id) = value.get("id").and_then(Value::as_u64) else {
                return Ok(());
            };
            let method = value
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if self.stalled.contains(method) {
                return Ok(());
            }
            let result = if method == "tools/list" {
                self.tools_list_count += 1;
                if self.tools_list_count > 1 {
                    if let Some(extra) = self.list_tools_extra.lock().unwrap().clone() {
                        extra
                    } else {
                        self.responses
                            .get(method)
                            .cloned()
                            .unwrap_or_else(|| json!({"tools": []}))
                    }
                } else {
                    self.responses
                        .get(method)
                        .cloned()
                        .unwrap_or_else(|| json!({"tools": []}))
                }
            } else {
                self.responses.get(method).cloned().unwrap_or(Value::Null)
            };
            self.pending.push_back(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result,
            }));
            Ok(())
        }

        fn receive(&mut self, timeout: Duration) -> Result<Option<Value>, AgentError> {
            if let Some(value) = self.pending.pop_front() {
                return Ok(Some(value));
            }
            thread::sleep(timeout.min(Duration::from_millis(1)));
            Ok(None)
        }

        fn is_alive(&mut self) -> Result<bool, AgentError> {
            Ok(self.alive.load(Ordering::Acquire))
        }

        fn terminate(&mut self) -> Result<(), AgentError> {
            Ok(())
        }
    }

    fn manifest(server_id: &str) -> McpServerManifest {
        McpServerManifest {
            server_id: server_id.into(),
            source: "test".into(),
            transport: McpTransportKind::Stdio,
            command: Some("unused".into()),
            args: Vec::new(),
            env_allowlist: Vec::new(),
            url: None,
            headers: Vec::new(),
            permissions: vec!["tools".into()],
            request_timeout_ms: Some(1_000),
        }
    }

    fn echo_tool(_server_id: &str, destructive: bool) -> Value {
        json!({
            "name": "echo",
            "description": "echo",
            "inputSchema": {"type": "object"},
            "annotations": {
                "readOnlyHint": !destructive,
                "destructiveHint": destructive,
                "openWorldHint": false,
            }
        })
    }

    #[test]
    fn mock_stdio_initialize_list_and_call_uses_resource_refs_and_namespaces() {
        let mut factory = MockFactory::new();
        factory.responses.insert(
            "initialize".into(),
            json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {"tools": {}}}),
        );
        factory.responses.insert(
            "tools/list".into(),
            json!({"tools": [echo_tool("ignored", true)]}),
        );
        factory
            .responses
            .insert("resources/list".into(), json!({"resources": []}));
        factory
            .responses
            .insert("prompts/list".into(), json!({"prompts": []}));
        factory.responses.insert(
            "tools/call".into(),
            json!({"content": [{"type": "text", "text": "x".repeat(70_000)}]}),
        );
        let service = SharedMcpService::new(Arc::new(factory));
        service.connect(manifest("alpha")).unwrap();
        service.connect(manifest("beta")).unwrap();
        let tools = service.tools();
        assert!(tools.iter().any(|tool| tool.name == "alpha/echo"));
        assert!(tools.iter().any(|tool| tool.name == "beta/echo"));
        assert_eq!(
            tools
                .iter()
                .find(|tool| tool.name == "alpha/echo")
                .unwrap()
                .side_effect,
            ToolSideEffect::ExternalWrite
        );
        assert!(
            tools
                .iter()
                .find(|tool| tool.name == "alpha/echo")
                .unwrap()
                .requires_approval
        );
        let result = service
            .call_tool(
                "alpha/echo",
                json!({"text": "hi"}),
                &McpRequestControl::default(),
            )
            .unwrap();
        assert!(result.inline.is_none());
        assert!(result.details.is_some());
        let descriptor = SharedMcpService::plugin_descriptor(3).unwrap();
        assert_eq!(descriptor.plugin_id, PLUGIN_ID);
        assert!(
            descriptor
                .required_capabilities
                .iter()
                .any(|value| value == "process.spawn")
        );
        service.dispose().unwrap();
        assert_eq!(service.active_server_count(), 0);
    }

    #[test]
    fn list_changed_keeps_pinned_turn_stable_and_exposes_new_catalog_next_turn() {
        let mut factory = MockFactory::new();
        factory.responses.insert(
            "initialize".into(),
            json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {}}),
        );
        factory.responses.insert(
            "tools/list".into(),
            json!({"tools": [echo_tool("alpha", false)]}),
        );
        factory
            .responses
            .insert("resources/list".into(), json!({"resources": []}));
        factory
            .responses
            .insert("prompts/list".into(), json!({"prompts": []}));
        *factory.list_tools_extra.lock().unwrap() = Some(json!({
            "tools": [
                echo_tool("alpha", false),
                {
                    "name": "ping",
                    "description": "ping",
                    "inputSchema": {"type": "object"},
                    "annotations": {"readOnlyHint": true}
                }
            ]
        }));
        let service = SharedMcpService::new(Arc::new(factory));
        service.connect(manifest("alpha")).unwrap();
        let pinned = service.pin_turn(1).unwrap();
        assert_eq!(pinned.tools.len(), 1);
        assert_eq!(service.tools().len(), 1);
        service
            .inject_notification_for_tests(
                "alpha",
                json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}),
            )
            .unwrap();
        assert_eq!(service.tools().len(), 1);
        assert_eq!(service.catalog(None, Some(1)).unwrap().tools.len(), 1);
        let next = service.pin_turn(2).unwrap();
        assert!(next.tools.iter().any(|tool| tool.name == "ping"));
        assert_eq!(service.tools().len(), 2);
        let original = SharedMcpService::plugin_descriptor(1).unwrap();
        let reloaded = SharedMcpService::plugin_descriptor(2).unwrap();
        let set = AgentPluginGenerationSet::pin([&original]).unwrap();
        assert_eq!(
            set.validate(&reloaded).unwrap_err().code,
            "agent.plugin.generation_changed"
        );
    }

    #[test]
    fn timeout_cancel_crash_and_reload_are_deterministic() {
        let mut factory = MockFactory::new();
        factory.responses.insert(
            "initialize".into(),
            json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {}}),
        );
        factory.responses.insert(
            "tools/list".into(),
            json!({"tools": [echo_tool("alpha", false)]}),
        );
        factory
            .responses
            .insert("resources/list".into(), json!({"resources": []}));
        factory
            .responses
            .insert("prompts/list".into(), json!({"prompts": []}));
        factory.stalled.insert("tools/call".into());
        let alive = factory.alive.clone();
        let sent = factory.sent.clone();
        let service = SharedMcpService::new(Arc::new(factory));
        service.connect(manifest("alpha")).unwrap();
        let timeout = service
            .call_tool(
                "alpha/echo",
                json!({}),
                &McpRequestControl {
                    timeout: Duration::from_millis(20),
                    cancellation: McpCancellation::default(),
                },
            )
            .unwrap_err();
        assert_eq!(timeout.code, "agent.mcp.timeout");
        assert!(sent.lock().unwrap().iter().any(|message| {
            message.get("method").and_then(Value::as_str) == Some("notifications/cancelled")
        }));

        let cancellation = McpCancellation::default();
        cancellation.cancel();
        let cancelled = service
            .call_tool(
                "alpha/echo",
                json!({}),
                &McpRequestControl {
                    timeout: Duration::from_secs(1),
                    cancellation,
                },
            )
            .unwrap_err();
        assert_eq!(cancelled.code, "agent.mcp.cancelled");

        alive.store(false, Ordering::Release);
        assert_eq!(
            service.status("alpha").unwrap().state,
            McpServerState::Failed
        );
        alive.store(true, Ordering::Release);
        let reloaded = service.reload("alpha").unwrap();
        assert_eq!(reloaded.restart_count, 1);
        assert_eq!(reloaded.state, McpServerState::Ready);
        service.dispose().unwrap();
        assert_eq!(service.active_server_count(), 0);
    }

    #[test]
    fn stdio_python_mcp_server_e2e() {
        let dir = tempdir().unwrap();
        let script = dir.path().join("mcp_server.py");
        fs::write(
            &script,
            r#"
import json, sys

def read_message():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        key, value = line.decode().split(":", 1)
        headers[key.strip().lower()] = value.strip()
    length = int(headers["content-length"])
    body = sys.stdin.buffer.read(length)
    return json.loads(body)

def write_message(message):
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode())
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    message = read_message()
    if message is None:
        break
    method = message.get("method")
    id = message.get("id")
    if method == "initialize":
        write_message({"jsonrpc": "2.0", "id": id, "result": {
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "fixture", "version": "0.1.0"},
        }})
    elif method == "notifications/initialized":
        pass
    elif method == "tools/list":
        write_message({"jsonrpc": "2.0", "id": id, "result": {"tools": [{
            "name": "echo",
            "description": "echo text",
            "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}},
            "annotations": {"readOnlyHint": True},
        }]}})
    elif method == "resources/list":
        write_message({"jsonrpc": "2.0", "id": id, "result": {"resources": [{
            "uri": "mcp://fixture/note",
            "name": "note",
            "mimeType": "text/plain",
        }]}})
    elif method == "prompts/list":
        write_message({"jsonrpc": "2.0", "id": id, "result": {"prompts": [{
            "name": "greet",
            "description": "greet",
            "arguments": [],
        }]}})
    elif method == "tools/call":
        text = message["params"].get("arguments", {}).get("text", "")
        write_message({"jsonrpc": "2.0", "id": id, "result": {
            "content": [{"type": "text", "text": text}],
            "isError": False,
        }})
    elif method == "resources/read":
        write_message({"jsonrpc": "2.0", "id": id, "result": {
            "contents": [{"uri": "mcp://fixture/note", "text": "hello-resource"}],
        }})
    elif method == "prompts/get":
        write_message({"jsonrpc": "2.0", "id": id, "result": {
            "messages": [{"role": "user", "content": {"type": "text", "text": "hi"}}],
        }})
    elif id is not None:
        write_message({"jsonrpc": "2.0", "id": id, "result": {}})
"#,
        )
        .unwrap();
        let service = SharedMcpService::new(Arc::new(StdioMcpTransportFactory));
        service
            .connect(McpServerManifest {
                server_id: "fixture".into(),
                source: "local-test".into(),
                transport: McpTransportKind::Stdio,
                command: Some("python3".into()),
                args: vec![script.to_string_lossy().into_owned()],
                env_allowlist: Vec::new(),
                url: None,
                headers: Vec::new(),
                permissions: vec!["tools".into(), "resources".into()],
                request_timeout_ms: Some(5_000),
            })
            .unwrap();
        assert!(
            service
                .tools()
                .iter()
                .any(|tool| tool.name == mcp_namespaced_name("fixture", "echo"))
        );
        let called = service
            .call_tool(
                "fixture/echo",
                json!({"text": "pong"}),
                &McpRequestControl::default(),
            )
            .unwrap();
        assert!(called.inline.as_ref().unwrap().to_string().contains("pong"));
        let resource = service
            .read_resource("fixture", "mcp://fixture/note")
            .unwrap();
        assert!(resource.item.is_some());
        let prompt = service.get_prompt("fixture/greet", json!({})).unwrap();
        assert_eq!(prompt.fragments[0].content, "hi");
        service.dispose().unwrap();
    }

    #[test]
    fn streamable_http_mcp_server_e2e() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let counter = Arc::new(AtomicU64::new(0));
        let counter_clone = counter.clone();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline && !stop_flag.load(Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(_) => break,
                };
                stream.set_nonblocking(false).ok();
                let mut buffer = Vec::new();
                let mut chunk = [0_u8; 4096];
                let (headers, body) = loop {
                    let read = stream.read(&mut chunk).unwrap_or(0);
                    if read == 0 {
                        break (String::new(), Vec::new());
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                    if let Some(idx) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&buffer[..idx]).into_owned();
                        let mut body = buffer[idx + 4..].to_vec();
                        let content_length = headers
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                if key.eq_ignore_ascii_case("content-length") {
                                    value.trim().parse::<usize>().ok()
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(0);
                        while body.len() < content_length {
                            let read = stream.read(&mut chunk).unwrap_or(0);
                            if read == 0 {
                                break;
                            }
                            body.extend_from_slice(&chunk[..read]);
                        }
                        body.truncate(content_length);
                        break (headers, body);
                    }
                };
                let _ = headers;
                let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                let method = payload.get("method").and_then(Value::as_str).unwrap_or("");
                let id = payload.get("id").cloned().unwrap_or(Value::Null);
                counter_clone.fetch_add(1, Ordering::SeqCst);
                let result = match method {
                    "initialize" => json!({
                        "protocolVersion": PROTOCOL_VERSION,
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "http-fixture", "version": "1"},
                    }),
                    "tools/list" => json!({"tools": [echo_tool("http", false)]}),
                    "resources/list" => json!({"resources": []}),
                    "prompts/list" => json!({"prompts": []}),
                    "tools/call" => json!({
                        "content": [{"type": "text", "text": "http-ok"}],
                        "isError": false,
                    }),
                    _ => Value::Null,
                };
                let response = if id.is_null() {
                    "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_string()
                } else {
                    let message = json!({"jsonrpc": "2.0", "id": id, "result": result});
                    let encoded = message.to_string();
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        encoded.len(),
                        encoded
                    )
                };
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let service = SharedMcpService::new(Arc::new(HttpMcpTransportFactory::new(Arc::new(
            ReqwestMcpHttpClient,
        ))));
        service
            .connect(McpServerManifest {
                server_id: "http".into(),
                source: "local-http-test".into(),
                transport: McpTransportKind::StreamableHttp,
                command: None,
                args: Vec::new(),
                env_allowlist: Vec::new(),
                url: Some(format!("http://{address}/mcp")),
                headers: Vec::new(),
                permissions: vec!["tools".into()],
                request_timeout_ms: Some(5_000),
            })
            .unwrap();
        let result = service
            .call_tool("http/echo", json!({}), &McpRequestControl::default())
            .unwrap();
        assert!(
            result
                .inline
                .as_ref()
                .unwrap()
                .to_string()
                .contains("http-ok")
        );
        service.dispose().unwrap();
        stop.store(true, Ordering::Release);
        let _ = server.join();
    }
}
