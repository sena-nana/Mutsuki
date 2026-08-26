// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::cast_possible_truncation,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value,
    clippy::return_self_not_must_use,
    clippy::too_many_lines,
    clippy::uninlined_format_args,
    clippy::unnecessary_wraps
)]

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mutsuki_agent_contracts::{
    AgentError, AgentPluginStateKind, AgentServiceDescriptor, AgentToolDescriptor,
    AgentWorkspaceRef, ArtifactRef, BrowserNavigateRequest, BrowserSnapshotResult,
    ComputerUseActionPlan, ComputerUseRisk, ComputerUseServiceRequest, ComputerUseServiceResponse,
    ContextProviderRequest, ContextProviderResult, FsEntry, FsPatchRequest, FsReadResult,
    GrepMatch, PermissionRequest, ProcessExecRequest, ProcessExecResult, ResourceRef,
    ToolSideEffect, WorkspacePathRequest,
};
use mutsuki_agent_plugin_api::{AgentPluginRegistrar, AgentService, ContextProvider, ToolProvider};
use mutsuki_agent_runtime::AgentResourceStore;
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.computer-use";
pub const SERVICE_ID: &str = mutsuki_agent_contracts::AGENT_COMPUTER_USE_PROTOCOL;
pub const CONTEXT_PROVIDER_ID: &str = "mutsuki.agent.context.computer-use";
pub const INLINE_LIMIT: usize = 2_048;
pub const SUMMARY_LIMIT: usize = 512;

pub trait FilesystemGateway: Send + Sync {
    fn list(&self, root: &Path, relative: &Path) -> Result<Vec<FsEntry>, AgentError>;
    fn read(
        &self,
        root: &Path,
        relative: &Path,
        max_bytes: u64,
    ) -> Result<(Vec<u8>, bool), AgentError>;
    fn stat(&self, root: &Path, relative: &Path) -> Result<FsEntry, AgentError>;
    fn write(
        &self,
        root: &Path,
        relative: &Path,
        content: &[u8],
        create: bool,
        overwrite: bool,
    ) -> Result<(), AgentError>;
    fn delete(&self, root: &Path, relative: &Path) -> Result<(), AgentError>;
    fn rename(&self, root: &Path, from: &Path, to: &Path) -> Result<(), AgentError>;
    fn glob(&self, root: &Path, pattern: &str) -> Result<Vec<String>, AgentError>;
    fn grep(
        &self,
        root: &Path,
        pattern: &str,
        relative: Option<&Path>,
    ) -> Result<Vec<GrepMatch>, AgentError>;
}

pub trait ProcessGateway: Send + Sync {
    fn exec(
        &self,
        handle_id: &str,
        request: &ProcessExecRequest,
    ) -> Result<ProcessExecResult, AgentError>;
    fn cancel(&self, handle_id: &str) -> Result<(), AgentError>;
}

pub trait BrowserGateway: Send + Sync {
    fn snapshot(
        &self,
        request: &BrowserNavigateRequest,
    ) -> Result<(String, String, Vec<u8>), AgentError>;
    fn cancel(&self, handle_id: &str) -> Result<(), AgentError>;
}

pub fn resolve_workspace_path(
    workspace: &AgentWorkspaceRef,
    relative: &str,
) -> Result<PathBuf, AgentError> {
    if relative.trim().is_empty() {
        return Ok(PathBuf::new());
    }
    let candidate = Path::new(relative);
    if candidate.is_absolute() {
        return Err(AgentError::new(
            "agent.computer_use.path_escape",
            "workspace paths must be relative",
        ));
    }
    let mut normalized = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(AgentError::new(
                        "agent.computer_use.path_escape",
                        format!(
                            "path `{relative}` escapes workspace {}",
                            workspace.workspace_id
                        ),
                    ));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(AgentError::new(
                    "agent.computer_use.path_escape",
                    "workspace paths must be relative",
                ));
            }
        }
    }
    Ok(normalized)
}

pub fn workspace_root(workspace: &AgentWorkspaceRef) -> Result<PathBuf, AgentError> {
    if workspace.workspace_id.trim().is_empty() || workspace.root.trim().is_empty() {
        return Err(AgentError::invalid_input(
            "workspace_id and root are required",
        ));
    }
    Ok(PathBuf::from(&workspace.root))
}

#[derive(Default)]
pub struct InMemoryFilesystemBackend {
    files: Mutex<BTreeMap<String, Vec<u8>>>,
}

impl InMemoryFilesystemBackend {
    pub fn with_file(self, path: impl Into<String>, content: impl Into<Vec<u8>>) -> Self {
        self.files
            .lock()
            .expect("fs mutex")
            .insert(path.into(), content.into());
        self
    }

    fn key(relative: &Path) -> String {
        relative.to_string_lossy().replace('\\', "/")
    }
}

