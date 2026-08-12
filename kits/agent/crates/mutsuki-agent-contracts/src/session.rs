use serde::{Deserialize, Serialize};

use crate::{
    AgentBudget, AgentEventEnvelope, AgentMessage, CoordinatorLease, PermissionRequest,
    ResourceCellRef, ResourceRef, SessionVersion,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionSnapshotRef {
    pub session_id: String,
    pub version: SessionVersion,
    pub snapshot: ResourceRef,
    #[serde(default)]
    pub base: Option<ResourceRef>,
    #[serde(default)]
    pub deltas: Vec<ResourceRef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionDelta {
    pub session_id: String,
    pub expected_version: SessionVersion,
    pub next_version: SessionVersion,
    pub delta: ResourceRef,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentSessionEvent {
    TurnStarted {
        turn_id: String,
    },
    SnapshotAdvanced {
        snapshot: Box<SessionSnapshotRef>,
    },
    ApprovalPending {
        request: PermissionRequest,
    },
    AttemptCompleted {
        attempt_id: String,
        result: Option<Box<ResourceRef>>,
    },
    TurnCompleted {
        turn_id: String,
        result: Option<Box<ResourceRef>>,
    },
    TurnCancelled {
        turn_id: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAttemptCheckpoint {
    pub attempt_id: String,
    pub turn_id: String,
    pub step_index: u32,
    pub state: String,
    #[serde(default)]
    pub committed_side_effects: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSessionCheckpoint {
    pub session_id: String,
    #[serde(default)]
    pub profile_id: String,
    pub version: SessionVersion,
    #[serde(default)]
    pub budget: AgentBudget,
    pub state: ResourceRef,
    pub snapshot: SessionSnapshotRef,
    #[serde(default)]
    pub pending_approvals: Vec<PermissionRequest>,
    #[serde(default)]
    pub plugin_generations: std::collections::BTreeMap<String, u64>,
    #[serde(default)]
    pub attempts: std::collections::BTreeMap<String, AgentAttemptCheckpoint>,
    #[serde(default)]
    pub coordinator: Option<CoordinatorLease>,
    #[serde(default)]
    pub degraded_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionCreateRequest {
    /// Optional externally stable id used when attaching a migrated product
    /// binding. Omitted requests retain AgentKit-assigned ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub profile_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionGetRequest {
    pub session_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSessionAppendRequest {
    pub session_id: String,
    pub messages: Vec<AgentMessage>,
    #[serde(default)]
    pub events: Vec<AgentEventEnvelope>,
    /// False for approval-only continuation of the current logical turn.
    #[serde(default = "default_true")]
    pub advance_turn: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionSnapshotRequest {
    pub session_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionForkRequest {
    pub source_session_id: String,
    pub target_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through_turn_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSession {
    pub session_id: String,
    pub profile_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub messages: Vec<AgentMessage>,
    pub turn_count: u64,
    #[serde(default)]
    pub events: Vec<AgentEventEnvelope>,
    #[serde(default)]
    pub next_event_sequence: u64,
    pub resource: ResourceRef,
    pub cell: ResourceCellRef,
}

impl AgentSession {
    pub fn new(
        session_id: impl Into<String>,
        profile_id: impl Into<String>,
        resource: ResourceRef,
        cell: ResourceCellRef,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            profile_id: profile_id.into(),
            title: None,
            messages: Vec::new(),
            turn_count: 0,
            events: Vec::new(),
            next_event_sequence: 0,
            resource,
            cell,
        }
    }
}
