use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::env;
use std::fs;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use mutsuki_runtime_contracts::{
    ERR_OPERATION_NOT_FOUND, ERR_RUNTIME_BACKEND_FAILED, Envelope, OperationHandlerKey,
    OperationSnapshot, OperationStatus, RuntimeError, ScalarValue, SourceDescriptor,
    SourceSnapshot, StrategyResult, StrategyResultStatus,
};
use mutsuki_runtime_core::{
    BackendPayload, OperationBackend, RuntimeFailure, RuntimeResult, StrategyBackend,
};
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "mutsuki-codex-core";
pub const DEFAULT_SOURCE_ID: &str = "codex:local";
pub const DEFAULT_SOURCE_KIND: &str = "codex.strategy";
pub const MIN_CODEX_APP_SERVER_VERSION: (u32, u32, u32) = (0, 128, 0);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexAppServerProbeStatus {
    pub path: Option<String>,
    pub version: Option<String>,
    pub available: bool,
    pub supports_required_protocol: bool,
    pub failure_kind: Option<String>,
    pub issues: Vec<String>,
}

impl CodexAppServerProbeStatus {
    pub fn ok(path: String, version: String) -> Self {
        Self {
            path: Some(path),
            version: Some(version),
            available: true,
            supports_required_protocol: true,
            failure_kind: None,
            issues: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub struct CodexAppServerProcess {
    child: Child,
}

impl Drop for CodexAppServerProcess {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

pub type ProcessCodexAppServerClient =
    CodexAppServerClient<std::io::BufReader<ChildStdout>, std::io::BufWriter<ChildStdin>>;

impl CodexAppServerProcess {
    pub fn spawn(binary: &str) -> RuntimeResult<(Self, ProcessCodexAppServerClient)> {
        let mut command = spawn_codex_app_server_command(binary);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| runtime_failure("codex.app_server.spawn", "exception_repr", err))?;
        let stdout = child.stdout.take().ok_or_else(|| {
            runtime_failure_with_evidence("codex.app_server.spawn", "reason", "missing stdout")
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            runtime_failure_with_evidence("codex.app_server.spawn", "reason", "missing stdin")
        })?;
        Ok((
            Self { child },
            CodexAppServerClient::new(
                std::io::BufReader::new(stdout),
                std::io::BufWriter::new(stdin),
            ),
        ))
    }
}

#[derive(Debug)]
pub struct CodexAppServerClient<R, W> {
    inner: RefCell<AppServerTransport<R, W>>,
}

#[derive(Debug)]
struct AppServerTransport<R, W> {
    reader: R,
    writer: W,
    next_request: u64,
    notifications: VecDeque<Value>,
}

impl<R, W> CodexAppServerClient<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            inner: RefCell::new(AppServerTransport {
                reader,
                writer,
                next_request: 0,
                notifications: VecDeque::new(),
            }),
        }
    }

    pub fn into_inner(self) -> (R, W, Vec<Value>) {
        let inner = self.inner.into_inner();
        (
            inner.reader,
            inner.writer,
            inner.notifications.into_iter().collect(),
        )
    }
}

impl<R: BufRead, W: Write> CodexAppServerClient<R, W> {
    pub fn initialized(&self) -> RuntimeResult<()> {
        self.notify("initialized", json!({}))
    }

    pub fn notify(&self, method: &str, params: Value) -> RuntimeResult<()> {
        self.write_message(json!({"method": method, "params": params}))
    }

    pub fn respond(&self, id: Value, result: Value) -> RuntimeResult<()> {
        self.write_message(json!({"id": id, "result": result}))
    }

    pub fn request(&self, method: &str, params: Value) -> RuntimeResult<Value> {
        let id = {
            let mut inner = self.inner.borrow_mut();
            inner.next_request += 1;
            inner.next_request
        };
        self.write_message(json!({"method": method, "id": id, "params": params}))?;

        loop {
            let msg = self.read_message()?;
            if msg.get("id") == Some(&json!(id)) {
                if let Some(error) = msg.get("error") {
                    return Err(app_server_error_failure(error));
                }
                return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
            }
            if msg.get("id").is_some() && msg.get("method").is_none() {
                return Err(runtime_failure_with_evidence(
                    "codex.app_server.protocol",
                    "reason",
                    "response id mismatch",
                ));
            }
            self.inner.borrow_mut().notifications.push_back(msg);
        }
    }