impl FilesystemGateway for InMemoryFilesystemBackend {
    fn list(&self, _root: &Path, relative: &Path) -> Result<Vec<FsEntry>, AgentError> {
        let prefix = {
            let key = Self::key(relative);
            if key.is_empty() {
                String::new()
            } else {
                format!("{key}/")
            }
        };
        let files = self.files.lock().expect("fs mutex");
        let mut entries = BTreeMap::new();
        for path in files.keys() {
            if !prefix.is_empty() && !path.starts_with(&prefix) {
                continue;
            }
            let rest = if prefix.is_empty() {
                path.as_str()
            } else {
                &path[prefix.len()..]
            };
            let name = rest.split('/').next().unwrap_or(rest);
            if name.is_empty() {
                continue;
            }
            let entry_path = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{}{name}", prefix)
            };
            let kind = if rest.contains('/') { "dir" } else { "file" };
            entries.insert(
                entry_path.clone(),
                FsEntry {
                    path: entry_path,
                    kind: kind.into(),
                    size: files.get(path).map(|bytes| bytes.len() as u64),
                },
            );
        }
        Ok(entries.into_values().collect())
    }

    fn read(
        &self,
        _root: &Path,
        relative: &Path,
        max_bytes: u64,
    ) -> Result<(Vec<u8>, bool), AgentError> {
        let key = Self::key(relative);
        let mut bytes = self
            .files
            .lock()
            .expect("fs mutex")
            .get(&key)
            .cloned()
            .ok_or_else(|| AgentError::not_found(format!("file `{key}` was not found")))?;
        let truncated = bytes.len() as u64 > max_bytes;
        if truncated {
            bytes.truncate(max_bytes as usize);
        }
        Ok((bytes, truncated))
    }

    fn stat(&self, _root: &Path, relative: &Path) -> Result<FsEntry, AgentError> {
        let key = Self::key(relative);
        let files = self.files.lock().expect("fs mutex");
        let bytes = files
            .get(&key)
            .ok_or_else(|| AgentError::not_found(format!("file `{key}` was not found")))?;
        Ok(FsEntry {
            path: key,
            kind: "file".into(),
            size: Some(bytes.len() as u64),
        })
    }

    fn write(
        &self,
        _root: &Path,
        relative: &Path,
        content: &[u8],
        create: bool,
        overwrite: bool,
    ) -> Result<(), AgentError> {
        let key = Self::key(relative);
        let mut files = self.files.lock().expect("fs mutex");
        let exists = files.contains_key(&key);
        if exists && !overwrite {
            return Err(AgentError::invalid_input(format!(
                "file `{key}` already exists"
            )));
        }
        if !exists && !create {
            return Err(AgentError::not_found(format!("file `{key}` was not found")));
        }
        files.insert(key, content.to_vec());
        Ok(())
    }

    fn delete(&self, _root: &Path, relative: &Path) -> Result<(), AgentError> {
        let key = Self::key(relative);
        if self.files.lock().expect("fs mutex").remove(&key).is_none() {
            return Err(AgentError::not_found(format!("file `{key}` was not found")));
        }
        Ok(())
    }

    fn rename(&self, _root: &Path, from: &Path, to: &Path) -> Result<(), AgentError> {
        let from_key = Self::key(from);
        let to_key = Self::key(to);
        let mut files = self.files.lock().expect("fs mutex");
        let content = files
            .remove(&from_key)
            .ok_or_else(|| AgentError::not_found(format!("file `{from_key}` was not found")))?;
        files.insert(to_key, content);
        Ok(())
    }

    fn glob(&self, _root: &Path, pattern: &str) -> Result<Vec<String>, AgentError> {
        let files = self.files.lock().expect("fs mutex");
        Ok(files
            .keys()
            .filter(|path| glob_match(pattern, path))
            .cloned()
            .collect())
    }

    fn grep(
        &self,
        _root: &Path,
        pattern: &str,
        relative: Option<&Path>,
    ) -> Result<Vec<GrepMatch>, AgentError> {
        let files = self.files.lock().expect("fs mutex");
        let mut matches = Vec::new();
        for (path, bytes) in files.iter() {
            if let Some(relative) = relative {
                let key = InMemoryFilesystemBackend::key(relative);
                if path != &key && !path.starts_with(&format!("{key}/")) {
                    continue;
                }
            }
            let text = String::from_utf8_lossy(bytes);
            for (index, line) in text.lines().enumerate() {
                if line.contains(pattern) {
                    matches.push(GrepMatch {
                        path: path.clone(),
                        line: (index as u64) + 1,
                        text: line.to_string(),
                    });
                }
            }
        }
        Ok(matches)
    }
}

fn glob_match(pattern: &str, path: &str) -> bool {
    if pattern == "*" || pattern == "**/*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("**/") {
        return path.ends_with(suffix) || path.contains(&format!("/{suffix}"));
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return path.ends_with(suffix);
    }
    path == pattern
}

pub struct WorkspaceFilesystemBackend;

