//! Native Coding Agent reference bundle.
//!
//! Assembles AgentKit Runtime services and coding plugins without Codex /
//! Claude official Agent Server processes. Products inject Host secrets,
//! editor context, and process backends; this crate only owns the assembly.

use std::collections::BTreeSet;
use std::sync::Arc;

use mutsuki_agent_contracts::{
    AGENT_RUN_PROTOCOL, AgentDelegationRequest, AgentError, AgentKitPluginDescriptor,
    AgentPluginStateKind, AgentResult, AgentRuntimeProfile, AgentServiceDescriptor,
    AgentToolDescriptor, AgentToolExecuteRequest, AgentToolExecution, AgentWorkspaceRef,
    BrowserNavigateRequest, CodeFileChange, CodeIndexBatch, CodeIndexServiceRequest,
    CodeSearchMode, CodeSearchQuery, CodeWorkspaceRef, ComputerUseServiceRequest, DelegationBudget,
    DelegationMode, DelegationScope, ExecutionLimits, FsPatchRequest, GitDiffRequest, GitDiffScope,
    GitRepositoryRef, GitServiceRequest, GitServiceResponse, GitWorktreeRef, InteractionKind,
    LspServiceRequest, LspWorkspaceId, ProcessExecRequest, SubAgentDescriptor, SubAgentOutcomeKind,
    ToolSideEffect, ToolTargetPayloadMode, WorkspacePathRequest,
};
use mutsuki_agent_plugin_api::{
    AgentPluginRegistrar, AgentService, ContextProvider, ToolProvider, context_collect_plugin,
};
use mutsuki_agent_plugin_code_index::{CodeIndexLspSignals, SharedCodeIndexService};
use mutsuki_agent_plugin_computer_use::{
    BrowserGateway, FakeBrowserBackend, FakeProcessBackend, FilesystemGateway,
    InMemoryFilesystemBackend, ProcessGateway, SharedComputerUseService,
};
use mutsuki_agent_plugin_git::{GitGateway, InMemoryGitBackend, SharedGitService};
use mutsuki_agent_plugin_lsp::{LspProcess, LspProcessFactory, SharedLspService};
use mutsuki_agent_plugin_mcp::{
    McpRequestControl, McpTransport, McpTransportFactory, SharedMcpService,
};
use mutsuki_agent_runtime::{
    AgentResourceStore, ChildAgentExecutor, RequiredChildExecutor, SubAgentOrchestrator,
    reference_coding_agent_test_profile,
};
use mutsuki_agent_sdk::{orchestration_runner, runtime_failure};
use mutsuki_runtime_sdk::contracts::{RunnerResult, Task};
use mutsuki_runtime_sdk::{
    PluginBuilder, ProtocolSpec, RuntimeClientRef, SdkProtocol, TaskAwaitRunnerAdapter,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{AgentPluginBundle, ContextBuilder};

pub const NATIVE_CODING_BUNDLE_ID: &str = "mutsuki.agent.bundle.native-coding";
pub const LSP_PLUGIN_ID: &str = "mutsuki.plugin.agent.lsp";
pub const NATIVE_CODING_TOOL_PLUGIN_ID: &str = "mutsuki.plugin.agent.native-coding-tools";
pub const NATIVE_CODING_TOOL_RUNNER_ID: &str = "mutsuki.agent.native-coding-tools.runner";
pub const NATIVE_CODING_TOOL_PROTOCOL: &str = "mutsuki.agent.native-coding.tool@1";

#[derive(Clone, Debug)]
pub struct NativeCodingToolProtocol;

impl SdkProtocol for NativeCodingToolProtocol {
    const PROTOCOL_ID: &'static str = NATIVE_CODING_TOOL_PROTOCOL;
}

impl ProtocolSpec for NativeCodingToolProtocol {}

/// Provider-neutral Host facts forwarded to a Native Coding tool target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeCodingRunContext {
    pub workspace: AgentWorkspaceRef,
    pub turn_id: String,
}

/// Product-provided facts and approval state for one Native Coding tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeCodingToolContext {
    pub session_id: String,
    pub turn_id: String,
    pub workspace: AgentWorkspaceRef,
    pub approval_version: u64,
    pub approved: bool,
    pub permission_mode: mutsuki_agent_contracts::AgentPermissionMode,
}

/// Host-neutral Native Coding Agent assembly used by conformance, LiliaCode
/// migration, and CLI smoke. Not a product Persona.
#[derive(Clone)]
pub struct NativeCodingAgentBundle {
    pub core: AgentPluginBundle,
    pub resources: AgentResourceStore,
    pub git: Arc<SharedGitService>,
    pub code_index: Arc<SharedCodeIndexService>,
    pub lsp: Arc<SharedLspService>,
    pub computer_use: Arc<SharedComputerUseService>,
    pub mcp: Arc<SharedMcpService>,
    pub subagents: SubAgentOrchestrator,
    pub profile: AgentRuntimeProfile,
}

/// Injected backends for reference / test assemblies.
pub struct NativeCodingBackends {
    pub git: Arc<dyn GitGateway>,
    pub filesystem: Arc<dyn FilesystemGateway>,
    pub process: Option<Arc<dyn ProcessGateway>>,
    pub browser: Option<Arc<dyn BrowserGateway>>,
    pub lsp: Arc<dyn LspProcessFactory>,
    pub mcp: Arc<dyn McpTransportFactory>,
    pub code_index_lsp: Arc<dyn CodeIndexLspSignals>,
}

impl Default for NativeCodingBackends {
    fn default() -> Self {
        Self {
            git: Arc::new(InMemoryGitBackend::default()),
            filesystem: Arc::new(InMemoryFilesystemBackend::default()),
            process: None,
            browser: None,
            lsp: Arc::new(UnavailableLspFactory),
            mcp: Arc::new(UnavailableMcpFactory),
            code_index_lsp: Arc::new(mutsuki_agent_plugin_code_index::UnavailableLspSignals),
        }
    }
}

impl NativeCodingBackends {
    /// Test helper. Production assemblies leave process/browser unset until the
    /// Host injects real backends; Native Coding does not register Fake by default.
    #[must_use]
    pub fn with_fake_process_and_browser(mut self) -> Self {
        self.process = Some(Arc::new(FakeProcessBackend::default()));
        self.browser = Some(Arc::new(FakeBrowserBackend::default()));
        self
    }
}

/// Process factory that fails loud until a Host injects a real LSP backend.
#[derive(Default)]
pub struct UnavailableLspFactory;

impl LspProcessFactory for UnavailableLspFactory {
    fn spawn(
        &self,
        _descriptor: &mutsuki_agent_contracts::LspServerDescriptor,
    ) -> Result<Box<dyn LspProcess>, AgentError> {
        Err(AgentError::provider_unavailable(
            "LSP process factory is not injected; Native Bundle does not spawn language servers by default",
        ))
    }
}

/// MCP transport factory that fails loud until a Host injects transports.
#[derive(Default)]
pub struct UnavailableMcpFactory;

impl McpTransportFactory for UnavailableMcpFactory {
    fn open(
        &self,
        _manifest: &mutsuki_agent_contracts::McpServerManifest,
    ) -> Result<Box<dyn McpTransport>, AgentError> {
        Err(AgentError::provider_unavailable(
            "MCP transport factory is not injected; Native Bundle does not start MCP servers by default",
        ))
    }
}

