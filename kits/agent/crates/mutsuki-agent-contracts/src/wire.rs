use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    AgentMessage, AgentSession, AgentSessionCreateRequest, AgentSessionState, PermissionDecision,
    ResourceRef, SessionSnapshotRef, SessionVersion,
};

pub const AGENT_WIRE_VERSION: u16 = 1;
pub const AGENT_WIRE_SUPPORTED_FEATURES: [&str; 4] = [
    "approval-binding",
    "event-resume",
    "monotonic-events",
    "resource-ref",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentWireHello {
    pub version: u16,
    pub required_features: Vec<String>,
    pub optional_features: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentWireNegotiation {
    pub version: u16,
    pub enabled_features: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentWireRequestEnvelope {
    pub request_id: u64,
    pub hello: AgentWireHello,
    pub request: AgentWireRequest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum AgentWireRequest {
    Negotiate,
    StartSession {
        request: AgentSessionCreateRequest,
    },
    GetSession {
        session_id: String,
    },
    SubmitTurn {
        session_id: String,
        expected_version: SessionVersion,
        turn_id: String,
        messages: Vec<AgentMessage>,
        idempotency_key: String,
    },
    CancelTurn {
        session_id: String,
        turn_id: String,
        expected_version: SessionVersion,
    },
    ApproveAction {
        decision: PermissionDecision,
    },
    RejectAction {
        decision: PermissionDecision,
    },
    SubscribeSessionEvents {
        session_id: String,
        after_sequence: u64,
        limit: u32,
    },
    ResumeSession {
        session_id: String,
        after_sequence: u64,
    },
    ForkSession {
        source_session_id: String,
        target_session_id: String,
        snapshot: SessionSnapshotRef,
    },
    CloseSession {
        session_id: String,
        expected_version: SessionVersion,
    },
    ListSessions {
        #[serde(default)]
        after_session_id: Option<String>,
        limit: u32,
    },
    ReadResource {
        resource: ResourceRef,
        offset: u64,
        length: u32,
    },
    ListRuntimeCapabilities,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentWireResponseEnvelope {
    pub request_id: u64,
    pub response: Result<AgentWireResponse, AgentWireError>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum AgentWireResponse {
    Negotiated(AgentWireNegotiation),
    Session(AgentSession),
    SessionState(AgentSessionState),
    Accepted {
        session_id: String,
        version: SessionVersion,
    },
    Events(AgentEventPage),
    Capabilities(BTreeMap<String, String>),
    Sessions {
        session_ids: Vec<String>,
        #[serde(default)]
        next_session_id: Option<String>,
    },
    ResourceChunk {
        resource: ResourceRef,
        offset: u64,
        bytes: Vec<u8>,
        eof: bool,
    },
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentWireError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentEventEnvelope {
    pub session_id: String,
    pub sequence: u64,
    pub event: AgentEvent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentEventPage {
    pub events: Vec<AgentEventEnvelope>,
    pub next_sequence: u64,
    pub lost: u64,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    SessionState {
        version: SessionVersion,
        status: String,
    },
    TurnState {
        turn_id: String,
        status: String,
    },
    StepState {
        turn_id: String,
        step_index: u32,
        status: String,
    },
    ModelDelta {
        turn_id: String,
        text: String,
    },
    ReasoningDelta {
        turn_id: String,
        text: String,
    },
    ToolCall {
        turn_id: String,
        call_id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        turn_id: String,
        call_id: String,
        summary: String,
        details: Option<ResourceRef>,
    },
    ApprovalRequest {
        request: crate::PermissionRequest,
    },
    ContextStatus {
        turn_id: String,
        status: String,
    },
    Usage {
        turn_id: String,
        usage: crate::AgentUsage,
    },
    Final {
        turn_id: String,
        result: Option<ResourceRef>,
    },
    Failed {
        turn_id: Option<String>,
        error: AgentWireError,
    },
    Cancelled {
        turn_id: String,
    },
}