impl FilesystemGateway for WorkspaceFilesystemBackend {
    fn list(&self, root: &Path, relative: &Path) -> Result<Vec<FsEntry>, AgentError> {
        let path = root.join(relative);
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&path).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let meta = entry.metadata().map_err(io_error)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if relative.as_os_str().is_empty() {
                name
            } else {
                format!("{}/{}", relative.to_string_lossy(), name)
            };
            entries.push(FsEntry {
                path: rel.replace('\\', "/"),
                kind: if meta.is_dir() {
                    "dir".into()
                } else {
                    "file".into()
                },
                size: meta.is_file().then_some(meta.len()),
            });
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(entries)
    }

    fn read(
        &self,
        root: &Path,
        relative: &Path,
        max_bytes: u64,
    ) -> Result<(Vec<u8>, bool), AgentError> {
        let bytes = std::fs::read(root.join(relative)).map_err(io_error)?;
        let truncated = bytes.len() as u64 > max_bytes;
        Ok((
            if truncated {
                bytes[..max_bytes as usize].to_vec()
            } else {
                bytes
            },
            truncated,
        ))
    }

    fn stat(&self, root: &Path, relative: &Path) -> Result<FsEntry, AgentError> {
        let meta = std::fs::metadata(root.join(relative)).map_err(io_error)?;
        Ok(FsEntry {
            path: relative.to_string_lossy().replace('\\', "/"),
            kind: if meta.is_dir() {
                "dir".into()
            } else {
                "file".into()
            },
            size: meta.is_file().then_some(meta.len()),
        })
    }

    fn write(
        &self,
        root: &Path,
        relative: &Path,
        content: &[u8],
        create: bool,
        overwrite: bool,
    ) -> Result<(), AgentError> {
        let path = root.join(relative);
        let exists = path.exists();
        if exists && !overwrite {
            return Err(AgentError::invalid_input("file already exists"));
        }
        if !exists && !create {
            return Err(AgentError::not_found("file was not found"));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        std::fs::write(path, content).map_err(io_error)
    }

    fn delete(&self, root: &Path, relative: &Path) -> Result<(), AgentError> {
        let path = root.join(relative);
        if path.is_dir() {
            std::fs::remove_dir_all(path).map_err(io_error)
        } else {
            std::fs::remove_file(path).map_err(io_error)
        }
    }

    fn rename(&self, root: &Path, from: &Path, to: &Path) -> Result<(), AgentError> {
        std::fs::rename(root.join(from), root.join(to)).map_err(io_error)
    }

    fn glob(&self, root: &Path, pattern: &str) -> Result<Vec<String>, AgentError> {
        let mut paths = Vec::new();
        walk(root, root, &mut paths)?;
        Ok(paths
            .into_iter()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .filter(|path| glob_match(pattern, path))
            .collect())
    }

    fn grep(
        &self,
        root: &Path,
        pattern: &str,
        relative: Option<&Path>,
    ) -> Result<Vec<GrepMatch>, AgentError> {
        let mut paths = Vec::new();
        let start = relative
            .map(|path| root.join(path))
            .unwrap_or_else(|| root.to_path_buf());
        if start.is_file() {
            paths.push(start.strip_prefix(root).unwrap_or(&start).to_path_buf());
        } else {
            walk(root, &start, &mut paths)?;
        }
        let mut matches = Vec::new();
        for path in paths {
            let bytes = std::fs::read(root.join(&path)).map_err(io_error)?;
            let text = String::from_utf8_lossy(&bytes);
            for (index, line) in text.lines().enumerate() {
                if line.contains(pattern) {
                    matches.push(GrepMatch {
                        path: path.to_string_lossy().replace('\\', "/"),
                        line: (index as u64) + 1,
                        text: line.to_string(),
                    });
                }
            }
        }
        Ok(matches)
    }
}

fn walk(root: &Path, current: &Path, out: &mut Vec<PathBuf>) -> Result<(), AgentError> {
    for entry in std::fs::read_dir(current).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, out)?;
        } else {
            out.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
        }
    }
    Ok(())
}

fn io_error(error: std::io::Error) -> AgentError {
    AgentError::new("agent.computer_use.io", error.to_string())
}

#[derive(Default)]
pub struct FakeProcessBackend {
    pub calls: Mutex<Vec<ProcessExecRequest>>,
    pub cancelled: Mutex<Vec<String>>,
    pub response: Mutex<Option<ProcessExecResult>>,
}

impl ProcessGateway for FakeProcessBackend {
    fn exec(
        &self,
        _handle_id: &str,
        request: &ProcessExecRequest,
    ) -> Result<ProcessExecResult, AgentError> {
        self.calls
            .lock()
            .expect("process mutex")
            .push(request.clone());
        if let Some(response) = self.response.lock().expect("process mutex").clone() {
            return Ok(response);
        }
        Ok(ProcessExecResult {
            exit_code: 0,
            summary: format!("executed {} {}", request.command, request.args.join(" ")),
            stdout_ref: None,
            stderr_ref: None,
            truncated: false,
            cancelled: false,
        })
    }

    fn cancel(&self, handle_id: &str) -> Result<(), AgentError> {
        self.cancelled
            .lock()
            .expect("process mutex")
            .push(handle_id.into());
        Ok(())
    }
}

#[derive(Default)]
pub struct FakeBrowserBackend {
    pub calls: Mutex<Vec<String>>,
    pub cancelled: Mutex<Vec<String>>,
}

impl BrowserGateway for FakeBrowserBackend {
    fn snapshot(
        &self,
        request: &BrowserNavigateRequest,
    ) -> Result<(String, String, Vec<u8>), AgentError> {
        self.calls
            .lock()
            .expect("browser mutex")
            .push(request.url.clone());
        Ok((
            request.url.clone(),
            "Example".into(),
            format!(
                "<html><title>Example</title><body>{}</body></html>",
                request.url
            )
            .into_bytes(),
        ))
    }