    pub fn drain_notifications(&self) -> Vec<Value> {
        self.inner
            .borrow_mut()
            .notifications
            .drain(..)
            .collect::<Vec<_>>()
    }

    pub fn read_notification(&self) -> RuntimeResult<Value> {
        if let Some(msg) = self.inner.borrow_mut().notifications.pop_front() {
            return Ok(msg);
        }
        let msg = self.read_message()?;
        if msg.get("id").is_some() && msg.get("method").is_none() {
            return Err(runtime_failure_with_evidence(
                "codex.app_server.protocol",
                "reason",
                "expected notification",
            ));
        }
        Ok(msg)
    }

    fn write_message(&self, msg: Value) -> RuntimeResult<()> {
        let mut inner = self.inner.borrow_mut();
        serde_json::to_writer(&mut inner.writer, &msg)
            .map_err(|err| runtime_failure("codex.app_server.protocol", "exception_repr", err))?;
        inner
            .writer
            .write_all(b"\n")
            .map_err(|err| runtime_failure("codex.app_server.protocol", "exception_repr", err))?;
        inner
            .writer
            .flush()
            .map_err(|err| runtime_failure("codex.app_server.protocol", "exception_repr", err))?;
        Ok(())
    }

    fn read_message(&self) -> RuntimeResult<Value> {
        let mut line = String::new();
        self.inner
            .borrow_mut()
            .reader
            .read_line(&mut line)
            .map_err(|err| runtime_failure("codex.app_server.protocol", "exception_repr", err))?;
        if line.trim().is_empty() {
            return Err(runtime_failure_with_evidence(
                "codex.app_server.protocol",
                "reason",
                "empty response",
            ));
        }
        serde_json::from_str(&line)
            .map_err(|err| runtime_failure("codex.app_server.protocol", "exception_repr", err))
    }
}

#[derive(Debug)]
pub struct CodexAppServerBackend<R, W> {
    client: CodexAppServerClient<R, W>,
    source_snapshot: SourceSnapshot,
    sessions: BTreeMap<String, CodexAgentSession>,
}

#[derive(Clone, Debug, Default)]
struct CodexAgentSession {
    thread_id: Option<String>,
    pending_prompt: Option<String>,
    cwd: Option<String>,
    settings: Value,
}

impl<R, W> CodexAppServerBackend<R, W> {
    pub fn new(client: CodexAppServerClient<R, W>) -> Self {
        Self {
            client,
            source_snapshot: codex_source_snapshot(DEFAULT_SOURCE_ID, DEFAULT_SOURCE_KIND),
            sessions: BTreeMap::new(),
        }
    }

    pub fn with_source(mut self, source_id: &str, kind: &str) -> Self {
        self.source_snapshot = codex_source_snapshot(source_id, kind);
        self
    }
}

impl<R: BufRead, W: Write> StrategyBackend for CodexAppServerBackend<R, W> {
    fn on_awake(&mut self, agent_id: &str) -> RuntimeResult<()> {
        self.client.initialized()?;
        self.sessions.entry(agent_id.to_string()).or_default();
        Ok(())
    }

    fn on_input(&mut self, agent_id: &str, envelope: &Envelope) -> RuntimeResult<StrategyResult> {
        let prompt = prompt_from_envelope(envelope)?;
        let session = self.sessions.entry(agent_id.to_string()).or_default();
        session.pending_prompt = Some(prompt);
        session.cwd = envelope
            .payload
            .get("cwd")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        session.settings = envelope
            .payload
            .get("settings")
            .cloned()
            .unwrap_or_else(|| json!({}));
        self.next_step(agent_id)
    }