impl NativeCodingAgentBundle {
    /// Builds a reference Native Coding Bundle with shared coding services.
    pub fn reference(backends: NativeCodingBackends) -> Self {
        Self::reference_with_child_executor(backends, Arc::new(RequiredChildExecutor))
    }

    /// Test helper that injects a child executor. Production assemblies must use
    /// `RuntimeClientChildExecutor` or another TaskPool-backed implementation.
    pub fn reference_with_child_executor(
        backends: NativeCodingBackends,
        executor: Arc<dyn ChildAgentExecutor>,
    ) -> Self {
        let resources = AgentResourceStore::default();
        let git = Arc::new(SharedGitService::new(backends.git, resources.clone()));
        let code_index = Arc::new(SharedCodeIndexService::with_lsp(
            resources.clone(),
            backends.code_index_lsp,
        ));
        let lsp = Arc::new(SharedLspService::with_resources(
            backends.lsp,
            resources.clone(),
        ));
        let computer_use = Arc::new(SharedComputerUseService::new(
            backends.filesystem,
            backends.process,
            backends.browser,
            resources.clone(),
        ));
        let mcp = Arc::new(SharedMcpService::with_resources(
            backends.mcp,
            resources.clone(),
        ));
        let subagents = SubAgentOrchestrator::new(executor);
        let _ = subagents.register_agent(SubAgentDescriptor {
            agent_id: "review".into(),
            profile_id: "mutsuki.reference.coding-agent.review".into(),
            summary: "Optional review SubAgent".into(),
            tools: vec![
                "git.diff".into(),
                "code.search".into(),
                "lsp.diagnostics".into(),
            ],
            permissions: vec![
                "git.read".into(),
                "code.index.read".into(),
                "lsp.read".into(),
            ],
            max_depth: 1,
        });
        let profile = reference_coding_agent_test_profile();
        let core = AgentPluginBundle {
            context: ContextBuilder::with_resources(resources.clone()),
            ..AgentPluginBundle::default()
        };
        let bundle = Self {
            core,
            resources,
            git,
            code_index,
            lsp,
            computer_use,
            mcp,
            subagents,
            profile,
        };
        bundle
            .install_routed_tools()
            .expect("reference coding tools register on the ToolRegistry");
        bundle
    }

    /// Registers model-visible tools onto the core ToolRegistry and applies the
    /// profile allowlist. Call again after MCP connect so newly advertised tools
    /// enter `tool/list`.
    pub fn install_routed_tools(&self) -> AgentResult<()> {
        for tool in self.routed_model_tools() {
            self.core.tools.register(tool)?;
        }
        self.core.configure_profile(&self.profile)
    }

    /// Returns kit plugin descriptors for coding surfaces (generation pinned).
    pub fn coding_plugin_descriptors(
        &self,
        generation: u64,
    ) -> AgentResult<Vec<AgentKitPluginDescriptor>> {
        Ok(vec![
            SharedGitService::plugin_descriptor(generation)?,
            SharedCodeIndexService::plugin_descriptor(generation)?,
            SharedComputerUseService::plugin_descriptor(generation)?,
            SharedMcpService::plugin_descriptor(generation)?,
            lsp_plugin_descriptor(generation)?,
        ])
    }

    /// Hosts register these so `profile.context.provider_ids` hit live collect.
    pub fn context_collect_plugins(&self, generation: u64) -> AgentResult<Vec<PluginBuilder>> {
        [
            ("git", Arc::clone(&self.git) as Arc<dyn ContextProvider>),
            (
                "code-index",
                Arc::clone(&self.code_index) as Arc<dyn ContextProvider>,
            ),
            ("lsp", Arc::clone(&self.lsp) as Arc<dyn ContextProvider>),
            (
                "computer-use",
                Arc::clone(&self.computer_use) as Arc<dyn ContextProvider>,
            ),
            ("mcp", Arc::clone(&self.mcp) as Arc<dyn ContextProvider>),
        ]
        .into_iter()
        .map(|(name, provider)| {
            context_collect_plugin(
                format!("mutsuki.plugin.agent.context.collect.{name}"),
                generation,
                provider,
            )
        })
        .collect()
    }

