//! `AgentService` and `ContextProvider` implementations for `SharedGitService`.

use std::sync::atomic::Ordering;

use mutsuki_agent_contracts::{
    AgentError, AgentServiceDescriptor, GitOperationState, GitServiceRequest,
};
use mutsuki_agent_ext_api::{AgentService, ContextProvider};
use serde_json::{Value, json};

use super::SharedGitService;
use crate::{CONTEXT_PROVIDER_ID, SERVICE_ID};

impl AgentService for SharedGitService {
    fn descriptor(&self) -> &AgentServiceDescriptor {
        &self.descriptor
    }

    fn call(&self, request: Value) -> Result<Value, AgentError> {
        let request: GitServiceRequest = serde_json::from_value(request)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let response = self.dispatch(request)?;
        serde_json::to_value(response).map_err(|error| AgentError::invalid_input(error.to_string()))
    }

    fn drain(&self) -> Result<(), AgentError> {
        Ok(())
    }

    fn dispose(&self) -> Result<(), AgentError> {
        let handles = self
            .active
            .lock()
            .expect("git active")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for handle_id in handles {
            if let Some(op) = self.active.lock().expect("git active").get(&handle_id) {
                op.cancel.store(true, Ordering::Relaxed);
            }
            self.backend.cancel_process(&handle_id)?;
            self.finish_op(&handle_id, GitOperationState::Cancelled);
        }
        Ok(())
    }
}

impl ContextProvider for SharedGitService {
    fn provider_id(&self) -> &str {
        CONTEXT_PROVIDER_ID
    }

    fn collect(
        &self,
        request: mutsuki_agent_contracts::ContextProviderRequest,
    ) -> Result<mutsuki_agent_contracts::ContextProviderResult, AgentError> {
        let path = request
            .input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("");
        if path.is_empty() {
            return Ok(mutsuki_agent_contracts::ContextProviderResult {
                provider_id: CONTEXT_PROVIDER_ID.into(),
                summary: format!("git ready active_handles={}", self.active_handle_count()),
                details: None,
                estimated_tokens: 8,
                estimated_bytes: 64,
                priority: 0,
                required: false,
            });
        }
        let (worktree, head) = self.backend.discover(path)?;
        let status = self.backend.status(&worktree)?;
        let details = self.resources.put_json(
            SERVICE_ID,
            "mutsuki.agent.git.context",
            "mutsuki.agent.git.context@1",
            1,
            &json!({
                "worktree": worktree,
                "head": status.head,
                "clean": status.clean,
                "changes": status.changes.len(),
            }),
        )?;
        Ok(mutsuki_agent_contracts::ContextProviderResult {
            provider_id: CONTEXT_PROVIDER_ID.into(),
            summary: format!(
                "{} @ {} ({} change(s))",
                head.branch.unwrap_or_else(|| "DETACHED".into()),
                head.commit,
                status.changes.len()
            ),
            details: Some(details),
            estimated_tokens: 64,
            estimated_bytes: 256,
            priority: 10,
            required: false,
        })
    }
}