    fn next_step(&mut self, agent_id: &str) -> RuntimeResult<StrategyResult> {
        let session = self.sessions.entry(agent_id.to_string()).or_default();
        let prompt = session.pending_prompt.take().unwrap_or_default();
        if session.thread_id.is_none() {
            let mut params = json!({});
            assign_optional_turn_params(&mut params, session);
            let started = self.client.request("thread/new", params)?;
            session.thread_id = extract_thread_id(&started).or_else(|| Some(agent_id.to_string()));
        }
        let thread_id = session.thread_id.clone().ok_or_else(|| {
            runtime_failure_with_evidence(
                "codex.app_server.protocol",
                "reason",
                "missing thread id",
            )
        })?;
        let mut params = json!({
            "threadId": thread_id,
            "input": [{"type": "text", "text": prompt}],
        });
        assign_optional_turn_params(&mut params, session);
        self.client.request("turn/start", params)?;
        drain_turn_result(&self.client)
    }

    fn on_stop(&mut self, agent_id: &str) -> RuntimeResult<()> {
        self.sessions.remove(agent_id);
        Ok(())
    }
}

impl<R: BufRead, W: Write> OperationBackend for CodexAppServerBackend<R, W> {
    fn list_operations(&self, _agent_id: &str) -> RuntimeResult<Vec<OperationSnapshot>> {
        Ok(Vec::new())
    }

    fn list_sources(&self, _agent_id: &str) -> RuntimeResult<Vec<SourceSnapshot>> {
        Ok(vec![self.source_snapshot.clone()])
    }

    fn invoke(
        &mut self,
        _agent_id: &str,
        _key: &OperationHandlerKey,
        _payload: Value,
    ) -> RuntimeResult<BackendPayload> {
        Err(RuntimeFailure::new(RuntimeError::new(
            ERR_OPERATION_NOT_FOUND,
            PLUGIN_ID,
            "codex.operation.invoke",
        )))
    }

    fn operation_status(&self, _agent_id: &str, _key: &OperationHandlerKey) -> OperationStatus {
        OperationStatus::NotFound
    }
}

pub fn codex_source_snapshot(source_id: &str, kind: &str) -> SourceSnapshot {
    SourceSnapshot {
        descriptor: SourceDescriptor {
            source_id: source_id.into(),
            kind: kind.into(),
            capabilities: vec!["strategy".into()],
            description: "Local Codex app-server strategy backend source".into(),
        },
        plugin_id: PLUGIN_ID.into(),
        plugin_generation: 0,
    }
}

pub fn parse_codex_cli_version(output: &str) -> Option<(u32, u32, u32)> {
    let version = output
        .split_whitespace()
        .find(|part| part.chars().next().is_some_and(|ch| ch.is_ascii_digit()))?;
    let mut parts = version.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next().unwrap_or("0").parse::<u32>().ok()?;
    let patch_text = parts.next().unwrap_or("0");
    let patch_digits = patch_text
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    let patch = patch_digits.parse::<u32>().ok()?;
    Some((major, minor, patch))
}

pub fn codex_version_at_least(version: (u32, u32, u32), minimum: (u32, u32, u32)) -> bool {
    version >= minimum
}

