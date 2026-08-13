//! Public Agent console management contracts.
//!
//! Web extensions depend on these DTOs and traits. Owner implementations live
//! in Host integration or another assembly crate.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    AgentEventEnvelope, AgentSession, AgentSessionState, AgentSessionStatus, AgentWireError,
    InteractionResolution, PermissionDecision, SessionVersion,
};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConnectionConfig {
    pub connection_id: String,
    pub connector_id: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default)]
    pub config: Value,
}

const fn enabled_by_default() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentConnectionState {
    Disabled,
    Healthy,
    Unavailable,
    Reconnecting,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentConnectionStatus {
    pub connection_id: String,
    pub connector_id: String,
    pub generation: u64,
    pub state: AgentConnectionState,
    pub negotiated_version: Option<u16>,
    pub enabled_features: Vec<String>,
    pub last_error_code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentConnectionManagementSnapshot {
    pub revision: u64,
    pub connections: Vec<AgentConnectionStatus>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSessionSummary {
    pub session_id: String,
    pub title: Option<String>,
    pub turn_count: u64,
    pub message_count: usize,
    pub status: AgentSessionStatus,
    pub total_tokens: u64,
    pub cost_microunits: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSessionPage {
    pub items: Vec<AgentSessionSummary>,
    pub next_session_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentManagementError {
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for AgentManagementError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AgentManagementError {}

/// Revision-fenced Agent connection management used by the console.
pub trait AgentConnectionManagementApi: Send + Sync {
    fn snapshot(&self) -> AgentConnectionManagementSnapshot;

    fn test_connection(
        &self,
        config: AgentConnectionConfig,
    ) -> Result<AgentConnectionStatus, AgentManagementError>;

    fn upsert(
        &self,
        expected_revision: u64,
        config: AgentConnectionConfig,
    ) -> Result<AgentConnectionStatus, AgentManagementError>;

    fn reconnect(
        &self,
        expected_revision: u64,
        connection_id: &str,
    ) -> Result<AgentConnectionStatus, AgentManagementError>;
}

/// Local in-process Agent session management used by the console.
pub trait AgentSessionManagementApi: Send + Sync {
    fn test_provider(&self) -> Result<(), AgentWireError>;

    fn list_sessions(
        &self,
        after_session_id: Option<&str>,
        limit: usize,
    ) -> Result<AgentSessionPage, AgentWireError>;

    fn session(&self, session_id: &str) -> Result<AgentSession, AgentWireError>;

    fn session_state(&self, session_id: &str) -> Result<AgentSessionState, AgentWireError>;

    fn events_after(
        &self,
        session_id: &str,
        after_sequence: u64,
        limit: u32,
    ) -> Result<Vec<AgentEventEnvelope>, AgentWireError>;

    fn approve(&self, decision: PermissionDecision) -> Result<SessionVersion, AgentWireError>;

    fn reject(&self, decision: PermissionDecision) -> Result<SessionVersion, AgentWireError>;

    fn resolve_interaction(
        &self,
        resolution: InteractionResolution,
    ) -> Result<SessionVersion, AgentWireError>;

    fn cancel_turn(
        &self,
        session_id: &str,
        turn_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError>;

    fn close_session(
        &self,
        session_id: &str,
        expected_version: SessionVersion,
    ) -> Result<(), AgentWireError>;
}