    /// Tools that the Native Coding model may call directly.
    ///
    /// These descriptors deliberately expose only operations with a complete
    /// model-input → public Service request mapping. Dynamic MCP tools retain
    /// the schema advertised by their server catalog.
    pub fn model_tools(&self) -> Vec<AgentToolDescriptor> {
        let mut tools = vec![
            interaction_tool(
                "ask_user_question",
                "Ask the user for information required to continue the current turn",
                InteractionKind::Clarification,
                json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "prompt": {"type": "string"},
                        "question": {"type": "string"},
                        "questions": {"type": "array", "minItems": 1, "items": {"type": "object"}},
                        "options": {"type": "array", "items": {}}
                    },
                    "anyOf": [
                        {"required": ["prompt"]},
                        {"required": ["question"]},
                        {"required": ["questions"]}
                    ],
                    "additionalProperties": true
                }),
            ),
            interaction_tool(
                "confirm_plan",
                "Request approval or revision feedback for the current implementation plan",
                InteractionKind::PlanConfirm,
                json!({
                    "type": "object",
                    "required": ["plan"],
                    "properties": {
                        "plan": {"type": "string"},
                        "question": {"type": "string"},
                        "title": {"type": "string"}
                    },
                    "additionalProperties": false
                }),
            ),
            model_tool(
                "git.status",
                "Read the current Git branch and working tree status",
                ToolSideEffect::WorkspaceRead,
                false,
                json!({"type": "object", "properties": {}, "additionalProperties": false}),
            ),
            model_tool(
                "git.diff",
                "Read a working tree or staged Git diff",
                ToolSideEffect::WorkspaceRead,
                false,
                json!({
                    "type": "object",
                    "properties": {
                        "staged": {"type": "boolean"},
                        "paths": {"type": "array", "items": {"type": "string"}}
                    },
                    "additionalProperties": false
                }),
            ),
            model_tool(
                "git.log",
                "Read recent Git commits",
                ToolSideEffect::WorkspaceRead,
                false,
                json!({
                    "type": "object",
                    "properties": {
                        "limit": {"type": "integer", "minimum": 1, "maximum": 100},
                        "path": {"type": "string"}
                    },
                    "additionalProperties": false
                }),
            ),
            model_tool(
                "code.search",
                "Search the shared workspace code index",
                ToolSideEffect::WorkspaceRead,
                false,
                json!({
                    "type": "object",
                    "required": ["query"],
                    "properties": {
                        "query": {"type": "string"},
                        "mode": {"type": "string", "enum": ["text", "regex", "symbol", "semantic"]},
                        "path_prefix": {"type": "string"},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 64}
                    },
                    "additionalProperties": false
                }),
            ),
            model_tool(
                "code.project_summary",
                "Summarize the shared workspace code index",
                ToolSideEffect::WorkspaceRead,
                false,
                json!({"type": "object", "properties": {}, "additionalProperties": false}),
            ),
            model_tool(
                "computer.fs.list",
                "List files in the workspace",
                ToolSideEffect::WorkspaceRead,
                false,
                path_schema(false),
            ),
            model_tool(
                "computer.fs.read",
                "Read a workspace file",
                ToolSideEffect::WorkspaceRead,
                false,
                path_schema(true),
            ),
            model_tool(
                "computer.fs.write",
                "Create or replace a workspace file",
                ToolSideEffect::WorkspaceWrite,
                true,
                json!({
                    "type": "object",
                    "required": ["path", "content"],
                    "properties": {
                        "path": {"type": "string"},
                        "content": {"type": "string"},
                        "create": {"type": "boolean"},
                        "overwrite": {"type": "boolean"}
                    },
                    "additionalProperties": false
                }),
            ),
            model_tool(
                "computer.fs.patch",
                "Replace one exact text segment in a workspace file",
                ToolSideEffect::WorkspaceWrite,
                true,
                json!({
                    "type": "object",
                    "required": ["path", "old_text", "new_text"],
                    "properties": {
                        "path": {"type": "string"},
                        "old_text": {"type": "string"},
                        "new_text": {"type": "string"}
                    },
                    "additionalProperties": false
                }),
            ),
            model_tool(
                "computer.fs.glob",
                "Find workspace files by glob",
                ToolSideEffect::WorkspaceRead,
                false,
                json!({
                    "type": "object",
                    "required": ["pattern"],
                    "properties": {"pattern": {"type": "string"}},
                    "additionalProperties": false
                }),
            ),
            model_tool(
                "computer.fs.grep",
                "Find text in workspace files",
                ToolSideEffect::WorkspaceRead,
                false,
                json!({
                    "type": "object",
                    "required": ["pattern"],
                    "properties": {
                        "pattern": {"type": "string"},
                        "path": {"type": "string"}
                    },
                    "additionalProperties": false
                }),
            ),
            model_tool(
                "computer.shell.exec",
                "Run an approved process in the workspace",
                ToolSideEffect::ExternalWrite,
                true,
                json!({
                    "type": "object",
                    "required": ["command"],
                    "properties": {
                        "command": {"type": "string"},
                        "args": {"type": "array", "items": {"type": "string"}},
                        "stdin": {"type": "string"},
                        "timeout_ms": {"type": "integer", "minimum": 1, "maximum": 300000},
                        "max_output_bytes": {"type": "integer", "minimum": 1, "maximum": 1048576},
                        "allow_network": {"type": "boolean"}
                    },
                    "additionalProperties": false
                }),
            ),
            model_tool(
                "computer.browser.snapshot",
                "Fetch an approved web page snapshot",
                ToolSideEffect::ExternalRead,
                true,
                json!({
                    "type": "object",
                    "required": ["url"],
                    "properties": {"url": {"type": "string", "format": "uri"}},
                    "additionalProperties": false
                }),
            ),
        ];
        if self.lsp.active_workspace_count() > 0 {
            tools.push(model_tool(
                "lsp.workspace_symbols",
                "Search symbols in the active language server workspace",
                ToolSideEffect::WorkspaceRead,
                false,
                json!({
                    "type": "object",
                    "required": ["query"],
                    "properties": {"query": {"type": "string"}},
                    "additionalProperties": false
                }),
            ));
        }
        tools.extend(self.mcp.tools());
        tools
    }

    /// Tool descriptors routed through the declared Native Coding effect Runner.
    pub fn routed_model_tools(&self) -> Vec<AgentToolDescriptor> {
        self.model_tools()
            .into_iter()
            .map(|mut descriptor| {
                if matches!(&descriptor.execution, AgentToolExecution::Routed) {
                    descriptor.target_protocol_id = NATIVE_CODING_TOOL_PROTOCOL.into();
                    descriptor.target_payload_mode = ToolTargetPayloadMode::ExecutionRequest;
                }
                descriptor
            })
            .collect()
    }

    /// Execute a model-selected tool through the public shared Service surface.
    pub fn invoke_model_tool(
        &self,
        name: &str,
        input: Value,
        context: &NativeCodingToolContext,
    ) -> AgentResult<Value> {
        let descriptor = self
            .model_tools()
            .into_iter()
            .find(|tool| tool.name == name)
            .ok_or_else(|| {
                AgentError::not_found(format!("Native Coding tool `{name}` is not registered"))
            })?;
        if descriptor.requires_approval && !context.approved {
            return Err(AgentError::new(
                "agent.permission.denied",
                format!("Native Coding tool `{name}` requires approval"),
            ));
        }
        if context.permission_mode == mutsuki_agent_contracts::AgentPermissionMode::ReadOnly
            && descriptor.side_effect.is_write()
        {
            return Err(AgentError::new(
                "agent.permission.read_only",
                format!("Native Coding tool `{name}` is blocked by read-only policy"),
            ));
        }
        match name {
            "git.status" => {
                let worktree = self.discover_worktree(&context.workspace.root)?;
                self.git
                    .call_value(agent_value(GitServiceRequest::Status { worktree })?)
            }
            "git.diff" => {
                let worktree = self.discover_worktree(&context.workspace.root)?;
                let scope = if bool_field(&input, "staged", false)? {
                    GitDiffScope::Staged
                } else {
                    GitDiffScope::WorkingTree
                };
                self.git.call_value(agent_value(GitServiceRequest::Diff {
                    request: GitDiffRequest {
                        worktree,
                        scope,
                        base: None,
                        head: None,
                        paths: string_array_field(&input, "paths")?,
                    },
                })?)
            }
            "git.log" => {
                let worktree = self.discover_worktree(&context.workspace.root)?;
                self.git.call_value(agent_value(GitServiceRequest::Log {
                    worktree,
                    limit: u64_field(&input, "limit", 20)?.clamp(1, 100) as u32,
                    path: optional_string_field(&input, "path")?,
                })?)
            }
            "code.search" => AgentService::call(
                self.code_index.as_ref(),
                agent_value(CodeIndexServiceRequest::Search {
                    query: CodeSearchQuery {
                        workspace: code_workspace(&context.workspace),
                        query: required_string_field(&input, "query")?,
                        mode: match optional_string_field(&input, "mode")?.as_deref() {
                            Some("regex") => CodeSearchMode::Regex,
                            Some("symbol") => CodeSearchMode::Symbol,
                            Some("semantic") => CodeSearchMode::Semantic,
                            Some("text") | None => CodeSearchMode::Text,
                            Some(other) => {
                                return Err(AgentError::invalid_input(format!(
                                    "unsupported code search mode `{other}`"
                                )));
                            }
                        },
                        path_prefix: optional_string_field(&input, "path_prefix")?,
                        limit: u64_field(&input, "limit", 32)?.clamp(1, 64) as u32,
                        include_overlay: true,
                    },
                })?,
            ),
            "code.project_summary" => AgentService::call(
                self.code_index.as_ref(),
                agent_value(CodeIndexServiceRequest::ProjectSummary {
                    workspace: code_workspace(&context.workspace),
                })?,
            ),
            "computer.fs.list" => {
                self.computer_use
                    .call_value(agent_value(ComputerUseServiceRequest::List {
                        request: workspace_path(&input, context, false)?,
                    })?)
            }
            "computer.fs.read" => {
                self.computer_use
                    .call_value(agent_value(ComputerUseServiceRequest::Read {
                        request: workspace_path(&input, context, true)?,
                        max_bytes: u64_field(&input, "max_bytes", 256 * 1024)?,
                    })?)
            }
            "computer.fs.write" => {
                self.computer_use
                    .call_value(agent_value(ComputerUseServiceRequest::Write {
                        workspace: context.workspace.clone(),
                        path: required_string_field(&input, "path")?,
                        content: required_string_field(&input, "content")?,
                        create: bool_field(&input, "create", true)?,
                        overwrite: bool_field(&input, "overwrite", false)?,
                        session_id: context.session_id.clone(),
                        turn_id: context.turn_id.clone(),
                        approval_version: Some(context.approval_version),
                        approved: true,
                    })?)
            }
            "computer.fs.patch" => {
                self.computer_use
                    .call_value(agent_value(ComputerUseServiceRequest::Patch {
                        request: FsPatchRequest {
                            workspace: context.workspace.clone(),
                            path: required_string_field(&input, "path")?,
                            old_text: required_string_field(&input, "old_text")?,
                            new_text: required_string_field(&input, "new_text")?,
                        },
                        session_id: context.session_id.clone(),
                        turn_id: context.turn_id.clone(),
                        approval_version: Some(context.approval_version),
                        approved: true,
                    })?)
            }
            "computer.fs.glob" => {
                self.computer_use
                    .call_value(agent_value(ComputerUseServiceRequest::Glob {
                        workspace: context.workspace.clone(),
                        pattern: required_string_field(&input, "pattern")?,
                    })?)
            }
            "computer.fs.grep" => {
                self.computer_use
                    .call_value(agent_value(ComputerUseServiceRequest::Grep {
                        workspace: context.workspace.clone(),
                        pattern: required_string_field(&input, "pattern")?,
                        path: optional_string_field(&input, "path")?,
                    })?)
            }
            "computer.shell.exec" => {
                self.computer_use
                    .call_value(agent_value(ComputerUseServiceRequest::Exec {
                        request: ProcessExecRequest {
                            workspace: context.workspace.clone(),
                            command: required_string_field(&input, "command")?,
                            args: string_array_field(&input, "args")?,
                            stdin: optional_string_field(&input, "stdin")?,
                            limits: ExecutionLimits {
                                timeout_ms: u64_field(&input, "timeout_ms", 30_000)?,
                                max_output_bytes: u64_field(
                                    &input,
                                    "max_output_bytes",
                                    256 * 1024,
                                )?,
                                max_concurrency: 1,
                            },
                            allow_network: bool_field(&input, "allow_network", false)?,
                        },
                        session_id: context.session_id.clone(),
                        turn_id: context.turn_id.clone(),
                        approval_version: Some(context.approval_version),
                        approved: true,
                    })?)
            }
            "computer.browser.snapshot" => self.computer_use.call_value(agent_value(
                ComputerUseServiceRequest::BrowserSnapshot {
                    request: BrowserNavigateRequest {
                        url: required_string_field(&input, "url")?,
                        limits: ExecutionLimits::default(),
                    },
                    session_id: context.session_id.clone(),
                    turn_id: context.turn_id.clone(),
                    approval_version: Some(context.approval_version),
                    approved: true,
                },
            )?),
            "lsp.workspace_symbols" => AgentService::call(
                self.lsp.as_ref(),
                agent_value(LspServiceRequest::WorkspaceSymbols {
                    workspace: LspWorkspaceId(context.workspace.workspace_id.clone()),
                    query: required_string_field(&input, "query")?,
                })?,
            ),
            dynamic if self.mcp.tools().iter().any(|tool| tool.name == dynamic) => agent_value(
                self.mcp
                    .call_tool(dynamic, input, &McpRequestControl::default())?,
            ),
            other => Err(AgentError::not_found(format!(
                "Native Coding tool `{other}` is not registered"
            ))),
        }
    }

    fn discover_worktree(&self, root: &str) -> AgentResult<GitWorktreeRef> {
        let value = self
            .git
            .call_value(agent_value(GitServiceRequest::Discover {
                path: root.to_string(),
            })?)?;
        match serde_json::from_value::<GitServiceResponse>(value)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?
        {
            GitServiceResponse::Discovered { worktree, .. } => Ok(worktree),
            _ => Err(AgentError::new(
                "agent.git.unexpected_response",
                "Git discovery returned an unexpected response",
            )),
        }
    }

    /// Asserts Git / Code Index / LSP / Computer Use / MCP share one resource store
    /// and that UI / Agent callers hold the same service instances.
    pub fn assert_shared_service_identity(&self) -> AgentResult<()> {
        let descriptors = self.coding_plugin_descriptors(1)?;
        let mut service_ids = BTreeSet::new();
        for descriptor in &descriptors {
            for service in &descriptor.services {
                if !service_ids.insert(service.service_id.as_str()) {
                    return Err(AgentError::invalid_input(format!(
                        "duplicate coding service `{}`",
                        service.service_id
                    )));
                }
            }
        }
        for expected in [
            mutsuki_agent_plugin_git::SERVICE_ID,
            mutsuki_agent_plugin_code_index::SERVICE_ID,
            mutsuki_agent_plugin_lsp::SERVICE_ID,
            mutsuki_agent_plugin_computer_use::SERVICE_ID,
            mutsuki_agent_plugin_mcp::SERVICE_ID,
        ] {
            if !service_ids.contains(expected) {
                return Err(AgentError::not_found(format!(
                    "native bundle missing shared service `{expected}`"
                )));
            }
        }
        Ok(())
    }

    /// Verifies the profile uses protocol adapters only (no Claude/Codex server ids).
    pub fn assert_no_official_agent_server_dependency(&self) -> AgentResult<()> {
        for adapter in &self.profile.adapters {
            let family = adapter.protocol_family.to_ascii_lowercase();
            let adapter_id = adapter.adapter_id.to_ascii_lowercase();
            if family.contains("claude")
                || family.contains("codex")
                || adapter_id.contains("claude")
                || adapter_id.contains("codex")
                || family.contains("agent-server")
            {
                return Err(AgentError::invalid_input(format!(
                    "native bundle must not bind official Agent Server adapter `{}`",
                    adapter.adapter_id
                )));
            }
        }
        if self.profile.providers.len() < 2 {
            return Err(AgentError::invalid_input(
                "native bundle profile requires at least two Provider instances on one Adapter",
            ));
        }
        let adapter_ids: BTreeSet<_> = self
            .profile
            .providers
            .iter()
            .map(|provider| provider.adapter_id.as_str())
            .collect();
        if adapter_ids.len() != 1 {
            return Err(AgentError::invalid_input(
                "native bundle providers must share one protocol Adapter id",
            ));
        }
        Ok(())
    }

    pub fn workspace_ref(&self) -> AgentWorkspaceRef {
        AgentWorkspaceRef {
            workspace_id: "native-workspace".into(),
            root: "/workspace".into(),
        }
    }

    pub fn code_workspace(&self) -> CodeWorkspaceRef {
        code_workspace(&self.workspace_ref())
    }
}