pub fn build_codex_app_server_probe_status_with<F>(
    candidates: &[String],
    mut command_output: F,
) -> CodexAppServerProbeStatus
where
    F: FnMut(&str, &[&str]) -> Result<String, String>,
{
    let mut app_server_unavailable = None;
    let mut version_unsupported = None;

    for candidate in candidates {
        let Ok(version) = command_output(candidate, &["--version"]) else {
            continue;
        };
        let Ok(help) = command_output(candidate, &["app-server", "--help"]) else {
            app_server_unavailable
                .get_or_insert_with(|| app_server_unavailable_status(candidate, version));
            continue;
        };
        if !codex_app_server_help_available(&help) {
            app_server_unavailable
                .get_or_insert_with(|| app_server_unavailable_status(candidate, version));
            continue;
        }
        let Some(parsed_version) = parse_codex_cli_version(&version) else {
            version_unsupported.get_or_insert_with(|| CodexAppServerProbeStatus {
                path: Some(candidate.clone()),
                version: Some(version),
                available: true,
                supports_required_protocol: false,
                failure_kind: Some("experimentalApiUnsupported".into()),
                issues: vec!["unable to parse codex CLI version".into()],
            });
            continue;
        };
        if codex_version_at_least(parsed_version, MIN_CODEX_APP_SERVER_VERSION) {
            return CodexAppServerProbeStatus::ok(candidate.clone(), version);
        }
        version_unsupported.get_or_insert_with(|| CodexAppServerProbeStatus {
            path: Some(candidate.clone()),
            version: Some(version),
            available: true,
            supports_required_protocol: false,
            failure_kind: Some("experimentalApiUnsupported".into()),
            issues: vec!["codex CLI version is too old for app-server protocol".into()],
        });
    }

    version_unsupported
        .or(app_server_unavailable)
        .unwrap_or_else(|| CodexAppServerProbeStatus {
            path: None,
            version: None,
            available: false,
            supports_required_protocol: false,
            failure_kind: Some("missingCli".into()),
            issues: vec!["codex CLI was not found".into()],
        })
}

pub fn codex_cli_candidate_paths() -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for path in path_codex_candidates()
        .into_iter()
        .chain(where_codex_candidates())
        .chain(windows_native_codex_paths())
        .chain(
            codex_candidate_filenames()
                .iter()
                .map(|item| (*item).to_string()),
        )
    {
        let key = if cfg!(windows) {
            path.to_ascii_lowercase()
        } else {
            path.clone()
        };
        if !path.trim().is_empty() && seen.insert(key) {
            out.push(path);
        }
    }
    out
}

pub fn build_codex_app_server_probe_status() -> CodexAppServerProbeStatus {
    build_codex_app_server_probe_status_with(&codex_cli_candidate_paths(), command_output_result)
}

pub fn spawn_codex_app_server_command(binary: &str) -> Command {
    if is_windows_command_script(binary) {
        let mut command = Command::new(env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into()));
        command.args([
            "/d",
            "/s",
            "/c",
            &windows_command_line(binary, &["app-server"]),
        ]);
        command
    } else {
        let mut command = Command::new(binary);
        command.arg("app-server");
        command
    }
}

pub fn is_windows_command_script(binary: &str) -> bool {
    cfg!(windows)
        && (binary.to_ascii_lowercase().ends_with(".cmd")
            || binary.to_ascii_lowercase().ends_with(".bat"))
}

