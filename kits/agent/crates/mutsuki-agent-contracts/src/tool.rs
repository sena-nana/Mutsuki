use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{AgentError, InteractionKind, PermissionDecision, PermissionRequest};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSideEffect {
    #[default]
    None,
    WorkspaceRead,
    WorkspaceWrite,
    ExternalRead,
    ExternalWrite,
}

impl ToolSideEffect {
    pub fn is_read_only(self) -> bool {
        matches!(self, Self::None | Self::WorkspaceRead | Self::ExternalRead)
    }

    pub fn is_write(self) -> bool {
        matches!(self, Self::WorkspaceWrite | Self::ExternalWrite)
    }
}

/// Payload shape delivered to the tool's target protocol.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolTargetPayloadMode {
    /// Backward-compatible target input: only the model-produced JSON value.
    #[default]
    RawInput,
    /// Full neutral execution envelope including session, approval and Host context.
    ExecutionRequest,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentToolExecution {
    #[default]
    Routed,
    Interaction {
        interaction_kind: InteractionKind,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentToolDescriptor {
    pub name: String,
    pub target_protocol_id: String,
    pub description: String,
    #[serde(default)]
    pub input_schema: Value,
    #[serde(default)]
    pub output_schema: Value,
    #[serde(default)]
    pub side_effect: ToolSideEffect,
    #[serde(default)]
    pub requires_approval: bool,
    #[serde(default)]
    pub target_payload_mode: ToolTargetPayloadMode,
    #[serde(default)]
    pub execution: AgentToolExecution,
    #[serde(default)]
    pub permissions: Vec<String>,
}

impl AgentToolDescriptor {
    pub fn new(
        name: impl Into<String>,
        target_protocol_id: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            target_protocol_id: target_protocol_id.into(),
            description: description.into(),
            input_schema: json!({}),
            output_schema: json!({}),
            side_effect: ToolSideEffect::None,
            requires_approval: false,
            target_payload_mode: ToolTargetPayloadMode::RawInput,
            execution: AgentToolExecution::Routed,
            permissions: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentToolListRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentToolListResult {
    pub tools: Vec<AgentToolDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentToolExecuteRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub input: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<AgentToolApproval>,
    /// Host/product facts forwarded without granting authority. Context-aware
    /// targets must still validate workspace and approval capabilities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    /// Copied from the originating Agent run. Tool Router enforces `read_only`
    /// even when callers bypass AgentLoop.
    #[serde(default)]
    pub permission_mode: crate::AgentPermissionMode,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentToolApproval {
    pub request: PermissionRequest,
    pub decision: PermissionDecision,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentToolExecuteResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_ref: Option<String>,
    /// Structured business failure returned by the target tool. Infrastructure
    /// failures remain Runtime failures and do not masquerade as model-visible output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<AgentError>,
    #[serde(default)]
    pub approved: bool,
}

/// Provider-neutral metadata attached to an `AgentRole::Tool` transcript message.
/// Model adapters use this contract to preserve tool-call causality and error state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentToolResultMetadata {
    pub call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_ref: Option<String>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<AgentError>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentToolCall {
    pub call_id: String,
    pub name: String,
    #[serde(default)]
    pub input: Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_result_metadata_round_trips_structured_failure() {
        let metadata = AgentToolResultMetadata {
            call_id: "call-1".into(),
            output_ref: None,
            is_error: true,
            error: Some(AgentError::new("tool.failed", "target rejected input")),
        };
        let encoded = serde_json::to_value(&metadata).unwrap();
        assert_eq!(
            serde_json::from_value::<AgentToolResultMetadata>(encoded).unwrap(),
            metadata
        );
    }

    #[test]
    fn legacy_tool_result_metadata_defaults_to_success() {
        let metadata: AgentToolResultMetadata =
            serde_json::from_value(serde_json::json!({"call_id": "call-1"})).unwrap();
        assert!(!metadata.is_error);
        assert!(metadata.error.is_none());
    }

    #[test]
    fn legacy_tool_descriptor_and_execution_default_to_raw_payload_without_context() {
        let descriptor: AgentToolDescriptor = serde_json::from_value(serde_json::json!({
            "name": "echo",
            "target_protocol_id": "test.echo@1",
            "description": "echo"
        }))
        .unwrap();
        assert_eq!(
            descriptor.target_payload_mode,
            ToolTargetPayloadMode::RawInput
        );
        assert_eq!(descriptor.execution, AgentToolExecution::Routed);

        let execution: AgentToolExecuteRequest = serde_json::from_value(serde_json::json!({
            "name": "echo",
            "input": {"value": "ping"}
        }))
        .unwrap();
        assert!(execution.context.is_none());
    }

    #[test]
    fn interaction_tool_execution_round_trips_with_its_kind() {
        let mut descriptor = AgentToolDescriptor::new(
            "confirm_plan",
            crate::AGENT_RUN_PROTOCOL,
            "Confirm a proposed plan",
        );
        descriptor.execution = AgentToolExecution::Interaction {
            interaction_kind: InteractionKind::PlanConfirm,
        };

        let encoded = serde_json::to_value(&descriptor).unwrap();
        assert_eq!(encoded["execution"]["kind"], "interaction");
        assert_eq!(encoded["execution"]["interaction_kind"], "plan_confirm");
        assert_eq!(
            serde_json::from_value::<AgentToolDescriptor>(encoded).unwrap(),
            descriptor
        );
    }
}
