// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::cast_sign_loss,
    clippy::ignored_unit_patterns,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::too_many_lines
)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use mutsuki_agent_contracts::{
    AgentError, AgentPluginStateKind, AgentServiceDescriptor, AgentToolDescriptor,
    ContextProviderRequest, ContextProviderResult, LspDiagnostic, LspDocumentId,
    LspDocumentSnapshot, LspPosition, LspQueryResult, LspRange, LspServerDescriptor,
    LspServiceRequest, LspServiceResponse, LspWorkspaceEditPlan, LspWorkspaceId, LspWorkspaceState,
    LspWorkspaceStatus, PermissionRequest, ToolSideEffect,
};
use mutsuki_agent_plugin_api::{AgentService, ContextProvider, ToolProvider};
use mutsuki_agent_runtime::AgentResourceStore;
use serde_json::{Value, json};

pub const SERVICE_ID: &str = mutsuki_agent_contracts::AGENT_LSP_PROTOCOL;
pub const CONTEXT_PROVIDER_ID: &str = "mutsuki.agent.context.lsp";
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub trait LspProcessFactory: Send + Sync {
    fn spawn(&self, descriptor: &LspServerDescriptor) -> Result<Box<dyn LspProcess>, AgentError>;
}

pub trait LspProcess: Send {
    fn send(&mut self, value: &Value) -> Result<(), AgentError>;
    fn receive(&mut self, timeout: Duration) -> Result<Option<Value>, AgentError>;
    fn is_alive(&mut self) -> Result<bool, AgentError>;
    fn terminate(&mut self) -> Result<(), AgentError>;
}

#[derive(Default)]
pub struct StdioLspProcessFactory;

impl LspProcessFactory for StdioLspProcessFactory {
    fn spawn(&self, descriptor: &LspServerDescriptor) -> Result<Box<dyn LspProcess>, AgentError> {
        let mut child = Command::new(&descriptor.command)
            .args(&descriptor.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| AgentError::new("agent.lsp.spawn_failed", err.to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AgentError::new("agent.lsp.spawn_failed", "stdin is unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AgentError::new("agent.lsp.spawn_failed", "stdout is unavailable"))?;
        let (messages, receiver) = mpsc::sync_channel(64);
        thread::Builder::new()
            .name(format!("mutsuki-lsp-reader-{}", descriptor.server_id))
            .spawn(move || {
                let mut stdout = BufReader::new(stdout);
                loop {
                    let result = read_lsp_frame(&mut stdout);
                    let stop = result.is_err();
                    if messages.send(result).is_err() || stop {
                        break;
                    }
                }
            })
            .map_err(|err| AgentError::new("agent.lsp.spawn_failed", err.to_string()))?;
        Ok(Box::new(StdioLspProcess {
            child,
            stdin: BufWriter::new(stdin),
            receiver,
        }))
    }
}

struct StdioLspProcess {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    receiver: Receiver<Result<Value, AgentError>>,
}

impl LspProcess for StdioLspProcess {
    fn send(&mut self, value: &Value) -> Result<(), AgentError> {
        let body = serde_json::to_vec(value)
            .map_err(|err| AgentError::new("agent.lsp.encode_failed", err.to_string()))?;
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len())
            .and_then(|_| self.stdin.write_all(&body))
            .and_then(|_| self.stdin.flush())
            .map_err(|err| AgentError::new("agent.lsp.write_failed", err.to_string()))
    }

    fn receive(&mut self, timeout: Duration) -> Result<Option<Value>, AgentError> {
        match self.receiver.recv_timeout(timeout) {
            Ok(result) => result.map(Some),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(AgentError::new(
                "agent.lsp.closed",
                "language server response stream closed",
            )),
        }
    }

    fn is_alive(&mut self) -> Result<bool, AgentError> {
        self.child
            .try_wait()
            .map(|status| status.is_none())
            .map_err(|err| AgentError::new("agent.lsp.process_failed", err.to_string()))
    }

    fn terminate(&mut self) -> Result<(), AgentError> {
        if self
            .child
            .try_wait()
            .map_err(|err| AgentError::new("agent.lsp.process_failed", err.to_string()))?
            .is_none()
        {
            self.child
                .kill()
                .map_err(|err| AgentError::new("agent.lsp.process_failed", err.to_string()))?;
        }
        self.child
            .wait()
            .map_err(|err| AgentError::new("agent.lsp.process_failed", err.to_string()))?;
        Ok(())
    }
}

fn read_lsp_frame(stdout: &mut BufReader<ChildStdout>) -> Result<Value, AgentError> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        stdout
            .read_line(&mut line)
            .map_err(|err| AgentError::new("agent.lsp.read_failed", err.to_string()))?;
        if line.is_empty() {
            return Err(AgentError::new(
                "agent.lsp.closed",
                "language server closed stdout",
            ));
        }
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = Some(value.trim().parse::<usize>().map_err(|_| {
                AgentError::new("agent.lsp.invalid_frame", "invalid Content-Length")
            })?);
        }
    }
    let length = content_length
        .ok_or_else(|| AgentError::new("agent.lsp.invalid_frame", "Content-Length is missing"))?;
    if length > 16 * 1024 * 1024 {
        return Err(AgentError::new(
            "agent.lsp.frame_too_large",
            "language server frame exceeds 16 MiB",
        ));
    }
    let mut body = vec![0; length];
    stdout
        .read_exact(&mut body)
        .map_err(|err| AgentError::new("agent.lsp.read_failed", err.to_string()))?;
    serde_json::from_slice(&body)
        .map_err(|err| AgentError::new("agent.lsp.decode_failed", err.to_string()))
}

#[derive(Clone, Default)]
pub struct LspCancellation {
    cancelled: Arc<AtomicBool>,
}