/// Build the effect Runner that turns AgentLoop tool requests into calls on the
/// bundle's shared Git/LSP/Index/ComputerUse/MCP Services.
pub fn native_coding_tool_plugin(
    client: RuntimeClientRef,
    bundle: NativeCodingAgentBundle,
) -> PluginBuilder {
    let descriptor =
        orchestration_runner(NATIVE_CODING_TOOL_RUNNER_ID, NATIVE_CODING_TOOL_PLUGIN_ID)
            .accepts::<NativeCodingToolProtocol>()
            .build();
    PluginBuilder::new(NATIVE_CODING_TOOL_PLUGIN_ID)
        .protocol::<NativeCodingToolProtocol>()
        .runner(Box::new(TaskAwaitRunnerAdapter::new(
            descriptor,
            client,
            Box::new(move |_ctx, task| {
                let bundle = bundle.clone();
                Box::pin(async move { run_native_coding_tool(bundle, task) })
            }),
        )))
}

fn run_native_coding_tool(
    bundle: NativeCodingAgentBundle,
    task: Task,
) -> mutsuki_runtime_sdk::RuntimeResult<RunnerResult> {
    let request: AgentToolExecuteRequest = serde_json::from_value(task.payload.clone().into())
        .map_err(|error| {
            runtime_failure(
                NATIVE_CODING_TOOL_PLUGIN_ID,
                &task.task_id,
                AgentError::invalid_input(error.to_string()),
            )
        })?;
    let context: NativeCodingRunContext = request
        .context
        .clone()
        .ok_or_else(|| {
            runtime_failure(
                NATIVE_CODING_TOOL_PLUGIN_ID,
                &task.task_id,
                AgentError::invalid_input("Native Coding tool context is required"),
            )
        })
        .and_then(|context| {
            serde_json::from_value(context).map_err(|error| {
                runtime_failure(
                    NATIVE_CODING_TOOL_PLUGIN_ID,
                    &task.task_id,
                    AgentError::invalid_input(error.to_string()),
                )
            })
        })?;
    let approval_version = request
        .approval
        .as_ref()
        .map(|approval| approval.request.version)
        .unwrap_or_default();
    let approved = request.approval.as_ref().is_none_or(|approval| {
        approval.decision.decision == mutsuki_agent_contracts::PermissionDecisionKind::Approved
    });
    let tool_context = NativeCodingToolContext {
        session_id: request.session_id.clone().ok_or_else(|| {
            runtime_failure(
                NATIVE_CODING_TOOL_PLUGIN_ID,
                &task.task_id,
                AgentError::invalid_input("Native Coding tool session_id is required"),
            )
        })?,
        turn_id: context.turn_id,
        workspace: context.workspace,
        approval_version,
        approved,
        permission_mode: request.permission_mode,
    };
    let output = bundle
        .invoke_model_tool(&request.name, request.input, &tool_context)
        .map_err(|error| runtime_failure(NATIVE_CODING_TOOL_PLUGIN_ID, &task.task_id, error))?;
    let mut result = RunnerResult::completed(task.task_id);
    result.output = Some(output);
    Ok(result)
}

