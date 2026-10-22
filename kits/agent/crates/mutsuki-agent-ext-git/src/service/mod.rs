//! `SharedGitService` state, approval/conflict fencing and operation tracking.

mod dispatch;
mod providers;
mod registration;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mutsuki_agent_contracts::{
    AgentError, AgentPluginStateKind, AgentServiceDescriptor, GitActionPlan, GitOperationHandle,
    GitOperationKind, GitOperationState, GitRevisionConflict, GitRisk, GitServiceRequest,
    GitServiceResponse, GitStatusSnapshot, GitWorktreeRef, GitWriteContext, PermissionRequest,
    ResourceRef,
};
use mutsuki_agent_ext_api::AgentService;
use mutsuki_agent_services::AgentResourceStore;
use serde_json::{Value, json};

use crate::gateway::GitGateway;
use crate::memory::InMemoryGitBackend;
use crate::{INLINE_LIMIT, SERVICE_ID};

struct ActiveOp {
    handle: GitOperationHandle,
    cancel: Arc<AtomicBool>,
}

/// Shared GitService consumed by Agent tools and product Git UI.
pub struct SharedGitService {
    descriptor: AgentServiceDescriptor,
    backend: Arc<dyn GitGateway>,
    resources: AgentResourceStore,
    next_action: AtomicU64,
    active: Mutex<BTreeMap<String, ActiveOp>>,
    write_fences: Mutex<BTreeMap<String, Arc<Mutex<()>>>>,
}