impl LspCancellation {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
pub struct LspRequestControl {
    pub timeout: Duration,
    pub cancellation: LspCancellation,
}

impl Default for LspRequestControl {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_REQUEST_TIMEOUT,
            cancellation: LspCancellation::default(),
        }
    }
}

struct LspSession {
    workspace: LspWorkspaceId,
    descriptor: LspServerDescriptor,
    process: Box<dyn LspProcess>,
    next_request_id: u64,
    documents: BTreeMap<String, LspDocumentSnapshot>,
    diagnostics: BTreeMap<String, Vec<LspDiagnostic>>,
    state: LspWorkspaceState,
    restart_count: u64,
    last_error: Option<String>,
}

impl LspSession {
    fn request(&mut self, method: &str, params: Value) -> Result<Value, AgentError> {
        self.request_with_control(method, params, &LspRequestControl::default())
    }

    fn request_with_control(
        &mut self,
        method: &str,
        params: Value,
        control: &LspRequestControl,
    ) -> Result<Value, AgentError> {
        if !self.process.is_alive()? {
            self.state = LspWorkspaceState::Failed;
            self.last_error = Some("language server exited".into());
            return Err(AgentError::new(
                "agent.lsp.crashed",
                "language server process exited",
            ));
        }
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.process
            .send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let deadline = Instant::now() + control.timeout;
        loop {
            if control.cancellation.is_cancelled() {
                self.cancel_request(id)?;
                return Err(AgentError::new(
                    "agent.lsp.cancelled",
                    "language server request was cancelled",
                ));
            }
            let now = Instant::now();
            if now >= deadline {
                self.cancel_request(id)?;
                return Err(AgentError::new(
                    "agent.lsp.timeout",
                    format!("language server request `{method}` timed out"),
                ));
            }
            let Some(message) = self
                .process
                .receive((deadline - now).min(Duration::from_millis(25)))?
            else {
                continue;
            };
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = message.get("error") {
                    let code = match error.get("code").and_then(Value::as_i64) {
                        Some(-32801) => "agent.lsp.content_modified",
                        Some(-32800) => "agent.lsp.cancelled",
                        _ => "agent.lsp.request_failed",
                    };
                    return Err(AgentError::new(code, error.to_string()));
                }
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
            self.handle_server_message(message)?;
        }
    }

    fn cancel_request(&mut self, id: u64) -> Result<(), AgentError> {
        self.notify("$/cancelRequest", json!({"id": id}))
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), AgentError> {
        self.process
            .send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    fn handle_server_message(&mut self, message: Value) -> Result<(), AgentError> {
        if message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
        {
            let uri = message
                .pointer("/params/uri")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let diagnostics = serde_json::from_value(
                message
                    .pointer("/params/diagnostics")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
            )
            .map_err(|err| AgentError::new("agent.lsp.invalid_diagnostics", err.to_string()))?;
            self.diagnostics.insert(uri, diagnostics);
        }
        if let (Some(id), Some(method)) = (
            message.get("id").cloned(),
            message.get("method").and_then(Value::as_str),
        ) {
            let result = if method == "workspace/configuration" {
                let count = message
                    .pointer("/params/items")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                Value::Array(vec![Value::Null; count])
            } else {
                Value::Null
            };
            self.process
                .send(&json!({"jsonrpc": "2.0", "id": id, "result": result}))?;
        }
        Ok(())
    }

    fn pump_notifications(&mut self, timeout: Duration) -> Result<(), AgentError> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let Some(message) = self
                .process
                .receive(remaining.min(Duration::from_millis(10)))?
            else {
                break;
            };
            self.handle_server_message(message)?;
        }
        Ok(())
    }

    fn status(&mut self) -> LspWorkspaceStatus {
        if self.state == LspWorkspaceState::Ready {
            match self.process.is_alive() {
                Ok(true) => {}
                Ok(false) => {
                    self.state = LspWorkspaceState::Failed;
                    self.last_error = Some("language server exited".into());
                }
                Err(error) => {
                    self.state = LspWorkspaceState::Failed;
                    self.last_error = Some(error.to_string());
                }
            }
        }
        LspWorkspaceStatus {
            workspace: self.workspace.clone(),
            server_id: self.descriptor.server_id.clone(),
            state: self.state.clone(),
            open_documents: self.documents.len(),
            restart_count: self.restart_count,
            last_error: self.last_error.clone(),
        }
    }
}

pub struct SharedLspService {
    descriptor: AgentServiceDescriptor,
    factory: Arc<dyn LspProcessFactory>,
    resources: AgentResourceStore,
    sessions: Mutex<BTreeMap<LspWorkspaceId, LspSession>>,
}

impl SharedLspService {
    pub fn new(factory: Arc<dyn LspProcessFactory>) -> Self {
        Self::with_resources(factory, AgentResourceStore::default())
    }