    fn cancel(&self, handle_id: &str) -> Result<(), AgentError> {
        self.cancelled
            .lock()
            .expect("browser mutex")
            .push(handle_id.into());
        Ok(())
    }
}

pub struct SharedComputerUseService {
    descriptor: AgentServiceDescriptor,
    fs: Arc<dyn FilesystemGateway>,
    process: Option<Arc<dyn ProcessGateway>>,
    browser: Option<Arc<dyn BrowserGateway>>,
    resources: AgentResourceStore,
    next_action: AtomicU64,
    active_handles: Mutex<BTreeMap<String, ComputerUseRisk>>,
}

impl SharedComputerUseService {
    pub fn new(
        fs: Arc<dyn FilesystemGateway>,
        process: Option<Arc<dyn ProcessGateway>>,
        browser: Option<Arc<dyn BrowserGateway>>,
        resources: AgentResourceStore,
    ) -> Self {
        Self {
            descriptor: AgentServiceDescriptor {
                service_id: SERVICE_ID.into(),
                version: "1".into(),
                request_schema: "mutsuki.agent.computer_use.request@1".into(),
                response_schema: "mutsuki.agent.computer_use.response@1".into(),
                state: AgentPluginStateKind::Stateful,
                affinity: Some("workspace".into()),
            },
            fs,
            process,
            browser,
            resources,
            next_action: AtomicU64::new(1),
            active_handles: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn plugin_descriptor(
        generation: u64,
    ) -> Result<mutsuki_agent_contracts::AgentKitPluginDescriptor, AgentError> {
        let mut registrar = AgentPluginRegistrar::new(PLUGIN_ID, generation)
            .service(AgentServiceDescriptor {
                service_id: SERVICE_ID.into(),
                version: "1".into(),
                request_schema: "mutsuki.agent.computer_use.request@1".into(),
                response_schema: "mutsuki.agent.computer_use.response@1".into(),
                state: AgentPluginStateKind::Stateful,
                affinity: Some("workspace".into()),
            })
            .context_provider(CONTEXT_PROVIDER_ID)
            .require_capability("workspace.fs")
            .require_service(SERVICE_ID);
        for (name, side_effect, approval) in [
            ("computer.fs.list", ToolSideEffect::WorkspaceRead, false),
            ("computer.fs.read", ToolSideEffect::WorkspaceRead, false),
            ("computer.fs.stat", ToolSideEffect::WorkspaceRead, false),
            ("computer.fs.write", ToolSideEffect::WorkspaceWrite, false),
            ("computer.fs.delete", ToolSideEffect::WorkspaceWrite, true),
            ("computer.fs.patch", ToolSideEffect::WorkspaceWrite, false),
            ("computer.fs.glob", ToolSideEffect::WorkspaceRead, false),
            ("computer.fs.grep", ToolSideEffect::WorkspaceRead, false),
            ("computer.shell.exec", ToolSideEffect::ExternalWrite, true),
            (
                "computer.browser.snapshot",
                ToolSideEffect::ExternalRead,
                true,
            ),
        ] {
            let mut tool = AgentToolDescriptor::new(
                name,
                format!("mutsuki.agent.tool.{name}@1"),
                format!("Run {name}"),
            );
            tool.side_effect = side_effect;
            tool.requires_approval = approval;
            registrar = registrar.tool(tool);
        }
        registrar.build()
    }

    pub fn active_handle_count(&self) -> usize {
        self.active_handles.lock().expect("handles mutex").len()
    }

    fn plan(
        &self,
        operation: &str,
        risk: ComputerUseRisk,
        summary: impl Into<String>,
        session_id: &str,
        turn_id: &str,
        version: u64,
        preview: Option<Value>,
    ) -> ComputerUseActionPlan {
        let action_id = format!(
            "computer-use-{}",
            self.next_action.fetch_add(1, Ordering::Relaxed)
        );
        ComputerUseActionPlan {
            action_id: action_id.clone(),
            operation: operation.into(),
            risk,
            summary: summary.into(),
            approval: PermissionRequest {
                session_id: session_id.into(),
                turn_id: turn_id.into(),
                action_id,
                tool: operation.into(),
                side_effect: risk.side_effect(),
                summary: format!("{operation} requires approval"),
                version,
            },
            preview,
        }
    }

    fn ensure_approved(
        &self,
        risk: ComputerUseRisk,
        approved: bool,
        operation: &str,
        session_id: &str,
        turn_id: &str,
        version: u64,
        preview: Option<Value>,
    ) -> Result<Option<ComputerUseActionPlan>, AgentError> {
        if !risk.requires_approval() || approved {
            return Ok(None);
        }
        Ok(Some(self.plan(
            operation,
            risk,
            format!("{operation} is waiting for approval"),
            session_id,
            turn_id,
            version,
            preview,
        )))
    }

    fn store_text(
        &self,
        kind: &str,
        schema: &str,
        text: &str,
    ) -> Result<Option<ResourceRef>, AgentError> {
        if text.len() <= INLINE_LIMIT {
            return Ok(None);
        }
        self.resources
            .put_json(SERVICE_ID, kind, schema, 1, &json!({"text": text}))
            .map(Some)
    }

    fn read(
        &self,
        request: &WorkspacePathRequest,
        max_bytes: u64,
    ) -> Result<FsReadResult, AgentError> {
        let root = workspace_root(&request.workspace)?;
        let relative = resolve_workspace_path(&request.workspace, &request.path)?;
        let (bytes, truncated) = self.fs.read(&root, &relative, max_bytes)?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let content_ref = self.store_text(
            "mutsuki.agent.computer_use.file",
            "mutsuki.agent.computer_use.file@1",
            &text,
        )?;
        Ok(FsReadResult {
            path: relative.to_string_lossy().replace('\\', "/"),
            summary: truncate(&text, SUMMARY_LIMIT),
            content_ref,
            truncated,
        })
    }

    fn patch(&self, request: &FsPatchRequest) -> Result<(), AgentError> {
        let root = workspace_root(&request.workspace)?;
        let relative = resolve_workspace_path(&request.workspace, &request.path)?;
        let (bytes, _) = self.fs.read(&root, &relative, u64::MAX)?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        if !text.contains(&request.old_text) {
            return Err(AgentError::invalid_input(
                "patch old_text was not found in the target file",
            ));
        }
        let updated = text.replacen(&request.old_text, &request.new_text, 1);
        self.fs
            .write(&root, &relative, updated.as_bytes(), false, true)
    }

    fn exec(&self, request: ProcessExecRequest) -> Result<ProcessExecResult, AgentError> {
        let process = self.process.as_ref().ok_or_else(|| {
            AgentError::provider_unavailable(
                "process backend is not injected; AgentKit does not own a shell runtime",
            )
        })?;
        let handle = format!(
            "process-{}",
            self.next_action.fetch_add(1, Ordering::Relaxed)
        );
        self.active_handles
            .lock()
            .expect("handles mutex")
            .insert(handle.clone(), ComputerUseRisk::ProcessExec);
        let mut result = process.exec(&handle, &request)?;
        if result.summary.len() > INLINE_LIMIT {
            result.stdout_ref = self.store_text(
                "mutsuki.agent.computer_use.stdout",
                "mutsuki.agent.computer_use.stdout@1",
                &result.summary,
            )?;
            result.summary = truncate(&result.summary, SUMMARY_LIMIT);
            result.truncated = true;
        }
        self.active_handles
            .lock()
            .expect("handles mutex")
            .remove(&handle);
        Ok(result)
    }

    fn browser_snapshot(
        &self,
        request: BrowserNavigateRequest,
    ) -> Result<BrowserSnapshotResult, AgentError> {
        let browser = self.browser.as_ref().ok_or_else(|| {
            AgentError::provider_unavailable(
                "browser backend is not injected; AgentKit does not own Chromium",
            )
        })?;
        let handle = format!(
            "browser-{}",
            self.next_action.fetch_add(1, Ordering::Relaxed)
        );
        self.active_handles
            .lock()
            .expect("handles mutex")
            .insert(handle.clone(), ComputerUseRisk::Network);
        let (url, title, bytes) = browser.snapshot(&request)?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let snapshot_ref = self.store_text(
            "mutsuki.agent.computer_use.browser",
            "mutsuki.agent.computer_use.browser@1",
            &text,
        )?;
        let artifact = Some(ArtifactRef {
            artifact_id: handle.clone(),
            media_type: "text/html".into(),
            summary: truncate(&text, SUMMARY_LIMIT),
            content_ref: snapshot_ref.clone(),
            kind: Some("browser_snapshot".into()),
            size_bytes: Some(text.len() as u64),
            content_hash: None,
            provenance: Some("computer_use.browser".into()),
            open_hint: None,
            action_hint: None,
        });
        self.active_handles
            .lock()
            .expect("handles mutex")
            .remove(&handle);
        Ok(BrowserSnapshotResult {
            url,
            title,
            summary: truncate(&text, SUMMARY_LIMIT),
            snapshot_ref,
            artifact,
        })
    }

    pub fn cancel(&self, handle_id: &str) -> Result<(), AgentError> {
        let risk = self
            .active_handles
            .lock()
            .expect("handles mutex")
            .remove(handle_id);
        match risk {
            Some(ComputerUseRisk::ProcessExec) => {
                if let Some(process) = &self.process {
                    process.cancel(handle_id)?;
                }
            }
            Some(ComputerUseRisk::Network) => {
                if let Some(browser) = &self.browser {
                    browser.cancel(handle_id)?;
                }
            }
            Some(_) | None => {}
        }
        Ok(())
    }

    pub fn call_value(&self, request: Value) -> Result<Value, AgentError> {
        AgentService::call(self, request)
    }
}

fn truncate(input: &str, limit: usize) -> String {
    if input.chars().count() <= limit {
        return input.to_string();
    }
    input.chars().take(limit).collect::<String>() + "…"
}

impl AgentService for SharedComputerUseService {
    fn descriptor(&self) -> &AgentServiceDescriptor {
        &self.descriptor
    }

    fn call(&self, request: Value) -> Result<Value, AgentError> {
        let request: ComputerUseServiceRequest = serde_json::from_value(request)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let response = match request {
            ComputerUseServiceRequest::List { request } => {
                let root = workspace_root(&request.workspace)?;
                let relative = resolve_workspace_path(&request.workspace, &request.path)?;
                ComputerUseServiceResponse::Entries {
                    entries: self.fs.list(&root, &relative)?,
                }
            }
            ComputerUseServiceRequest::Read { request, max_bytes } => {
                ComputerUseServiceResponse::Read(Box::new(self.read(&request, max_bytes)?))
            }
            ComputerUseServiceRequest::Stat { request } => {
                let root = workspace_root(&request.workspace)?;
                let relative = resolve_workspace_path(&request.workspace, &request.path)?;
                ComputerUseServiceResponse::Stat(self.fs.stat(&root, &relative)?)
            }
            ComputerUseServiceRequest::Write {
                workspace,
                path,
                content,
                create,
                overwrite,
                session_id,
                turn_id,
                approval_version,
                approved,
            } => {
                let risk = if overwrite {
                    ComputerUseRisk::DestructiveWrite
                } else {
                    ComputerUseRisk::ReversibleWrite
                };
                if let Some(plan) = self.ensure_approved(
                    risk,
                    approved,
                    "computer.fs.write",
                    &session_id,
                    &turn_id,
                    approval_version.unwrap_or(1),
                    Some(json!({"path": path, "bytes": content.len()})),
                )? {
                    ComputerUseServiceResponse::Plan(Box::new(plan))
                } else {
                    let root = workspace_root(&workspace)?;
                    let relative = resolve_workspace_path(&workspace, &path)?;
                    self.fs
                        .write(&root, &relative, content.as_bytes(), create, overwrite)?;
                    ComputerUseServiceResponse::Written { path }
                }
            }
            ComputerUseServiceRequest::Delete {
                request,
                session_id,
                turn_id,
                approval_version,
                approved,
            } => {
                if let Some(plan) = self.ensure_approved(
                    ComputerUseRisk::DestructiveWrite,
                    approved,
                    "computer.fs.delete",
                    &session_id,
                    &turn_id,
                    approval_version.unwrap_or(1),
                    Some(json!({"path": request.path})),
                )? {
                    ComputerUseServiceResponse::Plan(Box::new(plan))
                } else {
                    let root = workspace_root(&request.workspace)?;
                    let relative = resolve_workspace_path(&request.workspace, &request.path)?;
                    self.fs.delete(&root, &relative)?;
                    ComputerUseServiceResponse::Deleted { path: request.path }
                }
            }
            ComputerUseServiceRequest::Move {
                workspace,
                from,
                to,
                session_id,
                turn_id,
                approval_version,
                approved,
            } => {
                if let Some(plan) = self.ensure_approved(
                    ComputerUseRisk::DestructiveWrite,
                    approved,
                    "computer.fs.move",
                    &session_id,
                    &turn_id,
                    approval_version.unwrap_or(1),
                    Some(json!({"from": from, "to": to})),
                )? {
                    ComputerUseServiceResponse::Plan(Box::new(plan))
                } else {
                    let root = workspace_root(&workspace)?;
                    let from_path = resolve_workspace_path(&workspace, &from)?;
                    let to_path = resolve_workspace_path(&workspace, &to)?;
                    self.fs.rename(&root, &from_path, &to_path)?;
                    ComputerUseServiceResponse::Moved { from, to }
                }
            }
            ComputerUseServiceRequest::Patch {
                request,
                session_id,
                turn_id,
                approval_version,
                approved,
            } => {
                if let Some(plan) = self.ensure_approved(
                    ComputerUseRisk::ReversibleWrite,
                    approved,
                    "computer.fs.patch",
                    &session_id,
                    &turn_id,
                    approval_version.unwrap_or(1),
                    Some(json!({"path": request.path})),
                )? {
                    ComputerUseServiceResponse::Plan(Box::new(plan))
                } else {
                    let path = request.path.clone();
                    self.patch(&request)?;
                    ComputerUseServiceResponse::Patched { path }
                }
            }
            ComputerUseServiceRequest::Glob { workspace, pattern } => {
                let root = workspace_root(&workspace)?;
                ComputerUseServiceResponse::Paths {
                    paths: self.fs.glob(&root, &pattern)?,
                }
            }
            ComputerUseServiceRequest::Grep {
                workspace,
                pattern,
                path,
            } => {
                let root = workspace_root(&workspace)?;
                let relative = path
                    .as_deref()
                    .map(|path| resolve_workspace_path(&workspace, path))
                    .transpose()?;
                ComputerUseServiceResponse::Grep {
                    matches: self.fs.grep(&root, &pattern, relative.as_deref())?,
                }
            }
            ComputerUseServiceRequest::Exec {
                request,
                session_id,
                turn_id,
                approval_version,
                approved,
            } => {
                if let Some(plan) = self.ensure_approved(
                    ComputerUseRisk::ProcessExec,
                    approved,
                    "computer.shell.exec",
                    &session_id,
                    &turn_id,
                    approval_version.unwrap_or(1),
                    Some(json!({"command": request.command, "args": request.args})),
                )? {
                    ComputerUseServiceResponse::Plan(Box::new(plan))
                } else {
                    ComputerUseServiceResponse::Exec(Box::new(self.exec(request)?))
                }
            }
            ComputerUseServiceRequest::BrowserSnapshot {
                request,
                session_id,
                turn_id,
                approval_version,
                approved,
            } => {
                if let Some(plan) = self.ensure_approved(
                    ComputerUseRisk::Network,
                    approved,
                    "computer.browser.snapshot",
                    &session_id,
                    &turn_id,
                    approval_version.unwrap_or(1),
                    Some(json!({"url": request.url})),
                )? {
                    ComputerUseServiceResponse::Plan(Box::new(plan))
                } else {
                    ComputerUseServiceResponse::Browser(Box::new(self.browser_snapshot(request)?))
                }
            }
            ComputerUseServiceRequest::Plan {
                operation,
                risk,
                summary,
                session_id,
                turn_id,
                version,
                preview,
            } => ComputerUseServiceResponse::Plan(Box::new(self.plan(
                &operation,
                risk,
                summary,
                &session_id,
                &turn_id,
                version,
                preview,
            ))),
            ComputerUseServiceRequest::Cancel { handle_id } => {
                self.cancel(&handle_id)?;
                ComputerUseServiceResponse::Ack
            }
        };
        serde_json::to_value(response).map_err(|error| AgentError::invalid_input(error.to_string()))
    }

    fn drain(&self) -> Result<(), AgentError> {
        Ok(())
    }

    fn dispose(&self) -> Result<(), AgentError> {
        let handles = self
            .active_handles
            .lock()
            .expect("handles mutex")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for handle in handles {
            self.cancel(&handle)?;
        }
        Ok(())
    }
}

impl ToolProvider for SharedComputerUseService {
    fn tools(&self) -> Vec<AgentToolDescriptor> {
        SharedComputerUseService::plugin_descriptor(1)
            .map(|descriptor| descriptor.tools)
            .unwrap_or_default()
    }
}

impl ContextProvider for SharedComputerUseService {
    fn provider_id(&self) -> &str {
        CONTEXT_PROVIDER_ID
    }

    fn collect(
        &self,
        request: ContextProviderRequest,
    ) -> Result<ContextProviderResult, AgentError> {
        let summary = format!(
            "computer-use ready process={} browser={} active_handles={}",
            self.process.is_some(),
            self.browser.is_some(),
            self.active_handle_count()
        );
        let details = self.resources.put_json(
            SERVICE_ID,
            "mutsuki.agent.computer_use.context",
            "mutsuki.agent.computer_use.context@1",
            1,
            &json!({
                "process_backend": self.process.is_some(),
                "browser_backend": self.browser.is_some(),
                "active_handles": self.active_handle_count(),
            }),
        )?;
        Ok(ContextProviderResult {
            provider_id: request.provider_id,
            summary,
            details: Some(details),
            estimated_tokens: 24,
            estimated_bytes: 96,
            priority: 0,
            required: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_agent_contracts::ExecutionLimits;
    use tempfile::tempdir;

    fn workspace(root: &Path) -> AgentWorkspaceRef {
        AgentWorkspaceRef {
            workspace_id: "ws".into(),
            root: root.to_string_lossy().into_owned(),
        }
    }

    #[test]
    fn path_normalization_blocks_escape() {
        let ws = AgentWorkspaceRef {
            workspace_id: "ws".into(),
            root: "/tmp/ws".into(),
        };
        assert_eq!(
            resolve_workspace_path(&ws, "../secret").unwrap_err().code,
            "agent.computer_use.path_escape"
        );
        assert_eq!(
            resolve_workspace_path(&ws, "/etc/passwd").unwrap_err().code,
            "agent.computer_use.path_escape"
        );
        assert_eq!(
            resolve_workspace_path(&ws, "src/../readme.md")
                .unwrap()
                .to_string_lossy(),
            "readme.md"
        );
    }

    #[test]
    fn fake_and_workspace_backends_cover_golden_paths_with_approval() {
        let resources = AgentResourceStore::default();
        let fs = Arc::new(
            InMemoryFilesystemBackend::default()
                .with_file("src/main.rs", b"fn main() { println!(\"hi\"); }")
                .with_file(
                    "README.md",
                    format!("title\n{}", "x".repeat(INLINE_LIMIT + 8)).into_bytes(),
                ),
        );
        let process = Arc::new(FakeProcessBackend::default());
        let browser = Arc::new(FakeBrowserBackend::default());
        let service = SharedComputerUseService::new(
            fs,
            Some(process.clone()),
            Some(browser.clone()),
            resources,
        );
        let ws = AgentWorkspaceRef {
            workspace_id: "ws".into(),
            root: "/virtual".into(),
        };

        let read = service
            .read(
                &WorkspacePathRequest {
                    workspace: ws.clone(),
                    path: "README.md".into(),
                },
                64 * 1024,
            )
            .unwrap();
        assert!(read.content_ref.is_some());

        let grep = service
            .call_value(json!({
                "op": "grep",
                "workspace": ws,
                "pattern": "println"
            }))
            .unwrap();
        assert_eq!(grep["kind"], "grep");
        assert_eq!(grep["matches"][0]["path"], "src/main.rs");

        let denied = service
            .call_value(json!({
                "op": "delete",
                "request": {"workspace": ws, "path": "README.md"},
                "session_id": "s1",
                "turn_id": "t1",
                "approval_version": 3,
                "approved": false
            }))
            .unwrap();
        assert_eq!(denied["kind"], "plan");
        assert_eq!(denied["approval"]["version"], 3);

        let patched = service
            .call_value(json!({
                "op": "patch",
                "request": {
                    "workspace": ws,
                    "path": "src/main.rs",
                    "old_text": "hi",
                    "new_text": "hello"
                },
                "session_id": "s1",
                "turn_id": "t1",
                "approved": true
            }))
            .unwrap();
        assert_eq!(patched["kind"], "patched");

        let exec_plan = service
            .call_value(json!({
                "op": "exec",
                "request": {
                    "workspace": ws,
                    "command": "echo",
                    "args": ["ok"],
                    "limits": ExecutionLimits::default(),
                    "allow_network": false
                },
                "session_id": "s1",
                "turn_id": "t1",
                "approved": false
            }))
            .unwrap();
        assert_eq!(exec_plan["kind"], "plan");
        assert!(process.calls.lock().unwrap().is_empty());

        let exec = service
            .call_value(json!({
                "op": "exec",
                "request": {
                    "workspace": ws,
                    "command": "echo",
                    "args": ["ok"],
                    "limits": ExecutionLimits::default(),
                    "allow_network": false
                },
                "session_id": "s1",
                "turn_id": "t1",
                "approved": true
            }))
            .unwrap();
        assert_eq!(exec["kind"], "exec");
        assert_eq!(process.calls.lock().unwrap().len(), 1);

        let browser_snap = service
            .call_value(json!({
                "op": "browser_snapshot",
                "request": {"url": "https://example.com", "limits": ExecutionLimits::default()},
                "session_id": "s1",
                "turn_id": "t1",
                "approved": true
            }))
            .unwrap();
        assert_eq!(browser_snap["kind"], "browser");
        assert_eq!(browser.calls.lock().unwrap().len(), 1);
        assert_eq!(service.active_handle_count(), 0);

        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("note.txt"), "workspace backend").unwrap();
        let local = SharedComputerUseService::new(
            Arc::new(WorkspaceFilesystemBackend),
            None,
            None,
            AgentResourceStore::default(),
        );
        let local_ws = workspace(dir.path());
        let listed = local
            .call_value(json!({
                "op": "list",
                "request": {"workspace": local_ws, "path": ""}
            }))
            .unwrap();
        assert_eq!(listed["entries"][0]["path"], "note.txt");
        let descriptor = SharedComputerUseService::plugin_descriptor(2).unwrap();
        assert_eq!(descriptor.plugin_id, PLUGIN_ID);
        assert!(descriptor.tools.iter().any(|tool| tool.requires_approval));
    }

    #[test]
    fn performance_smoke_read_patch_shell_browser() {
        use std::time::Instant;
        let resources = AgentResourceStore::default();
        let fs = Arc::new(
            InMemoryFilesystemBackend::default()
                .with_file("src/lib.rs", b"pub fn answer() -> u32 { 42 }"),
        );
        let service = SharedComputerUseService::new(
            fs,
            Some(Arc::new(FakeProcessBackend::default())),
            Some(Arc::new(FakeBrowserBackend::default())),
            resources,
        );
        let ws = AgentWorkspaceRef {
            workspace_id: "ws".into(),
            root: "/virtual".into(),
        };
        let started = Instant::now();
        for index in 0..50 {
            service
                .call_value(json!({
                    "op": "read",
                    "request": {"workspace": ws, "path": "src/lib.rs"},
                    "max_bytes": 4096
                }))
                .unwrap();
            if index % 5 == 0 {
                service
                    .call_value(json!({
                        "op": "patch",
                        "request": {
                            "workspace": ws,
                            "path": "src/lib.rs",
                            "old_text": "42",
                            "new_text": "42"
                        },
                        "session_id": "s",
                        "turn_id": format!("t{index}"),
                        "approved": true
                    }))
                    .unwrap();
                service
                    .call_value(json!({
                        "op": "exec",
                        "request": {
                            "workspace": ws,
                            "command": "echo",
                            "args": ["ok"],
                            "limits": ExecutionLimits::default(),
                            "allow_network": false
                        },
                        "session_id": "s",
                        "turn_id": format!("t{index}"),
                        "approved": true
                    }))
                    .unwrap();
                service
                    .call_value(json!({
                        "op": "browser_snapshot",
                        "request": {"url": "https://example.com", "limits": ExecutionLimits::default()},
                        "session_id": "s",
                        "turn_id": format!("t{index}"),
                        "approved": true
                    }))
                    .unwrap();
            }
        }
        let elapsed = started.elapsed();
        assert_eq!(service.active_handle_count(), 0);
        assert!(
            elapsed.as_millis() < 5_000,
            "computer-use smoke too slow: {elapsed:?}"
        );
    }
}
