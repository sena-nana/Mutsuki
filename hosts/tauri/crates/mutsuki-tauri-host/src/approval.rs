use crate::error::{HostError, HostResult};
use mutsuki_tauri_bridge::{
    ApprovalAttribution, ApprovalDecision, ApprovalRequest, ApprovalResponse, FrontendContext,
};
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
        self.request_with_attribution(
            requester,
            operation,
            risk,
            payload,
            fallback_attribution(context),
        )
    }

    pub fn request_with_attribution(
        &self,
        requester: impl Into<String>,
        operation: impl Into<String>,
        risk: impl Into<String>,
        payload: Value,
        attribution: ApprovalAttribution,
    ) -> ApprovalRequest {
        let request = ApprovalRequest {
            approval_id: format!("approval:{}", Uuid::new_v4()),
            token: Uuid::new_v4().to_string(),
            requester: requester.into(),
            operation: operation.into(),
            risk: risk.into(),
            trace_id: attribution.trace_id,
            correlation_id: attribution.correlation_id,
            payload,
            context: attribution.context,
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
        let mut pending = self.pending.write();
        let Some(entry) = pending.get(&response.approval_id) else {
            return Err(HostError::Approval(format!(
                "approval request not pending: {}",
                response.approval_id
            )));
        };
        if entry.request.token != response.token {
            return Err(HostError::Approval(format!(
                "approval token mismatch: {}",
                response.approval_id
            )));
        }
        let attribution_matches = response
            .trace_id
            .as_deref()
            .is_none_or(|trace_id| trace_id == entry.request.trace_id)
            && response
                .correlation_id
                .as_deref()
                .is_none_or(|correlation_id| correlation_id == entry.request.correlation_id)
            && response
                .context
                .as_ref()
                .is_none_or(|context| context == &entry.request.context);
        if !attribution_matches {
            return Err(HostError::Approval(format!(
                "approval attribution mismatch: {}",
                response.approval_id
            )));
        }
        pending.remove(&response.approval_id);
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

fn fallback_attribution(context: FrontendContext) -> ApprovalAttribution {
    let id = Uuid::new_v4();
    ApprovalAttribution {
        trace_id: format!("approval-trace:{id}"),
        correlation_id: format!("approval-correlation:{id}"),
        context,
    }
}