    pub fn with_resources(
        factory: Arc<dyn LspProcessFactory>,
        resources: AgentResourceStore,
    ) -> Self {
        Self {
            descriptor: AgentServiceDescriptor {
                service_id: SERVICE_ID.into(),
                version: "1".into(),
                request_schema: "mutsuki.agent.lsp.request@1".into(),
                response_schema: "mutsuki.agent.lsp.response@1".into(),
                state: AgentPluginStateKind::Stateful,
                affinity: Some("workspace".into()),
            },
            factory,
            resources,
            sessions: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn open_workspace(
        &self,
        workspace: LspWorkspaceId,
        descriptor: LspServerDescriptor,
    ) -> Result<(), AgentError> {
        let mut sessions = self.sessions.lock().expect("lsp sessions mutex");
        if sessions.contains_key(&workspace) {
            return Ok(());
        }
        let session = self.spawn_session(
            workspace.clone(),
            descriptor,
            0,
            BTreeMap::new(),
            LspWorkspaceState::Starting,
        )?;
        sessions.insert(workspace, session);
        Ok(())
    }

    fn spawn_session(
        &self,
        workspace: LspWorkspaceId,
        descriptor: LspServerDescriptor,
        restart_count: u64,
        documents: BTreeMap<String, LspDocumentSnapshot>,
        initial_state: LspWorkspaceState,
    ) -> Result<LspSession, AgentError> {
        let process = self.factory.spawn(&descriptor)?;
        let mut session = LspSession {
            workspace,
            descriptor: descriptor.clone(),
            process,
            next_request_id: 1,
            documents: BTreeMap::new(),
            diagnostics: BTreeMap::new(),
            state: initial_state,
            restart_count,
            last_error: None,
        };
        session.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": &descriptor.workspace_uri,
                "workspaceFolders": [{
                    "uri": &descriptor.workspace_uri,
                    "name": &descriptor.server_id,
                }],
                "capabilities": {
                    "workspace": {"configuration": true, "workspaceFolders": true},
                    "textDocument": {
                        "definition": {"linkSupport": true},
                        "diagnostic": {},
                    },
                },
                "initializationOptions": descriptor
                    .initialization_options
                    .clone()
                    .unwrap_or_else(|| json!({})),
            }),
        )?;
        session.notify("initialized", json!({}))?;
        for document in documents.into_values() {
            session.notify(
                "textDocument/didOpen",
                json!({"textDocument": {
                    "uri": document.document.uri,
                    "languageId": document.language_id,
                    "version": document.version,
                    "text": document.text,
                }}),
            )?;
            session
                .documents
                .insert(document.document.uri.clone(), document);
        }
        session.state = LspWorkspaceState::Ready;
        Ok(session)
    }

    pub fn open_document(&self, document: LspDocumentSnapshot) -> Result<(), AgentError> {
        let mut sessions = self.sessions.lock().expect("lsp sessions mutex");
        let session = sessions
            .get_mut(&document.document.workspace)
            .ok_or_else(|| AgentError::not_found("LSP workspace is not open"))?;
        session.notify(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": &document.document.uri,
                "languageId": &document.language_id,
                "version": document.version,
                "text": &document.text,
            }}),
        )?;
        session
            .documents
            .insert(document.document.uri.clone(), document);
        Ok(())
    }

    pub fn change_document(
        &self,
        document: &LspDocumentId,
        version: i64,
        text: String,
    ) -> Result<(), AgentError> {
        let mut sessions = self.sessions.lock().expect("lsp sessions mutex");
        let session = sessions
            .get_mut(&document.workspace)
            .ok_or_else(|| AgentError::not_found("LSP workspace is not open"))?;
        let current = session
            .documents
            .get(&document.uri)
            .cloned()
            .ok_or_else(|| AgentError::not_found("LSP document is not open"))?;
        if version <= current.version {
            return Err(AgentError::new(
                "agent.lsp.document_version_conflict",
                "document version must increase",
            ));
        }
        session.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": document.uri, "version": version},
                "contentChanges": [{"text": text}],
            }),
        )?;
        session.documents.insert(
            document.uri.clone(),
            LspDocumentSnapshot {
                document: document.clone(),
                language_id: current.language_id,
                version,
                text,
            },
        );
        Ok(())
    }

    pub fn save_document(
        &self,
        document: &LspDocumentId,
        text: Option<String>,
    ) -> Result<(), AgentError> {
        let mut sessions = self.sessions.lock().expect("lsp sessions mutex");
        let session = sessions
            .get_mut(&document.workspace)
            .ok_or_else(|| AgentError::not_found("LSP workspace is not open"))?;
        if !session.documents.contains_key(&document.uri) {
            return Err(AgentError::not_found("LSP document is not open"));
        }
        let mut params = json!({"textDocument": {"uri": document.uri}});
        if let Some(text) = text {
            params
                .as_object_mut()
                .expect("didSave params object")
                .insert("text".into(), Value::String(text));
        }
        session.notify("textDocument/didSave", params)
    }

    pub fn close_document(&self, document: &LspDocumentId) -> Result<(), AgentError> {
        let mut sessions = self.sessions.lock().expect("lsp sessions mutex");
        let session = sessions
            .get_mut(&document.workspace)
            .ok_or_else(|| AgentError::not_found("LSP workspace is not open"))?;
        if session.documents.remove(&document.uri).is_none() {
            return Err(AgentError::not_found("LSP document is not open"));
        }
        session.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": document.uri}}),
        )?;
        session.diagnostics.remove(&document.uri);
        Ok(())
    }

    pub fn definition(
        &self,
        document: &LspDocumentId,
        position: LspPosition,
    ) -> Result<LspQueryResult, AgentError> {
        self.query(document, position, "textDocument/definition", Value::Null)
    }

    pub fn declaration(
        &self,
        document: &LspDocumentId,
        position: LspPosition,
    ) -> Result<LspQueryResult, AgentError> {
        self.query(document, position, "textDocument/declaration", Value::Null)
    }

    pub fn implementation(
        &self,
        document: &LspDocumentId,
        position: LspPosition,
    ) -> Result<LspQueryResult, AgentError> {
        self.query(
            document,
            position,
            "textDocument/implementation",
            Value::Null,
        )
    }

    pub fn hover(
        &self,
        document: &LspDocumentId,
        position: LspPosition,
    ) -> Result<LspQueryResult, AgentError> {
        self.query(document, position, "textDocument/hover", Value::Null)
    }

    pub fn references(
        &self,
        document: &LspDocumentId,
        position: LspPosition,
    ) -> Result<LspQueryResult, AgentError> {
        self.query(
            document,
            position,
            "textDocument/references",
            json!({"context": {"includeDeclaration": true}}),
        )
    }

    pub fn rename(
        &self,
        document: &LspDocumentId,
        position: LspPosition,
        new_name: String,
    ) -> Result<LspQueryResult, AgentError> {
        self.query(
            document,
            position,
            "textDocument/rename",
            json!({"newName": new_name}),
        )
    }

    pub fn rename_plan(
        &self,
        document: &LspDocumentId,
        position: LspPosition,
        new_name: String,
        session_id: &str,
        turn_id: &str,
    ) -> Result<LspWorkspaceEditPlan, AgentError> {
        let edit = self.rename(document, position, new_name)?;
        self.edit_plan(document, "rename", edit, session_id, turn_id)
    }

    pub fn document_symbols(&self, document: &LspDocumentId) -> Result<LspQueryResult, AgentError> {
        self.query_document(document, "textDocument/documentSymbol", json!({}))
    }

    pub fn workspace_symbols(
        &self,
        workspace: &LspWorkspaceId,
        query: String,
    ) -> Result<LspQueryResult, AgentError> {
        let mut sessions = self.sessions.lock().expect("lsp sessions mutex");
        let session = sessions
            .get_mut(workspace)
            .ok_or_else(|| AgentError::not_found("LSP workspace is not open"))?;
        let result = session.request("workspace/symbol", json!({"query": query}))?;
        self.query_result("workspace/symbol", result)
    }

    pub fn code_actions_plan(
        &self,
        document: &LspDocumentId,
        range: LspRange,
        context: Value,
        session_id: &str,
        turn_id: &str,
    ) -> Result<LspWorkspaceEditPlan, AgentError> {
        let edit = self.query_document(
            document,
            "textDocument/codeAction",
            json!({"range": range, "context": context}),
        )?;
        self.edit_plan(document, "code_action", edit, session_id, turn_id)
    }

    pub fn formatting(
        &self,
        document: &LspDocumentId,
        options: Value,
    ) -> Result<LspQueryResult, AgentError> {
        self.query_document(
            document,
            "textDocument/formatting",
            json!({"options": options}),
        )
    }

    pub fn diagnostics(&self, document: &LspDocumentId) -> Result<Vec<LspDiagnostic>, AgentError> {
        let mut sessions = self.sessions.lock().expect("lsp sessions mutex");
        let session = sessions
            .get_mut(&document.workspace)
            .ok_or_else(|| AgentError::not_found("LSP workspace is not open"))?;
        let pull = request_resilient(
            session,
            "textDocument/diagnostic",
            json!({"textDocument": {"uri": document.uri}}),
            &LspRequestControl {
                timeout: Duration::from_millis(500),
                cancellation: LspCancellation::default(),
            },
        );
        match pull {
            Ok(result) => {
                let diagnostics = serde_json::from_value(
                    result.get("items").cloned().unwrap_or_else(|| json!([])),
                )
                .map_err(|error| {
                    AgentError::new("agent.lsp.invalid_diagnostics", error.to_string())
                })?;
                session
                    .diagnostics
                    .insert(document.uri.clone(), diagnostics);
            }
            Err(error)
                if matches!(
                    error.code.as_str(),
                    "agent.lsp.request_failed" | "agent.lsp.timeout" | "agent.lsp.content_modified"
                ) => {}
            Err(error) => return Err(error),
        }
        session.pump_notifications(Duration::from_millis(50))?;
        Ok(session
            .diagnostics
            .get(&document.uri)
            .cloned()
            .unwrap_or_default())
    }

    pub fn restart(&self, workspace: &LspWorkspaceId) -> Result<(), AgentError> {
        let (descriptor, documents, restart_count) = {
            let mut sessions = self.sessions.lock().expect("lsp sessions mutex");
            let mut session = sessions
                .remove(workspace)
                .ok_or_else(|| AgentError::not_found("LSP workspace is not open"))?;
            session.state = LspWorkspaceState::Restarting;
            session.process.terminate()?;
            (
                session.descriptor,
                session.documents,
                session.restart_count.saturating_add(1),
            )
        };
        let session = self.spawn_session(
            workspace.clone(),
            descriptor,
            restart_count,
            documents,
            LspWorkspaceState::Restarting,
        )?;
        self.sessions
            .lock()
            .expect("lsp sessions mutex")
            .insert(workspace.clone(), session);
        Ok(())
    }

    pub fn close_workspace(&self, workspace: &LspWorkspaceId) -> Result<(), AgentError> {
        let mut session = self
            .sessions
            .lock()
            .expect("lsp sessions mutex")
            .remove(workspace)
            .ok_or_else(|| AgentError::not_found("LSP workspace is not open"))?;
        session.state = LspWorkspaceState::Draining;
        let documents = session.documents.keys().cloned().collect::<Vec<_>>();
        for uri in documents {
            let _ = session.notify(
                "textDocument/didClose",
                json!({"textDocument": {"uri": uri}}),
            );
        }
        let _ = session.request("shutdown", Value::Null);
        let _ = session.notify("exit", Value::Null);
        session.process.terminate()
    }

    pub fn workspace_status(
        &self,
        workspace: &LspWorkspaceId,
    ) -> Result<LspWorkspaceStatus, AgentError> {
        self.sessions
            .lock()
            .expect("lsp sessions mutex")
            .get_mut(workspace)
            .map(LspSession::status)
            .ok_or_else(|| AgentError::not_found("LSP workspace is not open"))
    }

    /// Snapshot every workspace owned by this shared service instance.
    ///
    /// Product diagnostics and Agent tools must observe the same sessions; this
    /// method exposes their public status without creating a second LSP client.
    pub fn list_workspaces(&self) -> Vec<LspWorkspaceStatus> {
        self.sessions
            .lock()
            .expect("lsp sessions mutex")
            .values_mut()
            .map(LspSession::status)
            .collect()
    }

    pub fn active_workspace_count(&self) -> usize {
        self.sessions.lock().expect("lsp sessions mutex").len()
    }

    fn query(
        &self,
        document: &LspDocumentId,
        position: LspPosition,
        method: &str,
        extra: Value,
    ) -> Result<LspQueryResult, AgentError> {
        self.query_with_control(
            document,
            position,
            method,
            extra,
            &LspRequestControl::default(),
        )
    }

    pub fn query_with_control(
        &self,
        document: &LspDocumentId,
        position: LspPosition,
        method: &str,
        extra: Value,
        control: &LspRequestControl,
    ) -> Result<LspQueryResult, AgentError> {
        let mut sessions = self.sessions.lock().expect("lsp sessions mutex");
        let session = sessions
            .get_mut(&document.workspace)
            .ok_or_else(|| AgentError::not_found("LSP workspace is not open"))?;
        let mut params = json!({
            "textDocument": {"uri": document.uri},
            "position": position,
        });
        if let (Some(target), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
            target.extend(extra.clone());
        }
        let result = request_resilient(session, method, params, control)?;
        self.query_result(method, result)
    }

    fn query_document(
        &self,
        document: &LspDocumentId,
        method: &str,
        extra: Value,
    ) -> Result<LspQueryResult, AgentError> {
        let mut sessions = self.sessions.lock().expect("lsp sessions mutex");
        let session = sessions
            .get_mut(&document.workspace)
            .ok_or_else(|| AgentError::not_found("LSP workspace is not open"))?;
        if !session.documents.contains_key(&document.uri) {
            return Err(AgentError::not_found("LSP document is not open"));
        }
        let mut params = json!({"textDocument": {"uri": document.uri}});
        if let (Some(target), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
            target.extend(extra.clone());
        }
        let result = request_resilient(session, method, params, &LspRequestControl::default())?;
        self.query_result(method, result)
    }

    fn query_result(&self, method: &str, result: Value) -> Result<LspQueryResult, AgentError> {
        let bytes = serde_json::to_vec(&result)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let summary = format!("{method} completed with {} response bytes", bytes.len());
        if bytes.len() <= 64 * 1024 {
            return Ok(LspQueryResult {
                summary,
                inline: Some(result),
                details: None,
            });
        }
        let details = self.resources.put_json(
            SERVICE_ID,
            "mutsuki.agent.lsp.result",
            "mutsuki.agent.lsp.result@1",
            1,
            &result,
        )?;
        Ok(LspQueryResult {
            summary,
            inline: None,
            details: Some(details),
        })
    }

    fn edit_plan(
        &self,
        document: &LspDocumentId,
        operation: &str,
        edit: LspQueryResult,
        session_id: &str,
        turn_id: &str,
    ) -> Result<LspWorkspaceEditPlan, AgentError> {
        let version = self
            .sessions
            .lock()
            .expect("lsp sessions mutex")
            .get(&document.workspace)
            .and_then(|session| session.documents.get(&document.uri))
            .map(|document| document.version)
            .ok_or_else(|| AgentError::not_found("LSP document is not open"))?;
        let action_id = format!(
            "lsp:{operation}:{}:{version}",
            document.uri.replace(':', "_")
        );
        Ok(LspWorkspaceEditPlan {
            action_id: action_id.clone(),
            operation: operation.into(),
            edit,
            approval: PermissionRequest {
                session_id: session_id.into(),
                turn_id: turn_id.into(),
                action_id,
                tool: format!("lsp.{operation}"),
                side_effect: ToolSideEffect::WorkspaceWrite,
                summary: format!("{operation} workspace edit for {}", document.uri),
                version: version.max(0) as u64,
            },
        })
    }
}

