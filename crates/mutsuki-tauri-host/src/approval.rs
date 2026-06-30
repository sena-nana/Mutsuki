use crate::error::{HostError, HostResult};
use mutsuki_tauri_bridge::{ApprovalDecision, ApprovalRequest, ApprovalResponse, FrontendContext};
use parking_lot::RwLock;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct PendingApproval {
    pub request: ApprovalRequest,
}

#[derive(Clone, Debug, Default)]
pub struct ApprovalBridge {
    pending: Arc<RwLock<BTreeMap<String, PendingApproval>>>,
}

impl ApprovalBridge {
    pub fn request(
        &self,
        requester: impl Into<String>,
        operation: impl Into<String>,
        risk: impl Into<String>,
        payload: Value,
        context: FrontendContext,
    ) -> ApprovalRequest {
        let request = ApprovalRequest {
            approval_id: format!("approval:{}", Uuid::new_v4()),
            token: Uuid::new_v4().to_string(),
            requester: requester.into(),
            operation: operation.into(),
            risk: risk.into(),
            payload,
            context,
        };
        self.pending.write().insert(
            request.approval_id.clone(),
            PendingApproval {
                request: request.clone(),
            },
        );
        request
    }

    pub fn resolve(&self, response: ApprovalResponse) -> HostResult<ApprovalDecision> {
        let Some(pending) = self.pending.write().remove(&response.approval_id) else {
            return Err(HostError::Approval(format!(
                "approval request not pending: {}",
                response.approval_id
            )));
        };
        if pending.request.token != response.token {
            return Err(HostError::Approval(format!(
                "approval token mismatch: {}",
                response.approval_id
            )));
        }
        Ok(response.decision)
    }

    pub fn pending(&self) -> Vec<ApprovalRequest> {
        self.pending
            .read()
            .values()
            .map(|entry| entry.request.clone())
            .collect()
    }
}
