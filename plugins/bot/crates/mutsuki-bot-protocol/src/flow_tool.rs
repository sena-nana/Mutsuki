//! Agent-facing Bot Flow editing tool contract.
//!
//! `mutsuki.bot.flow.tool/execute@1` receives the neutral Agent tool execution
//! envelope (`ToolTargetPayloadMode::ExecutionRequest`): the envelope `name`
//! selects the op and `input` carries its typed payload. Every op goes through
//! the Bot Flow ConfigService provider, so the active document remains the
//! single matching and routing source; no Agent tool bypasses revision-CAS
//! validation.

use serde::{Deserialize, Serialize};

use crate::{BotFlowDocument, BotNodeDescriptor};

pub const BOT_FLOW_TOOL_PROTOCOL_ID: &str = "mutsuki.bot.flow.tool/execute@1";

pub const BOT_FLOW_TOOL_READ: &str = "bot.flow.read";
pub const BOT_FLOW_TOOL_VALIDATE: &str = "bot.flow.validate";
pub const BOT_FLOW_TOOL_APPLY: &str = "bot.flow.apply";

/// `bot.flow.validate` input: the candidate document, not yet applied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotFlowToolValidateInput {
    pub flow: BotFlowDocument,
}

/// `bot.flow.apply` input: the candidate document plus the revision the caller
/// last observed. A stale `expected_revision` is a model-visible conflict, not
/// an infrastructure failure.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotFlowToolApplyInput {
    pub flow: BotFlowDocument,
    pub expected_revision: u64,
}

/// `bot.flow.read` output: the active document, its revision and the node
/// catalog the document must be built against.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotFlowToolReadOutput {
    pub revision: u64,
    pub flow: BotFlowDocument,
    pub catalog: Vec<BotNodeDescriptor>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotFlowToolIssueSeverity {
    Error,
    Warning,
}

/// One validation issue, mirrored from the Flow registry validation result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotFlowToolIssue {
    #[serde(default)]
    pub path: Vec<String>,
    pub severity: BotFlowToolIssueSeverity,
    pub message: String,
}

/// `bot.flow.validate` output.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BotFlowToolValidateOutput {
    pub ok: bool,
    #[serde(default)]
    pub issues: Vec<BotFlowToolIssue>,
}

/// `bot.flow.apply` output: the revision the CAS apply activated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotFlowToolApplyOutput {
    pub revision: u64,
    pub applied: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BotFlowEdge, BotFlowNode};

    #[test]
    fn flow_tool_inputs_round_trip_with_typed_payloads() {
        let flow = BotFlowDocument {
            nodes: vec![BotFlowNode {
                node_id: "n1".into(),
                node_type_id: "agent.submit".into(),
                node_type_version: 1,
                config: serde_json::json!({}),
                source: None,
                position: Default::default(),
            }],
            edges: vec![BotFlowEdge {
                edge_id: "e1".into(),
                from_node_id: "n1".into(),
                from_port_id: "out".into(),
                to_node_id: "n1".into(),
                to_port_id: "in".into(),
                kind: crate::BotFlowEdgeKind::Event,
            }],
            ..BotFlowDocument::default()
        };
        let apply = BotFlowToolApplyInput {
            flow: flow.clone(),
            expected_revision: 7,
        };
        let encoded = serde_json::to_value(&apply).unwrap();
        assert_eq!(encoded["expected_revision"], 7);
        assert_eq!(
            serde_json::from_value::<BotFlowToolApplyInput>(encoded).unwrap(),
            apply
        );

        let validate = BotFlowToolValidateInput { flow };
        let encoded = serde_json::to_value(&validate).unwrap();
        assert!(encoded["flow"]["flow_id"].is_string());
        assert_eq!(
            serde_json::from_value::<BotFlowToolValidateInput>(encoded).unwrap(),
            validate
        );
    }

    #[test]
    fn flow_tool_outputs_round_trip_and_default_issues() {
        let read = BotFlowToolReadOutput {
            revision: 3,
            flow: BotFlowDocument::default(),
            catalog: Vec::new(),
        };
        let encoded = serde_json::to_value(&read).unwrap();
        assert_eq!(encoded["revision"], 3);
        assert_eq!(
            serde_json::from_value::<BotFlowToolReadOutput>(encoded).unwrap(),
            read
        );

        let validate: BotFlowToolValidateOutput =
            serde_json::from_value(serde_json::json!({ "ok": true })).unwrap();
        assert!(validate.ok);
        assert!(validate.issues.is_empty());

        let issue = BotFlowToolIssue {
            path: vec!["nodes".into(), "n1".into()],
            severity: BotFlowToolIssueSeverity::Error,
            message: "unknown node type".into(),
        };
        let encoded = serde_json::to_value(&issue).unwrap();
        assert_eq!(encoded["severity"], "error");
        assert_eq!(
            serde_json::from_value::<BotFlowToolIssue>(encoded).unwrap(),
            issue
        );

        let apply = BotFlowToolApplyOutput {
            revision: 4,
            applied: true,
        };
        let encoded = serde_json::to_value(&apply).unwrap();
        assert_eq!(
            serde_json::from_value::<BotFlowToolApplyOutput>(encoded).unwrap(),
            apply
        );
    }
}