fn request_resilient(
    session: &mut LspSession,
    method: &str,
    params: Value,
    control: &LspRequestControl,
) -> Result<Value, AgentError> {
    for attempt in 0..3 {
        match session.request_with_control(method, params.clone(), control) {
            Err(error) if error.code == "agent.lsp.content_modified" && attempt < 2 => {
                if control.cancellation.is_cancelled() {
                    return Err(AgentError::new(
                        "agent.lsp.cancelled",
                        "language server request was cancelled",
                    ));
                }
                thread::sleep(Duration::from_millis(5));
            }
            result => return result,
        }
    }
    unreachable!("content-modified retry loop always returns")
}

impl AgentService for SharedLspService {
    fn descriptor(&self) -> &AgentServiceDescriptor {
        &self.descriptor
    }

    fn call(&self, request: Value) -> Result<Value, AgentError> {
        let request: LspServiceRequest = serde_json::from_value(request)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let response = match request {
            LspServiceRequest::OpenWorkspace {
                workspace,
                descriptor,
            } => {
                self.open_workspace(workspace, descriptor)?;
                LspServiceResponse::Ack
            }
            LspServiceRequest::RestartWorkspace { workspace } => {
                self.restart(&workspace)?;
                LspServiceResponse::Ack
            }
            LspServiceRequest::CloseWorkspace { workspace } => {
                self.close_workspace(&workspace)?;
                LspServiceResponse::Ack
            }
            LspServiceRequest::WorkspaceStatus { workspace } => {
                LspServiceResponse::WorkspaceStatus(self.workspace_status(&workspace)?)
            }
            LspServiceRequest::OpenDocument { document } => {
                self.open_document(document)?;
                LspServiceResponse::Ack
            }
            LspServiceRequest::ChangeDocument {
                document,
                version,
                text,
            } => {
                self.change_document(&document, version, text)?;
                LspServiceResponse::Ack
            }
            LspServiceRequest::SaveDocument { document, text } => {
                self.save_document(&document, text)?;
                LspServiceResponse::Ack
            }
            LspServiceRequest::CloseDocument { document } => {
                self.close_document(&document)?;
                LspServiceResponse::Ack
            }
            LspServiceRequest::Definition { document, position } => {
                LspServiceResponse::Query(self.definition(&document, position)?)
            }
            LspServiceRequest::Declaration { document, position } => {
                LspServiceResponse::Query(self.declaration(&document, position)?)
            }
            LspServiceRequest::Implementation { document, position } => {
                LspServiceResponse::Query(self.implementation(&document, position)?)
            }
            LspServiceRequest::Hover { document, position } => {
                LspServiceResponse::Query(self.hover(&document, position)?)
            }
            LspServiceRequest::References { document, position } => {
                LspServiceResponse::Query(self.references(&document, position)?)
            }
            LspServiceRequest::RenamePlan {
                document,
                position,
                new_name,
                session_id,
                turn_id,
            } => LspServiceResponse::EditPlan(self.rename_plan(
                &document,
                position,
                new_name,
                &session_id,
                &turn_id,
            )?),
            LspServiceRequest::DocumentSymbols { document } => {
                LspServiceResponse::Query(self.document_symbols(&document)?)
            }
            LspServiceRequest::WorkspaceSymbols { workspace, query } => {
                LspServiceResponse::Query(self.workspace_symbols(&workspace, query)?)
            }
            LspServiceRequest::Diagnostics { document } => {
                LspServiceResponse::Diagnostics(self.diagnostics(&document)?)
            }
            LspServiceRequest::CodeActionsPlan {
                document,
                range,
                context,
                session_id,
                turn_id,
            } => LspServiceResponse::EditPlan(self.code_actions_plan(
                &document,
                range,
                context,
                &session_id,
                &turn_id,
            )?),
            LspServiceRequest::Formatting { document, options } => {
                LspServiceResponse::Query(self.formatting(&document, options)?)
            }
        };
        serde_json::to_value(response).map_err(|error| AgentError::invalid_input(error.to_string()))
    }