fn model_tool(
    name: &str,
    description: &str,
    side_effect: ToolSideEffect,
    requires_approval: bool,
    input_schema: Value,
) -> AgentToolDescriptor {
    let mut descriptor =
        AgentToolDescriptor::new(name, format!("mutsuki.agent.tool.{name}@1"), description);
    descriptor.side_effect = side_effect;
    descriptor.requires_approval = requires_approval;
    descriptor.input_schema = input_schema;
    descriptor
}

fn interaction_tool(
    name: &str,
    description: &str,
    interaction_kind: InteractionKind,
    input_schema: Value,
) -> AgentToolDescriptor {
    let mut descriptor = AgentToolDescriptor::new(name, AGENT_RUN_PROTOCOL, description);
    descriptor.input_schema = input_schema;
    descriptor.execution = AgentToolExecution::Interaction { interaction_kind };
    descriptor
}

fn agent_value(value: impl serde::Serialize) -> AgentResult<Value> {
    serde_json::to_value(value).map_err(|error| AgentError::invalid_input(error.to_string()))
}

fn path_schema(required: bool) -> Value {
    json!({
        "type": "object",
        "required": if required { vec!["path"] } else { Vec::<&str>::new() },
        "properties": {
            "path": {"type": "string"},
            "max_bytes": {"type": "integer", "minimum": 1, "maximum": 1048576}
        },
        "additionalProperties": false
    })
}

fn input_object(input: &Value) -> AgentResult<&serde_json::Map<String, Value>> {
    input
        .as_object()
        .ok_or_else(|| AgentError::invalid_input("tool input must be a JSON object"))
}

fn required_string_field(input: &Value, name: &str) -> AgentResult<String> {
    optional_string_field(input, name)?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AgentError::invalid_input(format!("tool input requires `{name}`")))
}

fn optional_string_field(input: &Value, name: &str) -> AgentResult<Option<String>> {
    match input_object(input)?.get(name) {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(AgentError::invalid_input(format!(
            "tool input `{name}` must be a string"
        ))),
    }
}

fn string_array_field(input: &Value, name: &str) -> AgentResult<Vec<String>> {
    match input_object(input)?.get(name) {
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value.as_str().map(str::to_string).ok_or_else(|| {
                    AgentError::invalid_input(format!(
                        "tool input `{name}` must contain only strings"
                    ))
                })
            })
            .collect(),
        Some(Value::Null) | None => Ok(Vec::new()),
        Some(_) => Err(AgentError::invalid_input(format!(
            "tool input `{name}` must be an array"
        ))),
    }
}

fn bool_field(input: &Value, name: &str, default: bool) -> AgentResult<bool> {
    match input_object(input)?.get(name) {
        Some(Value::Bool(value)) => Ok(*value),
        Some(Value::Null) | None => Ok(default),
        Some(_) => Err(AgentError::invalid_input(format!(
            "tool input `{name}` must be a boolean"
        ))),
    }
}

fn u64_field(input: &Value, name: &str, default: u64) -> AgentResult<u64> {
    match input_object(input)?.get(name) {
        Some(Value::Number(value)) => value.as_u64().ok_or_else(|| {
            AgentError::invalid_input(format!("tool input `{name}` must be a positive integer"))
        }),
        Some(Value::Null) | None => Ok(default),
        Some(_) => Err(AgentError::invalid_input(format!(
            "tool input `{name}` must be an integer"
        ))),
    }
}

fn workspace_path(
    input: &Value,
    context: &NativeCodingToolContext,
    required: bool,
) -> AgentResult<WorkspacePathRequest> {
    let path = if required {
        required_string_field(input, "path")?
    } else {
        optional_string_field(input, "path")?.unwrap_or_default()
    };
    Ok(WorkspacePathRequest {
        workspace: context.workspace.clone(),
        path,
    })
}

fn code_workspace(workspace: &AgentWorkspaceRef) -> CodeWorkspaceRef {
    CodeWorkspaceRef {
        workspace_id: workspace.workspace_id.clone(),
        root: workspace.root.clone(),
        tenant_id: String::new(),
        git_revision: None,
        worktree_id: None,
    }
}

