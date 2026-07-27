use serde::{Deserialize, Serialize};

use crate::{AgentMessage, ResourceCellRef, ResourceRef};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionCreateRequest {
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionSnapshotRequest {
    pub session_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSession {
    pub session_id: String,
    pub profile_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub messages: Vec<AgentMessage>,
    pub turn_count: u64,
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
            resource,
            cell,
        }
    }
}