impl SharedGitService {
    pub fn new(backend: Arc<dyn GitGateway>, resources: AgentResourceStore) -> Self {
        Self {
            descriptor: AgentServiceDescriptor {
                service_id: SERVICE_ID.into(),
                version: "1".into(),
                request_schema: "mutsuki.agent.git.request@1".into(),
                response_schema: "mutsuki.agent.git.response@1".into(),
                state: AgentPluginStateKind::Stateful,
                affinity: Some("worktree".into()),
            },
            backend,
            resources,
            next_action: AtomicU64::new(1),
            active: Mutex::new(BTreeMap::new()),
            write_fences: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn in_memory(backend: InMemoryGitBackend) -> Self {
        Self::new(Arc::new(backend), AgentResourceStore::default())
    }

    pub fn active_handle_count(&self) -> usize {
        self.active.lock().expect("git active").len()
    }

    pub fn call_value(&self, request: Value) -> Result<Value, AgentError> {
        AgentService::call(self, request)
    }

    fn plan(
        &self,
        operation: &str,
        risk: GitRisk,
        summary: impl Into<String>,
        snapshot: &GitStatusSnapshot,
        write: &GitWriteContext,
        preview: Option<Value>,
    ) -> GitActionPlan {
        let action_id = format!("git-{}", self.next_action.fetch_add(1, Ordering::Relaxed));
        GitActionPlan {
            action_id: action_id.clone(),
            operation: operation.into(),
            risk,
            summary: summary.into(),
            approval: PermissionRequest {
                session_id: write.session_id.clone(),
                turn_id: write.turn_id.clone(),
                action_id,
                tool: operation.into(),
                side_effect: risk.side_effect(),
                summary: format!("{operation} requires approval"),
                version: write.approval_version.unwrap_or(1),
            },
            worktree: snapshot.worktree.clone(),
            head: snapshot.head.clone(),
            state: snapshot.state.clone(),
            preview,
        }
    }

    fn ensure_approved(
        &self,
        risk: GitRisk,
        operation: &str,
        worktree: &GitWorktreeRef,
        write: &GitWriteContext,
        preview: Option<Value>,
    ) -> Result<Option<GitServiceResponse>, AgentError> {
        let snapshot = self.backend.status(worktree)?;
        let head = &snapshot.head;
        let state = &snapshot.state;
        if let Some(expected) = &write.expected_state
            && expected != state
        {
            return Ok(Some(GitServiceResponse::Conflict(
                GitRevisionConflict::stale_state(
                    worktree.clone(),
                    expected.clone(),
                    state.clone(),
                    head.clone(),
                ),
            )));
        }
        if write.expected_state.is_none()
            && let Some(expected) = &write.expected_head
            && (expected.commit != head.commit || expected.generation != head.generation)
        {
            return Ok(Some(GitServiceResponse::Conflict(
                GitRevisionConflict::stale(worktree.clone(), expected.clone(), head.clone()),
            )));
        }
        if risk.requires_approval() && !write.approved {
            return Ok(Some(GitServiceResponse::Plan(self.plan(
                operation,
                risk,
                format!("{operation} is waiting for approval"),
                &snapshot,
                write,
                preview,
            ))));
        }
        Ok(None)
    }

    fn write_fence(&self, key: String) -> Arc<Mutex<()>> {
        self.write_fences
            .lock()
            .expect("git write fences")
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn begin_op(
        &self,
        kind: GitOperationKind,
        risk: GitRisk,
        worktree: Option<GitWorktreeRef>,
        summary: impl Into<String>,
    ) -> (String, Arc<AtomicBool>) {
        let handle_id = format!(
            "git-op-{}",
            self.next_action.fetch_add(1, Ordering::Relaxed)
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let handle = GitOperationHandle {
            handle_id: handle_id.clone(),
            kind,
            state: GitOperationState::Running,
            risk,
            worktree,
            summary: summary.into(),
        };
        self.active.lock().expect("git active").insert(
            handle_id.clone(),
            ActiveOp {
                handle,
                cancel: cancel.clone(),
            },
        );
        (handle_id, cancel)
    }

    fn finish_op(&self, handle_id: &str, state: GitOperationState) {
        if let Some(mut op) = self.active.lock().expect("git active").remove(handle_id) {
            op.handle.state = state;
        }
    }

    fn store_text(
        &self,
        kind: &str,
        schema: &str,
        text: &str,
    ) -> Result<(Option<ResourceRef>, Option<String>, bool), AgentError> {
        if text.len() <= INLINE_LIMIT {
            return Ok((None, Some(text.to_string()), false));
        }
        let reference =
            self.resources
                .put_json(SERVICE_ID, kind, schema, 1, &json!({ "text": text }))?;
        Ok((Some(reference), None, true))
    }

    fn with_timeout<T>(
        &self,
        write: &GitWriteContext,
        cancel: &AtomicBool,
        op: impl FnOnce() -> Result<T, AgentError>,
    ) -> Result<T, AgentError> {
        let started = Instant::now();
        let result = op();
        if cancel.load(Ordering::Relaxed) {
            return Err(AgentError::new(
                "agent.git.cancelled",
                "git operation was cancelled",
            ));
        }
        if started.elapsed() > Duration::from_millis(write.timeout_ms.max(1)) {
            return Err(AgentError::new(
                "agent.git.timeout",
                "git operation exceeded timeout",
            ));
        }
        result
    }

    fn dispatch(&self, request: GitServiceRequest) -> Result<GitServiceResponse, AgentError> {
        let Some(key) = write_affinity(&request) else {
            return self.dispatch_inner(request);
        };
        let fence = self.write_fence(key);
        let _guard = fence.lock().map_err(|_| {
            AgentError::new("agent.git.state_poisoned", "git write fence lock poisoned")
        })?;
        self.dispatch_inner(request)
    }
}

fn write_affinity(request: &GitServiceRequest) -> Option<String> {
    match request {
        GitServiceRequest::Stage { worktree, .. }
        | GitServiceRequest::Unstage { worktree, .. }
        | GitServiceRequest::Commit { worktree, .. }
        | GitServiceRequest::BranchCreate { worktree, .. }
        | GitServiceRequest::BranchSwitch { worktree, .. }
        | GitServiceRequest::BranchDelete { worktree, .. }
        | GitServiceRequest::WorktreeRemove { worktree, .. }
        | GitServiceRequest::Push { worktree, .. }
        | GitServiceRequest::Pull { worktree, .. }
        | GitServiceRequest::Fetch { worktree, .. }
        | GitServiceRequest::Merge { worktree, .. }
        | GitServiceRequest::Rebase { worktree, .. }
        | GitServiceRequest::Reset { worktree, .. }
        | GitServiceRequest::CherryPick { worktree, .. } => Some(worktree.worktree_id.clone()),
        GitServiceRequest::WorktreeCreate { repository, .. } => {
            Some(format!("repository:{}", repository.repo_id))
        }
        _ => None,
    }
}