fn lsp_plugin_descriptor(generation: u64) -> AgentResult<AgentKitPluginDescriptor> {
    let mut registrar = AgentPluginRegistrar::new(LSP_PLUGIN_ID, generation)
        .service(AgentServiceDescriptor {
            service_id: mutsuki_agent_plugin_lsp::SERVICE_ID.into(),
            version: "1".into(),
            request_schema: "mutsuki.agent.lsp.request@1".into(),
            response_schema: "mutsuki.agent.lsp.response@1".into(),
            state: AgentPluginStateKind::Stateful,
            affinity: Some("workspace".into()),
        })
        .context_provider(mutsuki_agent_plugin_lsp::CONTEXT_PROVIDER_ID)
        .require_service(mutsuki_agent_plugin_lsp::SERVICE_ID);
    for (name, side_effect) in [
        ("lsp.definition", ToolSideEffect::WorkspaceRead),
        ("lsp.hover", ToolSideEffect::WorkspaceRead),
        ("lsp.diagnostics", ToolSideEffect::WorkspaceRead),
        ("lsp.document_symbols", ToolSideEffect::WorkspaceRead),
        ("lsp.rename_plan", ToolSideEffect::WorkspaceWrite),
    ] {
        let mut tool = AgentToolDescriptor::new(
            name,
            format!("mutsuki.agent.tool.{name}@1"),
            format!("Run {name}"),
        );
        tool.side_effect = side_effect;
        registrar = registrar.tool(tool);
    }
    registrar.build()
}

/// Seed a small broken workspace for fix E2E (no network / no official clients).
pub fn seed_fix_fixture(bundle: &NativeCodingAgentBundle) -> AgentResult<GitWorktreeRef> {
    let ws = bundle.workspace_ref();
    bundle.computer_use.call_value(json!({
        "op": "write",
        "workspace": ws,
        "path": "src/lib.rs",
        "content": "pub fn answer() -> u32 { 41 }\n",
        "create": true,
        "overwrite": true,
        "session_id": "seed",
        "turn_id": "t0",
        "approved": true
    }))?;
    bundle.computer_use.call_value(json!({
        "op": "write",
        "workspace": ws,
        "path": "tests/answer.rs",
        "content": "#[test]\nfn answer_is_42() { assert_eq!(fixture::answer(), 42); }\n",
        "create": true,
        "overwrite": true,
        "session_id": "seed",
        "turn_id": "t0",
        "approved": true
    }))?;

    let code_ws = bundle.code_workspace();
    bundle
        .code_index
        .open_workspace(code_ws.clone(), None, None, false)?;
    bundle.code_index.apply_batch(CodeIndexBatch {
        workspace: code_ws,
        changes: vec![
            CodeFileChange::Create {
                path: "src/lib.rs".into(),
                content: "pub fn answer() -> u32 { 41 }\n".into(),
            },
            CodeFileChange::Create {
                path: "tests/answer.rs".into(),
                content: "#[test]\nfn answer_is_42() { assert_eq!(fixture::answer(), 42); }\n"
                    .into(),
            },
        ],
        rebuild: false,
    })?;

    match bundle
        .git
        .call_value(json!({ "op": "discover", "path": "/workspace" }))
    {
        Ok(value) => Ok(serde_json::from_value(value["worktree"].clone())
            .map_err(|error| AgentError::invalid_input(error.to_string()))?),
        Err(_) => Ok(GitWorktreeRef {
            worktree_id: "native-wt".into(),
            path: "/workspace".into(),
            repository: GitRepositoryRef {
                repo_id: "native".into(),
                root: "/workspace".into(),
            },
        }),
    }
}

/// Runs the fix golden path against shared services.
pub fn run_fix_golden_path(bundle: &NativeCodingAgentBundle) -> AgentResult<Value> {
    let _worktree = seed_fix_fixture(bundle)?;
    let ws = bundle.workspace_ref();
    let code_ws = bundle.code_workspace();

    let search = bundle.code_index.search(CodeSearchQuery {
        workspace: code_ws.clone(),
        query: "answer".into(),
        mode: CodeSearchMode::Symbol,
        path_prefix: None,
        limit: 8,
        include_overlay: false,
    })?;
    if search.hits.is_empty() {
        return Err(AgentError::not_found("fix fixture symbol `answer` missing"));
    }

    let patched = bundle.computer_use.call_value(json!({
        "op": "patch",
        "request": {
            "workspace": ws,
            "path": "src/lib.rs",
            "old_text": "41",
            "new_text": "42"
        },
        "session_id": "fix",
        "turn_id": "fix-turn",
        "approved": true
    }))?;
    if patched["kind"] != "patched" {
        return Err(AgentError::invalid_input(format!(
            "expected patched file change, got {patched}"
        )));
    }

    bundle.code_index.apply_batch(CodeIndexBatch {
        workspace: code_ws,
        changes: vec![CodeFileChange::Change {
            path: "src/lib.rs".into(),
            content: "pub fn answer() -> u32 { 42 }\n".into(),
        }],
        rebuild: false,
    })?;

    let test_result = bundle.computer_use.call_value(json!({
        "op": "exec",
        "request": {
            "workspace": ws,
            "command": "test",
            "args": ["answer"],
            "limits": mutsuki_agent_contracts::ExecutionLimits::default(),
            "allow_network": false
        },
        "session_id": "fix",
        "turn_id": "fix-turn",
        "approved": true
    }))?;

    let read = bundle.computer_use.call_value(json!({
        "op": "read",
        "request": {"workspace": ws, "path": "src/lib.rs"},
        "max_bytes": 4096
    }))?;

    Ok(json!({
        "search_hits": search.hits.len(),
        "patched": patched["kind"],
        "test": test_result["kind"],
        "content_has_42": read.to_string().contains("42"),
        "official_servers": 0,
    }))
}

/// Structured code-review golden path using index + optional SubAgent.
pub fn run_review_golden_path(bundle: &NativeCodingAgentBundle) -> AgentResult<Value> {
    let _ = seed_fix_fixture(bundle)?;
    let code_ws = bundle.code_workspace();
    let search = bundle.code_index.search(CodeSearchQuery {
        workspace: code_ws,
        query: "answer".into(),
        mode: CodeSearchMode::Text,
        path_prefix: Some("src/".into()),
        limit: 8,
        include_overlay: false,
    })?;
    let findings: Vec<Value> = search
        .hits
        .iter()
        .map(|hit| {
            json!({
                "path": hit.path,
                "summary": hit.summary,
                "location": hit.range,
            })
        })
        .collect();

    bundle.subagents.bind_parent(
        "review-parent",
        DelegationScope {
            allowed_tools: vec!["code.search".into(), "git.diff".into()],
            allowed_permissions: vec!["code.index.read".into(), "git.read".into()],
            inherit_conversation: false,
            inherit_knowledge: false,
        },
        DelegationBudget {
            max_context_tokens: None,
            max_total_tokens: Some(8_000),
            max_cost_microunits: Some(1_000),
            deadline_unix_ms: None,
            max_child_steps: Some(4),
        },
    );
    let child = bundle.subagents.invoke_as_tool(AgentDelegationRequest {
        parent_session_id: "review-parent".into(),
        parent_turn_id: "review-turn".into(),
        child_agent_id: "review".into(),
        child_profile_id: "mutsuki.reference.coding-agent.review".into(),
        mode: DelegationMode::AgentAsTool,
        budget: DelegationBudget::default(),
        scope: DelegationScope::default(),
        input: json!({"findings": findings.len()}),
        input_ref: None,
        depth: 0,
    })?;

    Ok(json!({
        "findings": findings,
        "child_status": match child.kind {
            SubAgentOutcomeKind::Succeeded => "succeeded",
            SubAgentOutcomeKind::Failed => "failed",
            SubAgentOutcomeKind::TimedOut => "timed_out",
            SubAgentOutcomeKind::Cancelled => "cancelled",
            SubAgentOutcomeKind::BudgetExceeded => "budget_exceeded",
        },
        "child_summary": child.summary,
        "artifact": {
            "kind": "code_review",
            "findings": findings.len(),
        }
    }))
}

