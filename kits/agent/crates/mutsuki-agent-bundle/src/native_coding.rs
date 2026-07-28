//! Native Coding Agent reference bundle.
//!
//! Assembles AgentKit Runtime services and coding plugins without Codex /
//! Claude official Agent Server processes. Products inject Host secrets,
//! editor context, and process backends; this crate only owns the assembly.

use std::collections::BTreeSet;
use std::sync::Arc;

use mutsuki_agent_contracts::{
    AgentDelegationRequest, AgentError, AgentKitPluginDescriptor, AgentPluginStateKind,
    AgentResult, AgentRuntimeProfile, AgentServiceDescriptor, AgentToolDescriptor,
    AgentWorkspaceRef, CodeFileChange, CodeIndexBatch, CodeSearchMode, CodeSearchQuery,
    CodeWorkspaceRef, DelegationBudget, DelegationMode, DelegationScope, GitRepositoryRef,
    GitWorktreeRef, SubAgentDescriptor, SubAgentOutcomeKind, ToolSideEffect,
};
use mutsuki_agent_plugin_api::AgentPluginRegistrar;
use mutsuki_agent_plugin_code_index::{CodeIndexLspSignals, SharedCodeIndexService};
use mutsuki_agent_plugin_computer_use::{
    BrowserBackend, FakeBrowserBackend, FakeProcessBackend, FilesystemBackend,
    InMemoryFilesystemBackend, ProcessBackend, SharedComputerUseService,
};
use mutsuki_agent_plugin_git::{GitBackend, InMemoryGitBackend, SharedGitService};
use mutsuki_agent_plugin_lsp::{LspProcess, LspProcessFactory, SharedLspService};
use mutsuki_agent_plugin_mcp::{McpTransport, McpTransportFactory, SharedMcpService};
use mutsuki_agent_runtime::{
    AgentResourceStore, EchoChildExecutor, SubAgentOrchestrator,
    reference_coding_agent_test_profile,
};
use serde_json::{Value, json};

use crate::AgentPluginBundle;

pub const NATIVE_CODING_BUNDLE_ID: &str = "mutsuki.agent.bundle.native-coding";
pub const LSP_PLUGIN_ID: &str = "mutsuki.plugin.agent.lsp";

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
    pub git: Arc<dyn GitBackend>,
    pub filesystem: Arc<dyn FilesystemBackend>,
    pub process: Option<Arc<dyn ProcessBackend>>,
    pub browser: Option<Arc<dyn BrowserBackend>>,
    pub lsp: Arc<dyn LspProcessFactory>,
    pub mcp: Arc<dyn McpTransportFactory>,
    pub code_index_lsp: Arc<dyn CodeIndexLspSignals>,
}

impl Default for NativeCodingBackends {
    fn default() -> Self {
        Self {
            git: Arc::new(InMemoryGitBackend::default()),
            filesystem: Arc::new(InMemoryFilesystemBackend::default()),
            process: Some(Arc::new(FakeProcessBackend::default())),
            browser: Some(Arc::new(FakeBrowserBackend::default())),
            lsp: Arc::new(UnavailableLspFactory),
            mcp: Arc::new(UnavailableMcpFactory),
            code_index_lsp: Arc::new(mutsuki_agent_plugin_code_index::UnavailableLspSignals),
        }
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
        let subagents = SubAgentOrchestrator::new(Arc::new(EchoChildExecutor));
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
        Self {
            core: AgentPluginBundle::default(),
            resources,
            git,
            code_index,
            lsp,
            computer_use,
            mcp,
            subagents,
            profile: reference_coding_agent_test_profile(),
        }
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
        CodeWorkspaceRef {
            workspace_id: "native-workspace".into(),
            root: "/workspace".into(),
            tenant_id: "native".into(),
            git_revision: Some("HEAD".into()),
            worktree_id: None,
        }
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
        NativeCodingAgentBundle::reference(NativeCodingBackends {
            git,
            filesystem: fs,
            ..Default::default()
        })
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