    fn drain(&self) -> Result<(), AgentError> {
        for session in self
            .sessions
            .lock()
            .expect("lsp sessions mutex")
            .values_mut()
        {
            session.state = LspWorkspaceState::Draining;
        }
        Ok(())
    }

    fn dispose(&self) -> Result<(), AgentError> {
        let workspaces = self
            .sessions
            .lock()
            .expect("lsp sessions mutex")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut first_error = None;
        for workspace in workspaces {
            if let Err(error) = self.close_workspace(&workspace)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl ToolProvider for SharedLspService {
    fn tools(&self) -> Vec<AgentToolDescriptor> {
        [
            ("lsp.definition", ToolSideEffect::WorkspaceRead),
            ("lsp.declaration", ToolSideEffect::WorkspaceRead),
            ("lsp.implementation", ToolSideEffect::WorkspaceRead),
            ("lsp.hover", ToolSideEffect::WorkspaceRead),
            ("lsp.references", ToolSideEffect::WorkspaceRead),
            ("lsp.document_symbols", ToolSideEffect::WorkspaceRead),
            ("lsp.workspace_symbols", ToolSideEffect::WorkspaceRead),
            ("lsp.diagnostics", ToolSideEffect::WorkspaceRead),
            ("lsp.rename_plan", ToolSideEffect::WorkspaceWrite),
            ("lsp.code_actions_plan", ToolSideEffect::WorkspaceWrite),
            ("lsp.formatting", ToolSideEffect::WorkspaceRead),
        ]
        .into_iter()
        .map(|(name, side_effect)| {
            let mut tool = AgentToolDescriptor::new(
                name,
                format!("mutsuki.agent.tool.{name}@1"),
                format!("Run the {name} language-server operation"),
            );
            tool.side_effect = side_effect;
            tool
        })
        .collect()
    }
}

impl ContextProvider for SharedLspService {
    fn provider_id(&self) -> &str {
        CONTEXT_PROVIDER_ID
    }

    fn collect(
        &self,
        request: ContextProviderRequest,
    ) -> Result<ContextProviderResult, AgentError> {
        let workspace = request
            .input
            .get("workspace")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::invalid_input("LSP context requires workspace"))?;
        let status = self.workspace_status(&LspWorkspaceId(workspace.into()))?;
        let value = serde_json::to_value(&status)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let details = self.resources.put_json(
            SERVICE_ID,
            "mutsuki.agent.lsp.status",
            "mutsuki.agent.lsp.status@1",
            status.restart_count.saturating_add(1),
            &value,
        )?;
        let summary = format!(
            "{} is {:?} with {} open documents",
            status.server_id, status.state, status.open_documents
        );
        Ok(ContextProviderResult {
            provider_id: request.provider_id,
            estimated_tokens: (summary.len() as u64).div_ceil(4),
            estimated_bytes: summary.len() as u64,
            summary,
            details: Some(details),
            priority: 0,
            required: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, VecDeque};
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use super::*;

    fn file_uri(path: &Path) -> String {
        url::Url::from_file_path(path)
            .expect("fixture path converts to a file URL")
            .to_string()
    }

    struct MockFactory {
        responses: BTreeMap<String, Value>,
        stalled: BTreeSet<String>,
        alive: Arc<AtomicBool>,
        sent: Arc<Mutex<Vec<Value>>>,
    }

    impl MockFactory {
        fn new() -> Self {
            Self {
                responses: BTreeMap::new(),
                stalled: BTreeSet::new(),
                alive: Arc::new(AtomicBool::new(true)),
                sent: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl LspProcessFactory for MockFactory {
        fn spawn(
            &self,
            _descriptor: &LspServerDescriptor,
        ) -> Result<Box<dyn LspProcess>, AgentError> {
            Ok(Box::new(MockProcess {
                responses: self.responses.clone(),
                stalled: self.stalled.clone(),
                alive: self.alive.clone(),
                sent: self.sent.clone(),
                pending: VecDeque::new(),
            }))
        }
    }

    struct MockProcess {
        responses: BTreeMap<String, Value>,
        stalled: BTreeSet<String>,
        alive: Arc<AtomicBool>,
        sent: Arc<Mutex<Vec<Value>>>,
        pending: VecDeque<Value>,
    }

    impl LspProcess for MockProcess {
        fn send(&mut self, value: &Value) -> Result<(), AgentError> {
            self.sent.lock().unwrap().push(value.clone());
            let Some(id) = value.get("id").and_then(Value::as_u64) else {
                return Ok(());
            };
            let method = value
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !self.stalled.contains(method) {
                self.pending.push_back(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": self.responses.get(method).cloned().unwrap_or(Value::Null),
                }));
            }
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

    fn mock_descriptor() -> LspServerDescriptor {
        LspServerDescriptor {
            server_id: "mock".into(),
            command: "unused".into(),
            args: Vec::new(),
            workspace_uri: "file:///workspace".into(),
            initialization_options: None,
        }
    }

    fn mock_document(workspace: &LspWorkspaceId) -> LspDocumentSnapshot {
        LspDocumentSnapshot {
            document: LspDocumentId {
                workspace: workspace.clone(),
                uri: "file:///workspace/main.rs".into(),
            },
            language_id: "rust".into(),
            version: 1,
            text: "fn main() {}".into(),
        }
    }

    #[test]
    fn service_surface_uses_resource_refs_approval_and_replays_unsaved_documents() {
        let mut factory = MockFactory::new();
        factory
            .responses
            .insert("textDocument/hover".into(), json!("x".repeat(70_000)));
        factory.responses.insert(
            "textDocument/rename".into(),
            json!({"changes": {"file:///workspace/main.rs": []}}),
        );
        let sent = factory.sent.clone();
        let workspace = LspWorkspaceId("workspace".into());
        let document = mock_document(&workspace);
        let service = SharedLspService::new(Arc::new(factory));
        service
            .open_workspace(workspace.clone(), mock_descriptor())
            .unwrap();
        service.open_document(document.clone()).unwrap();
        service
            .change_document(
                &document.document,
                2,
                "fn main() { let unsaved = true; }".into(),
            )
            .unwrap();

        let hover = service
            .hover(
                &document.document,
                LspPosition {
                    line: 0,
                    character: 3,
                },
            )
            .unwrap();
        assert!(hover.inline.is_none());
        assert!(hover.details.is_some());
        let plan = service
            .rename_plan(
                &document.document,
                LspPosition {
                    line: 0,
                    character: 3,
                },
                "entry".into(),
                "session",
                "turn",
            )
            .unwrap();
        assert_eq!(plan.approval.side_effect, ToolSideEffect::WorkspaceWrite);
        assert_eq!(plan.approval.version, 2);
        assert!(service.tools().len() >= 11);

        let context = service
            .collect(ContextProviderRequest {
                session_id: "session".into(),
                turn_id: "turn".into(),
                provider_id: CONTEXT_PROVIDER_ID.into(),
                input: json!({"workspace": "workspace"}),
            })
            .unwrap();
        assert!(context.details.is_some());
        let status = service
            .call(
                serde_json::to_value(LspServiceRequest::WorkspaceStatus {
                    workspace: workspace.clone(),
                })
                .unwrap(),
            )
            .unwrap();
        assert_eq!(status["type"], "workspace_status");

        service.restart(&workspace).unwrap();
        let status = service.workspace_status(&workspace).unwrap();
        assert_eq!(status.restart_count, 1);
        assert_eq!(status.open_documents, 1);
        let workspaces = service.list_workspaces();
        assert_eq!(workspaces, vec![status]);
        let did_open_texts = sent
            .lock()
            .unwrap()
            .iter()
            .filter(|message| {
                message.get("method").and_then(Value::as_str) == Some("textDocument/didOpen")
            })
            .filter_map(|message| {
                message
                    .pointer("/params/textDocument/text")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>();
        assert_eq!(did_open_texts.len(), 2);
        assert_eq!(did_open_texts[1], "fn main() { let unsaved = true; }");
        service.dispose().unwrap();
        assert_eq!(service.active_workspace_count(), 0);
    }

    #[test]
    fn request_timeout_cancel_and_crash_are_typed_and_leak_free() {
        let mut factory = MockFactory::new();
        factory.stalled.insert("textDocument/hover".into());
        let alive = factory.alive.clone();
        let sent = factory.sent.clone();
        let workspace = LspWorkspaceId("workspace".into());
        let document = mock_document(&workspace);
        let service = SharedLspService::new(Arc::new(factory));
        service
            .open_workspace(workspace.clone(), mock_descriptor())
            .unwrap();
        service.open_document(document.clone()).unwrap();

        let timeout = service
            .query_with_control(
                &document.document,
                LspPosition {
                    line: 0,
                    character: 1,
                },
                "textDocument/hover",
                Value::Null,
                &LspRequestControl {
                    timeout: Duration::from_millis(5),
                    cancellation: LspCancellation::default(),
                },
            )
            .unwrap_err();
        assert_eq!(timeout.code, "agent.lsp.timeout");

        let cancellation = LspCancellation::default();
        cancellation.cancel();
        let cancelled = service
            .query_with_control(
                &document.document,
                LspPosition {
                    line: 0,
                    character: 1,
                },
                "textDocument/hover",
                Value::Null,
                &LspRequestControl {
                    timeout: Duration::from_secs(1),
                    cancellation,
                },
            )
            .unwrap_err();
        assert_eq!(cancelled.code, "agent.lsp.cancelled");
        assert!(sent.lock().unwrap().iter().any(|message| {
            message.get("method").and_then(Value::as_str) == Some("$/cancelRequest")
        }));

        alive.store(false, Ordering::Release);
        assert_eq!(
            service.workspace_status(&workspace).unwrap().state,
            LspWorkspaceState::Failed
        );
        alive.store(true, Ordering::Release);
        service.restart(&workspace).unwrap();
        service.dispose().unwrap();
        assert_eq!(service.active_workspace_count(), 0);
    }

    #[test]
    fn rust_analyzer_full_surface_diagnostics_restart_and_unsaved_version_e2e() {
        let root = tempdir().unwrap();
        fs::write(
            root.path().join("Cargo.toml"),
            "[package]\nname=\"lsp-fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
        )
        .unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        let source = "fn target() -> u32 { 1 }\nfn main() { let _ = target(); }\n";
        let path = root.path().join("src/main.rs");
        fs::write(&path, source).unwrap();
        let canonical_root = fs::canonicalize(root.path()).unwrap();
        let canonical_path = fs::canonicalize(&path).unwrap();
        let workspace = LspWorkspaceId("fixture".into());
        let document = LspDocumentId {
            workspace: workspace.clone(),
            uri: file_uri(&canonical_path),
        };
        let service = SharedLspService::new(Arc::new(StdioLspProcessFactory));
        service
            .open_workspace(
                workspace.clone(),
                LspServerDescriptor {
                    server_id: "rust-analyzer".into(),
                    command: "rust-analyzer".into(),
                    args: Vec::new(),
                    workspace_uri: file_uri(&canonical_root),
                    initialization_options: Some(json!({"cargo": {"sysroot": null}})),
                },
            )
            .unwrap();
        service
            .open_document(LspDocumentSnapshot {
                document: document.clone(),
                language_id: "rust".into(),
                version: 1,
                text: source.into(),
            })
            .unwrap();
        let position = LspPosition {
            line: 1,
            character: 22,
        };
        let mut definition = None;
        for _ in 0..100 {
            let candidate = service.definition(&document, position).unwrap();
            if candidate
                .inline
                .as_ref()
                .and_then(Value::as_array)
                .is_some_and(|locations| !locations.is_empty())
            {
                definition = Some(candidate);
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            definition.is_some(),
            "rust-analyzer did not index the fixture"
        );
        let references = service.references(&document, position).unwrap();
        assert!(!references.inline.unwrap().as_array().unwrap().is_empty());
        assert!(service.hover(&document, position).unwrap().inline.is_some());
        service.declaration(&document, position).unwrap();
        service.implementation(&document, position).unwrap();
        let symbols = service.document_symbols(&document).unwrap();
        assert!(
            symbols
                .inline
                .as_ref()
                .and_then(Value::as_array)
                .is_some_and(|symbols| !symbols.is_empty())
        );
        service
            .workspace_symbols(&workspace, "target".into())
            .unwrap();
        let rename = service
            .rename_plan(
                &document,
                LspPosition {
                    line: 0,
                    character: 5,
                },
                "renamed".into(),
                "session",
                "turn",
            )
            .unwrap();
        assert_eq!(rename.approval.side_effect, ToolSideEffect::WorkspaceWrite);
        let rename = rename.edit.inline.unwrap();
        assert!(rename.get("changes").is_some() || rename.get("documentChanges").is_some());
        service
            .change_document(
                &document,
                2,
                "fn renamed() -> u32 { 1 }\nfn main() { let _ = renamed(); }\n".into(),
            )
            .unwrap();
        service.restart(&workspace).unwrap();
        assert_eq!(
            service.workspace_status(&workspace).unwrap().restart_count,
            1
        );
        let mut restarted_definition = None;
        for _ in 0..100 {
            let candidate = service
                .definition(
                    &document,
                    LspPosition {
                        line: 1,
                        character: 22,
                    },
                )
                .unwrap();
            if candidate
                .inline
                .as_ref()
                .and_then(Value::as_array)
                .is_some_and(|locations| !locations.is_empty())
            {
                restarted_definition = Some(candidate);
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            restarted_definition
                .as_ref()
                .and_then(|result| result.inline.as_ref())
                .and_then(Value::as_array)
                .is_some_and(|locations| !locations.is_empty()),
            "rust-analyzer did not re-index the replayed unsaved document"
        );
        assert_eq!(
            service
                .change_document(&document, 2, source.into())
                .unwrap_err()
                .code,
            "agent.lsp.document_version_conflict"
        );
        service
            .change_document(
                &document,
                3,
                "fn renamed() -> u32 { 1 }\nfn main() { let _: bool = renamed(); }\n".into(),
            )
            .unwrap();
        let mut diagnostics = Vec::new();
        for _ in 0..100 {
            diagnostics = service.diagnostics(&document).unwrap();
            if !diagnostics.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !diagnostics.is_empty(),
            "rust-analyzer did not publish diagnostics for the unsaved type error"
        );
        let code_actions = service
            .code_actions_plan(
                &document,
                LspRange {
                    start: LspPosition {
                        line: 1,
                        character: 12,
                    },
                    end: LspPosition {
                        line: 1,
                        character: 36,
                    },
                },
                json!({"diagnostics": diagnostics}),
                "session",
                "turn",
            )
            .unwrap();
        assert_eq!(
            code_actions.approval.side_effect,
            ToolSideEffect::WorkspaceWrite
        );
        service
            .formatting(&document, json!({"tabSize": 4, "insertSpaces": true}))
            .unwrap();
        service.save_document(&document, None).unwrap();
        service.close_document(&document).unwrap();
        assert_eq!(
            service.workspace_status(&workspace).unwrap().open_documents,
            0
        );
        service.close_workspace(&workspace).unwrap();
    }
}