/// Cancel / resume fencing: identical patch after success does not invent a second write.
pub fn run_resume_without_duplicate_side_effects(
    bundle: &NativeCodingAgentBundle,
) -> AgentResult<Value> {
    let ws = bundle.workspace_ref();
    let _ = bundle.computer_use.call_value(json!({
        "op": "write",
        "workspace": ws,
        "path": "src/lib.rs",
        "content": "pub fn answer() -> u32 { 41 }\n",
        "create": true,
        "overwrite": true,
        "session_id": "resume",
        "turn_id": "t0",
        "approved": true
    }))?;
    let first = bundle.computer_use.call_value(json!({
        "op": "patch",
        "request": {
            "workspace": ws,
            "path": "src/lib.rs",
            "old_text": "41",
            "new_text": "42"
        },
        "session_id": "resume",
        "turn_id": "t1",
        "approved": true
    }))?;
    bundle
        .subagents
        .restore_completed_attempt("resume-attempt-1");
    bundle.subagents.cancel_parent("resume-parent");

    let second = bundle.computer_use.call_value(json!({
        "op": "patch",
        "request": {
            "workspace": ws,
            "path": "src/lib.rs",
            "old_text": "41",
            "new_text": "42"
        },
        "session_id": "resume",
        "turn_id": "t1-retry",
        "approved": true
    }));
    let duplicate_suppressed = second.is_err();

    Ok(json!({
        "first_kind": first["kind"],
        "duplicate_suppressed": duplicate_suppressed,
        "cancelled_parent": true,
        "restored_attempt": "resume-attempt-1",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::time::Instant;

    use mutsuki_agent_contracts::{
        AgentRuntimeMode, EditorContextServiceRequest, EditorContextServiceResponse,
    };
    use mutsuki_agent_runtime::EchoChildExecutor;
    use mutsuki_agent_testkit::{
        CodingEventLog, FakeEditorContextService, emit_deterministic_coding_run,
    };

    fn seeded_bundle() -> NativeCodingAgentBundle {
        let mut files = BTreeMap::new();
        files.insert(
            "src/lib.rs".into(),
            "pub fn answer() -> u32 { 41 }\n".into(),
        );
        files.insert("README.md".into(), "native fixture\n".into());
        let git = Arc::new(InMemoryGitBackend::default().seed_repo("/workspace", files));
        let fs = Arc::new(
            InMemoryFilesystemBackend::default()
                .with_file("src/lib.rs", b"pub fn answer() -> u32 { 41 }\n")
                .with_file(
                    "tests/answer.rs",
                    b"#[test]\nfn answer_is_42() { assert_eq!(fixture::answer(), 42); }\n",
                ),
        );
        NativeCodingAgentBundle::reference_with_child_executor(
            NativeCodingBackends {
                git,
                filesystem: fs,
                ..Default::default()
            }
            .with_fake_process_and_browser(),
            Arc::new(EchoChildExecutor),
        )
    }

    /// LiliaCode migration surface exercised in-crate (product imports the bundle APIs).
    struct LiliaCodeNativeMigrationClient {
        bundle: NativeCodingAgentBundle,
        editor: FakeEditorContextService,
        events: CodingEventLog,
        task_id: String,
        session_id: String,
        pending_approvals: Vec<String>,
        artifacts: Vec<String>,
        timeline: Vec<String>,
        fork_of: Option<String>,
    }

    impl LiliaCodeNativeMigrationClient {
        fn bind(bundle: NativeCodingAgentBundle, task_id: impl Into<String>) -> Self {
            let task_id = task_id.into();
            let session_id = format!("session-{task_id}");
            let mut client = Self {
                editor: FakeEditorContextService::default(),
                events: CodingEventLog::new(session_id.clone()),
                bundle,
                task_id,
                session_id,
                pending_approvals: Vec::new(),
                artifacts: Vec::new(),
                timeline: Vec::new(),
                fork_of: None,
            };
            client.project_events();
            client
        }

        fn editor_snapshot(&self) -> AgentResult<mutsuki_agent_contracts::EditorContextSnapshot> {
            match self.editor.call(EditorContextServiceRequest::Snapshot {
                turn_id: Some(format!("turn-{}", self.task_id)),
            })? {
                EditorContextServiceResponse::Snapshot { snapshot } => Ok(snapshot),
                other => Err(AgentError::invalid_input(format!(
                    "unexpected editor response: {other:?}"
                ))),
            }
        }

        fn request_approval(&mut self, action_id: impl Into<String>) {
            let action_id = action_id.into();
            self.pending_approvals.push(action_id.clone());
            self.timeline
                .push(format!("interaction_requested:{action_id}"));
        }

        fn resolve_approval(&mut self, action_id: &str, approved: bool) -> AgentResult<()> {
            let idx = self
                .pending_approvals
                .iter()
                .position(|id| id == action_id)
                .ok_or_else(|| {
                    AgentError::not_found(format!("approval `{action_id}` not pending"))
                })?;
            self.pending_approvals.remove(idx);
            self.timeline.push(format!(
                "interaction_resolved:{action_id}:{}",
                if approved { "approved" } else { "rejected" }
            ));
            Ok(())
        }

        fn cancel(&mut self) {
            self.bundle.subagents.cancel_parent(&self.session_id);
            self.timeline.push("cancelled".into());
        }

        fn reset(&mut self) {
            self.pending_approvals.clear();
            self.artifacts.clear();
            self.timeline.push("reset".into());
        }

        fn regenerate(&mut self) {
            self.timeline.push("regenerate".into());
            emit_deterministic_coding_run(&self.events, "regenerated");
            self.project_events();
        }

        fn fork(&self, new_task_id: impl Into<String>) -> Self {
            let mut forked = Self::bind(self.bundle.clone(), new_task_id);
            forked.editor = self.editor.clone();
            forked.fork_of = Some(self.session_id.clone());
            forked
                .timeline
                .push(format!("forked_from:{}", self.session_id));
            forked
        }

        fn project_events(&mut self) {
            for envelope in self.events.events() {
                let kind = match &envelope.event {
                    mutsuki_agent_contracts::AgentEvent::PlanUpdated { .. } => "plan",
                    mutsuki_agent_contracts::AgentEvent::TodoUpdated { .. } => "todo",
                    mutsuki_agent_contracts::AgentEvent::CommandStarted { .. } => "command",
                    mutsuki_agent_contracts::AgentEvent::FileChangeApplied { .. } => "file_change",
                    mutsuki_agent_contracts::AgentEvent::ArtifactProduced { artifact, .. } => {
                        self.artifacts.push(artifact.artifact_id.clone());
                        "artifact"
                    }
                    mutsuki_agent_contracts::AgentEvent::FinalResponse { .. } => "final",
                    _ => "other",
                };
                self.timeline
                    .push(format!("event:{kind}:{}", envelope.sequence));
            }
        }
    }

    #[test]
    fn reference_bundle_composes_shared_services_and_dual_providers() {
        let bundle = seeded_bundle();
        bundle.assert_shared_service_identity().unwrap();
        bundle.assert_no_official_agent_server_dependency().unwrap();
        assert_eq!(bundle.profile.mode, AgentRuntimeMode::Test);
        assert!(
            bundle
                .profile
                .providers
                .iter()
                .all(|provider| provider.test_only)
        );
        assert_eq!(bundle.profile.providers.len(), 2);
        let descriptors = bundle.coding_plugin_descriptors(1).unwrap();
        assert!(descriptors.len() >= 5);
        assert_eq!(
            bundle.core.manifests().len(),
            AgentPluginBundle::default().manifests().len()
        );
        assert_eq!(bundle.context_collect_plugins(1).unwrap().len(), 5);
    }

    #[test]
    fn default_backends_do_not_register_fake_process_or_browser() {
        let backends = NativeCodingBackends::default();
        assert!(backends.process.is_none());
        assert!(backends.browser.is_none());
        let bundle = NativeCodingAgentBundle::reference(backends);
        let context = NativeCodingToolContext {
            session_id: "native-default".into(),
            turn_id: "turn-1".into(),
            workspace: bundle.workspace_ref(),
            approval_version: 1,
            approved: true,
            permission_mode: mutsuki_agent_contracts::AgentPermissionMode::Ask,
        };
        assert_eq!(
            bundle
                .invoke_model_tool(
                    "computer.shell.exec",
                    json!({"command": "echo", "args": ["hi"]}),
                    &context,
                )
                .unwrap_err()
                .code,
            "agent.provider_unavailable"
        );
    }

    #[test]
    fn native_model_tools_route_through_shared_services() {
        let bundle = seeded_bundle();
        seed_fix_fixture(&bundle).unwrap();
        let listed = bundle
            .core
            .tools
            .list(mutsuki_agent_contracts::AgentToolListRequest {
                profile_id: Some(bundle.profile.profile_id.clone()),
            });
        assert!(listed.tools.iter().any(|tool| tool.name == "git.status"
            && tool.target_protocol_id == NATIVE_CODING_TOOL_PROTOCOL));
        let tools = bundle.model_tools();
        for name in [
            "git.status",
            "code.search",
            "computer.fs.read",
            "computer.fs.write",
        ] {
            let tool = tools
                .iter()
                .find(|tool| tool.name == name)
                .unwrap_or_else(|| panic!("missing model tool {name}"));
            assert_eq!(tool.input_schema["type"], "object");
        }
        assert!(
            tools
                .iter()
                .find(|tool| tool.name == "computer.fs.write")
                .unwrap()
                .requires_approval
        );
        let routed = bundle.routed_model_tools();
        let ask = routed
            .iter()
            .find(|tool| tool.name == "ask_user_question")
            .expect("clarification interaction is model-visible");
        assert_eq!(ask.target_protocol_id, AGENT_RUN_PROTOCOL);
        assert!(matches!(
            &ask.execution,
            AgentToolExecution::Interaction {
                interaction_kind: InteractionKind::Clarification
            }
        ));
        let status = routed
            .iter()
            .find(|tool| tool.name == "git.status")
            .expect("routed coding tool remains model-visible");
        assert_eq!(status.target_protocol_id, NATIVE_CODING_TOOL_PROTOCOL);
        assert_eq!(
            status.target_payload_mode,
            ToolTargetPayloadMode::ExecutionRequest
        );

        let context = NativeCodingToolContext {
            session_id: "native-model".into(),
            turn_id: "turn-1".into(),
            workspace: bundle.workspace_ref(),
            approval_version: 1,
            approved: true,
            permission_mode: mutsuki_agent_contracts::AgentPermissionMode::Ask,
        };
        let status = bundle
            .invoke_model_tool("git.status", json!({}), &context)
            .unwrap();
        assert_eq!(status["kind"], "status");

        let search = bundle
            .invoke_model_tool(
                "code.search",
                json!({"query": "answer", "mode": "symbol"}),
                &context,
            )
            .unwrap();
        assert_eq!(search["kind"], "search");
        assert!(!search["hits"].as_array().unwrap().is_empty());

        let read = bundle
            .invoke_model_tool("computer.fs.read", json!({"path": "src/lib.rs"}), &context)
            .unwrap();
        assert!(read.to_string().contains("41"));
        let mut unapproved = context.clone();
        unapproved.approved = false;
        assert_eq!(
            bundle
                .invoke_model_tool(
                    "computer.fs.write",
                    json!({"path": "blocked.txt", "content": "no"}),
                    &unapproved,
                )
                .unwrap_err()
                .code,
            "agent.permission.denied"
        );
        assert_eq!(
            bundle
                .invoke_model_tool("missing.tool", json!({}), &context)
                .unwrap_err()
                .code,
            "agent.not_found"
        );
    }

    #[test]
    fn fix_and_review_golden_paths_emit_replayable_events() {
        let bundle = seeded_bundle();
        let events = CodingEventLog::new("native-fix");
        emit_deterministic_coding_run(&events, "fix-turn");

        let fix = run_fix_golden_path(&bundle).unwrap();
        assert_eq!(fix["patched"], "patched");
        assert!(fix["content_has_42"].as_bool().unwrap());
        assert_eq!(fix["official_servers"], 0);

        let review = run_review_golden_path(&bundle).unwrap();
        assert!(!review["findings"].as_array().unwrap().is_empty());
        assert_eq!(review["artifact"]["kind"], "code_review");

        let page = events.page(0);
        assert_eq!(page.events.len(), 8);
        let replay = events.page(3);
        assert_eq!(replay.events.first().unwrap().sequence, 4);
    }

    #[test]
    fn cancel_resume_does_not_duplicate_file_side_effects() {
        let bundle = seeded_bundle();
        let result = run_resume_without_duplicate_side_effects(&bundle).unwrap();
        assert_eq!(result["first_kind"], "patched");
        assert!(result["duplicate_suppressed"].as_bool().unwrap());
    }

    #[test]
    fn liliacode_migration_client_covers_binding_timeline_and_fork() {
        let bundle = seeded_bundle();
        let mut client = LiliaCodeNativeMigrationClient::bind(bundle, "task-42");
        emit_deterministic_coding_run(&client.events, "migrate");
        client.project_events();
        let snapshot = client.editor_snapshot().unwrap();
        assert_eq!(snapshot.workspace.workspace_id, "ws");
        client.request_approval("write-main");
        client.resolve_approval("write-main", true).unwrap();
        client.cancel();
        client.reset();
        client.regenerate();
        let forked = client.fork("task-43");
        assert_eq!(forked.fork_of.as_deref(), Some(client.session_id.as_str()));
        assert!(client.timeline.iter().any(|item| item.contains("event:")));
        assert!(Arc::ptr_eq(&client.bundle.git, &forked.bundle.git));
        assert!(Arc::ptr_eq(
            &client.bundle.code_index,
            &forked.bundle.code_index
        ));
    }

    #[test]
    fn performance_smoke_native_fix_loop() {
        let bundle = seeded_bundle();
        let started = Instant::now();
        for _ in 0..24 {
            let _ = run_fix_golden_path(&bundle).unwrap();
        }
        assert!(
            started.elapsed().as_millis() < 2_500,
            "native coding fix smoke exceeded budget"
        );
    }
}