pub fn windows_command_line(binary: &str, args: &[&str]) -> String {
    std::iter::once(binary)
        .chain(args.iter().copied())
        .map(windows_command_line_token)
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn windows_command_line_token(value: &str) -> String {
    if !value
        .chars()
        .any(|ch| matches!(ch, ' ' | '\t' | '"' | '&' | '|' | '<' | '>' | '^'))
    {
        return value.to_string();
    }
    format!("\"{}\"", value.replace('"', "\\\""))
}

fn drain_turn_result<R: BufRead, W: Write>(
    client: &CodexAppServerClient<R, W>,
) -> RuntimeResult<StrategyResult> {
    loop {
        let msg = client.read_notification()?;
        match msg.get("method").and_then(Value::as_str) {
            Some("turn/completed") => return Ok(StrategyResult::wait_input()),
            Some("turn/failed") => {
                return Ok(StrategyResult {
                    status: StrategyResultStatus::Failed,
                    decision: None,
                    emitted: Vec::new(),
                    error: Some(runtime_error_from_turn_failure(&msg)),
                });
            }
            Some(method)
                if method.contains("approval")
                    || method.contains("elicitation")
                    || method.contains("request_user_input")
                    || method.contains("ask") =>
            {
                return Ok(StrategyResult {
                    status: StrategyResultStatus::WaitInput,
                    decision: Some(json!({
                        "kind": "codex_app_server_request",
                        "method": method,
                        "payload": msg.get("params").cloned().unwrap_or(Value::Null),
                    })),
                    emitted: Vec::new(),
                    error: None,
                });
            }
            Some(_) | None => {}
        }
    }
}

fn runtime_error_from_turn_failure(msg: &Value) -> RuntimeError {
    let mut err = RuntimeError::new(ERR_RUNTIME_BACKEND_FAILED, PLUGIN_ID, "codex.turn.failed");
    if let Some(params) = msg.get("params") {
        err.evidence
            .insert("params".into(), ScalarValue::String(params.to_string()));
    }
    if let Some(message) = msg
        .pointer("/params/error/message")
        .or_else(|| msg.pointer("/params/message"))
        .and_then(Value::as_str)
    {
        err.evidence
            .insert("message".into(), ScalarValue::String(message.into()));
    }
    err
}

fn app_server_error_failure(error: &Value) -> RuntimeFailure {
    let mut err = RuntimeError::new(
        ERR_RUNTIME_BACKEND_FAILED,
        PLUGIN_ID,
        "codex.app_server.protocol",
    );
    if let Some(message) = error.get("message").and_then(Value::as_str) {
        err.evidence
            .insert("message".into(), ScalarValue::String(message.into()));
    }
    err.evidence
        .insert("error".into(), ScalarValue::String(error.to_string()));
    RuntimeFailure::new(err)
}

fn prompt_from_envelope(envelope: &Envelope) -> RuntimeResult<String> {
    envelope
        .payload
        .get("prompt")
        .or_else(|| envelope.payload.get("text"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| runtime_failure_with_evidence("codex.input", "reason", "missing prompt"))
}

fn assign_optional_turn_params(params: &mut Value, session: &CodexAgentSession) {
    if let Some(cwd) = &session.cwd {
        params["cwd"] = json!(cwd);
    }
    if let Some(model) = session.settings.get("model").and_then(Value::as_str) {
        params["model"] = json!(model);
    }
    if let Some(reasoning_effort) = session
        .settings
        .get("reasoningEffort")
        .and_then(Value::as_str)
    {
        params["reasoningEffort"] = json!(reasoning_effort);
        params["effort"] = json!(reasoning_effort);
    }
    if let Some(permissions) = session.settings.get("permissions") {
        params["permissions"] = permissions.clone();
    }
}

fn extract_thread_id(value: &Value) -> Option<String> {
    value
        .get("threadId")
        .or_else(|| value.pointer("/thread/id"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn codex_app_server_help_available(help: &str) -> bool {
    help.contains("codex app-server") || help.contains("Usage:")
}

fn app_server_unavailable_status(candidate: &str, version: String) -> CodexAppServerProbeStatus {
    CodexAppServerProbeStatus {
        path: Some(candidate.to_string()),
        version: Some(version),
        available: false,
        supports_required_protocol: false,
        failure_kind: Some("appServerUnavailable".into()),
        issues: vec!["codex CLI does not support app-server".into()],
    }
}

fn command_output_result(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(stderr);
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        Ok(String::from_utf8_lossy(&output.stderr).trim().to_string())
    } else {
        Ok(stdout)
    }
}

fn codex_candidate_filenames() -> &'static [&'static str] {
    if cfg!(windows) {
        &["codex.cmd", "codex.exe", "codex.bat", "codex"]
    } else {
        &["codex"]
    }
}

fn path_codex_candidates() -> Vec<String> {
    let Some(path_var) = env::var_os("PATH") else {
        return Vec::new();
    };
    env::split_paths(&path_var)
        .flat_map(|dir| {
            codex_candidate_filenames()
                .iter()
                .map(move |filename| dir.join(filename))
        })
        .filter(|path| path.exists())
        .map(|path| path.to_string_lossy().to_string())
        .collect()
}

fn where_codex_candidates() -> Vec<String> {
    let output = Command::new(if cfg!(windows) { "where.exe" } else { "which" })
        .arg("codex")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn windows_native_codex_paths() -> Vec<String> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let Some(local_app_data) = env::var_os("LOCALAPPDATA") else {
        return Vec::new();
    };
    let bin = PathBuf::from(local_app_data)
        .join("OpenAI")
        .join("Codex")
        .join("bin");
    let mut paths = Vec::new();
    let root = bin.join("codex.exe");
    if root.exists() {
        paths.push(root.to_string_lossy().to_string());
    }
    if let Ok(entries) = fs::read_dir(&bin) {
        for entry in entries.flatten() {
            let path = entry.path().join("codex.exe");
            if path.exists() {
                paths.push(path.to_string_lossy().to_string());
            }
        }
    }
    paths
}

fn runtime_failure(
    route: &str,
    evidence_key: &str,
    evidence_value: impl ToString,
) -> RuntimeFailure {
    runtime_failure_with_evidence(route, evidence_key, evidence_value)
}

fn runtime_failure_with_evidence(
    route: &str,
    evidence_key: &str,
    evidence_value: impl ToString,
) -> RuntimeFailure {
    let mut err = RuntimeError::new(ERR_RUNTIME_BACKEND_FAILED, PLUGIN_ID, route);
    err.evidence.insert(
        evidence_key.into(),
        ScalarValue::String(evidence_value.to_string()),
    );
    RuntimeFailure::new(err)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use mutsuki_runtime_contracts::{
        AgentParticipation, AgentPhase, AgentSpec, ScopeRuleSpec, SideEffectPolicy, SourceRef,
    };
    use mutsuki_runtime_core::AgentRuntime;

    use super::*;

    fn client_with_response(response: String) -> CodexAppServerClient<Cursor<Vec<u8>>, Vec<u8>> {
        CodexAppServerClient::new(Cursor::new(response.into_bytes()), Vec::new())
    }

    fn envelope() -> Envelope {
        Envelope {
            id: "env-1".into(),
            timestamp: 1.0,
            source: SourceRef {
                source_id: DEFAULT_SOURCE_ID.into(),
                kind: DEFAULT_SOURCE_KIND.into(),
                metadata: BTreeMap::new(),
            },
            payload_schema_id: "codex.input".into(),
            capabilities_required: Vec::new(),
            payload: json!({
                "prompt": "decide",
                "cwd": "C:\\work",
                "settings": {"model": "gpt-5", "reasoningEffort": "medium"},
            }),
        }
    }

    fn agent() -> AgentSpec {
        AgentSpec {
            agent_id: "codex-agent".into(),
            owner: None,
            priority: 0,
            participation: AgentParticipation::PrimaryCandidate,
            accepts: vec![ScopeRuleSpec::BySourceId {
                source_id: DEFAULT_SOURCE_ID.into(),
            }],
            strategy_id: PLUGIN_ID.into(),
            side_effect_policy: SideEffectPolicy::ReadOnly,
        }
    }

    #[test]
    fn parse_version_accepts_suffix_and_missing_patch() {
        assert_eq!(
            parse_codex_cli_version("codex 0.128.0-beta.1"),
            Some((0, 128, 0))
        );
        assert_eq!(parse_codex_cli_version("codex 0.129"), Some((0, 129, 0)));
        assert_eq!(parse_codex_cli_version("codex unknown"), None);
    }

    #[test]
    fn probe_reports_missing_old_missing_app_server_and_available() {
        let missing = build_codex_app_server_probe_status_with(&[], |_, _| Ok(String::new()));
        assert_eq!(missing.failure_kind.as_deref(), Some("missingCli"));

        let old = build_codex_app_server_probe_status_with(&["old".into()], |_, args| match args {
            ["--version"] => Ok("codex 0.127.0".into()),
            ["app-server", "--help"] => Ok("Usage: codex app-server".into()),
            _ => Err("bad".into()),
        });
        assert_eq!(
            old.failure_kind.as_deref(),
            Some("experimentalApiUnsupported")
        );

        let no_app_server =
            build_codex_app_server_probe_status_with(&["codex".into()], |_, args| match args {
                ["--version"] => Ok("codex 0.128.0".into()),
                ["app-server", "--help"] => Err("unknown subcommand".into()),
                _ => Err("bad".into()),
            });
        assert_eq!(
            no_app_server.failure_kind.as_deref(),
            Some("appServerUnavailable")
        );

        let available =
            build_codex_app_server_probe_status_with(&["codex".into()], |_, args| match args {
                ["--version"] => Ok("codex 0.128.0".into()),
                ["app-server", "--help"] => Ok("Usage: codex app-server".into()),
                _ => Err("bad".into()),
            });
        assert!(available.supports_required_protocol);
        assert_eq!(available.path.as_deref(), Some("codex"));
    }

    #[test]
    fn windows_command_line_quotes_space_paths() {
        assert_eq!(
            windows_command_line("C:\\Program Files\\Codex\\codex.cmd", &["app-server"]),
            "\"C:\\Program Files\\Codex\\codex.cmd\" app-server"
        );
    }

    #[test]
    fn jsonl_client_writes_initialized_and_request() {
        let response = json!({"id": 1, "result": {"threadId": "thread-1"}}).to_string() + "\n";
        let client = client_with_response(response);

        client.initialized().unwrap();
        let result = client.request("thread/new", json!({})).unwrap();

        assert_eq!(result["threadId"], "thread-1");
        let (_reader, writer, _notifications) = client.into_inner();
        let written = String::from_utf8(writer).unwrap();
        let lines = written
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(lines[0]["method"], "initialized");
        assert_eq!(lines[1]["method"], "thread/new");
        assert_eq!(lines[1]["id"], 1);
    }

    #[test]
    fn jsonl_client_queues_notifications_before_response() {
        let response = [
            json!({"method": "turn/started", "params": {"turnId": "turn-1"}}).to_string(),
            json!({"id": 1, "result": {"threadId": "thread-1"}}).to_string(),
        ]
        .join("\n")
            + "\n";
        let client = client_with_response(response);

        let result = client.request("thread/new", json!({})).unwrap();

        assert_eq!(result["threadId"], "thread-1");
        assert_eq!(client.drain_notifications()[0]["method"], "turn/started");
    }

    #[test]
    fn jsonl_client_rejects_malformed_empty_error_and_id_mismatch() {
        let malformed = client_with_response("not json\n".into());
        assert_eq!(
            malformed
                .request("thread/new", json!({}))
                .unwrap_err()
                .error()
                .route,
            "codex.app_server.protocol"
        );

        let empty = client_with_response("\n".into());
        assert_eq!(
            empty
                .request("thread/new", json!({}))
                .unwrap_err()
                .error()
                .route,
            "codex.app_server.protocol"
        );

        let error =
            client_with_response(json!({"id": 1, "error": {"message": "boom"}}).to_string() + "\n");
        let err = error.request("thread/new", json!({})).unwrap_err();
        assert_eq!(err.error().route, "codex.app_server.protocol");
        assert_eq!(
            err.error().evidence.get("message"),
            Some(&ScalarValue::String("boom".into()))
        );

        let mismatch = client_with_response(json!({"id": 2, "result": null}).to_string() + "\n");
        let err = mismatch.request("thread/new", json!({})).unwrap_err();
        assert_eq!(err.error().route, "codex.app_server.protocol");
    }

    #[test]
    fn strategy_backend_maps_turn_completed_to_wait_input() {
        let response = [
            json!({"id": 1, "result": {"threadId": "thread-1"}}).to_string(),
            json!({"id": 2, "result": {"turnId": "turn-1"}}).to_string(),
            json!({"method": "turn/completed", "params": {"turnId": "turn-1"}}).to_string(),
        ]
        .join("\n")
            + "\n";
        let client = client_with_response(response);
        let mut backend = CodexAppServerBackend::new(client);

        backend.on_awake("agent-a").unwrap();
        let result = backend.on_input("agent-a", &envelope()).unwrap();

        assert_eq!(result.status, StrategyResultStatus::WaitInput);
        let (_reader, writer, _notifications) = backend.client.into_inner();
        let written = String::from_utf8(writer).unwrap();
        assert!(written.contains("\"method\":\"turn/start\""));
        assert!(written.contains("\"reasoningEffort\":\"medium\""));
    }

    #[test]
    fn strategy_backend_maps_turn_failed_to_structured_error() {
        let response = [
            json!({"id": 1, "result": {"threadId": "thread-1"}}).to_string(),
            json!({"id": 2, "result": {"turnId": "turn-1"}}).to_string(),
            json!({"method": "turn/failed", "params": {"error": {"message": "failed"}}})
                .to_string(),
        ]
        .join("\n")
            + "\n";
        let client = client_with_response(response);
        let mut backend = CodexAppServerBackend::new(client);

        backend.on_awake("agent-a").unwrap();
        let result = backend.on_input("agent-a", &envelope()).unwrap();

        assert_eq!(result.status, StrategyResultStatus::Failed);
        let error = result.error.unwrap();
        assert_eq!(error.route, "codex.turn.failed");
        assert_eq!(
            error.evidence.get("message"),
            Some(&ScalarValue::String("failed".into()))
        );
    }

    #[test]
    fn strategy_backend_surfaces_app_server_request_as_wait_input_decision() {
        let response = [
            json!({"id": 1, "result": {"threadId": "thread-1"}}).to_string(),
            json!({"id": 2, "result": {"turnId": "turn-1"}}).to_string(),
            json!({
                "id": 99,
                "method": "approval/request",
                "params": {"approvalId": "approval-1"}
            })
            .to_string(),
        ]
        .join("\n")
            + "\n";
        let client = client_with_response(response);
        let mut backend = CodexAppServerBackend::new(client);

        backend.on_awake("agent-a").unwrap();
        let result = backend.on_input("agent-a", &envelope()).unwrap();

        assert_eq!(result.status, StrategyResultStatus::WaitInput);
        let decision = result.decision.unwrap();
        assert_eq!(decision["kind"], "codex_app_server_request");
        assert_eq!(decision["method"], "approval/request");
    }

    #[test]
    fn runtime_routes_codex_agent_through_app_server_backend() {
        let response = [
            json!({"id": 1, "result": {"threadId": "thread-1"}}).to_string(),
            json!({"id": 2, "result": {"turnId": "turn-1"}}).to_string(),
            json!({"method": "turn/completed", "params": {"turnId": "turn-1"}}).to_string(),
            json!({"id": 3, "result": null}).to_string(),
        ]
        .join("\n")
            + "\n";
        let client = client_with_response(response);
        let mut backend = CodexAppServerBackend::new(client);
        let mut runtime = AgentRuntime::new();

        runtime.register_agent(agent()).unwrap();
        runtime.start_agent("codex-agent", &mut backend).unwrap();
        assert_eq!(runtime.phase("codex-agent"), Some(&AgentPhase::Awake));
        assert_eq!(runtime.publish(envelope()).unwrap(), vec!["codex-agent"]);

        let result = runtime.tick_once("codex-agent", &mut backend).unwrap();
        runtime.stop_agent("codex-agent", &mut backend).unwrap();

        assert_eq!(result.status, StrategyResultStatus::WaitInput);
        assert!(
            runtime
                .events()
                .iter()
                .any(|event| event.name == "agent.input")
        );
    }

    #[test]
    fn runtime_records_failed_turn_event_error() {
        let response = [
            json!({"id": 1, "result": {"threadId": "thread-1"}}).to_string(),
            json!({"id": 2, "result": {"turnId": "turn-1"}}).to_string(),
            json!({"method": "turn/failed", "params": {"message": "failed"}}).to_string(),
        ]
        .join("\n")
            + "\n";
        let client = client_with_response(response);
        let mut backend = CodexAppServerBackend::new(client);
        let mut runtime = AgentRuntime::new();

        runtime.register_agent(agent()).unwrap();
        runtime.start_agent("codex-agent", &mut backend).unwrap();
        runtime.publish(envelope()).unwrap();
        let result = runtime.tick_once("codex-agent", &mut backend).unwrap();

        assert_eq!(result.status, StrategyResultStatus::Failed);
        let event = runtime
            .events()
            .into_iter()
            .find(|event| event.name == "agent.input.error")
            .unwrap();
        assert_eq!(event.error.unwrap().route, "codex.turn.failed");
    }
}
