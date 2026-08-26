use serde::{Deserialize, Serialize};

use crate::{
    AgentBudget, AgentEvent, AgentEventEnvelope, AgentMessage, ContextInjection, ContextProvenance,
    CoordinatorLease, PermissionRequest, ResourceCellRef, ResourceRef, SessionVersion,
    apply_injections_to_messages,
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

    pub fn trajectory(&self) -> SessionTrajectory {
        project_session_trajectory(&self.session_id, &self.events)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionTrajectoryRequest {
    pub session_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionTrajectory {
    pub session_id: String,
    pub groups: Vec<SessionTrajectoryGroup>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionTrajectoryGroup {
    pub source_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ContextProvenance>,
    pub items: Vec<SessionTrajectoryItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTrajectoryItem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub summary: String,
}

/// Rebuilds the model-visible prefix from `ContextInjected` events for one turn.
pub fn derive_model_messages(
    events: &[AgentEventEnvelope],
    turn_id: &str,
    transcript: &[AgentMessage],
) -> Vec<AgentMessage> {
    let injections = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            AgentEvent::ContextInjected {
                turn_id: event_turn,
                text,
                provenance,
            } if event_turn == turn_id && !text.trim().is_empty() => {
                Some(ContextInjection::new(text.clone(), provenance.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    apply_injections_to_messages(&injections, transcript)
}

pub fn project_session_trajectory(
    session_id: &str,
    events: &[AgentEventEnvelope],
) -> SessionTrajectory {
    let mut groups: Vec<SessionTrajectoryGroup> = Vec::new();
    for envelope in events {
        let (source_kind, provenance, turn_id, summary) = match &envelope.event {
            AgentEvent::UserMessage {
                turn_id, content, ..
            } => ("user".into(), None, Some(turn_id.clone()), content.clone()),
            AgentEvent::ContextInjected {
                turn_id,
                text,
                provenance,
            } => (
                provenance.source_kind.clone(),
                Some(provenance.clone()),
                Some(turn_id.clone()),
                text.clone(),
            ),
            AgentEvent::ToolCall {
                turn_id,
                name,
                call_id,
                ..
            } => (
                "tool".into(),
                None,
                Some(turn_id.clone()),
                format!("{name} ({call_id})"),
            ),
            AgentEvent::ToolResult {
                turn_id, summary, ..
            }
            | AgentEvent::ToolCallCompleted {
                turn_id, summary, ..
            } => ("tool".into(), None, Some(turn_id.clone()), summary.clone()),
            AgentEvent::FinalResponse {
                turn_id, summary, ..
            } => (
                "assistant".into(),
                None,
                Some(turn_id.clone()),
                summary.clone(),
            ),
            AgentEvent::ModelDelta { turn_id, text } => (
                "assistant".into(),
                None,
                Some(turn_id.clone()),
                text.clone(),
            ),
            _ => continue,
        };
        let item = SessionTrajectoryItem {
            sequence: Some(envelope.sequence),
            turn_id,
            summary,
        };
        if let Some(last) = groups.last_mut()
            && last.source_kind == source_kind
            && last.provenance == provenance
        {
            last.items.push(item);
        } else {
            groups.push(SessionTrajectoryGroup {
                source_kind,
                provenance,
                items: vec![item],
            });
        }
    }
    SessionTrajectory {
        session_id: session_id.to_owned(),
        groups,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentEvent, AgentEventMeta, CONTEXT_SOURCE_ICL, ContextProvenance};

    fn envelope(sequence: u64, event: AgentEvent) -> AgentEventEnvelope {
        AgentEventEnvelope {
            session_id: "session".into(),
            sequence,
            meta: AgentEventMeta::new(format!("e{sequence}"), "test"),
            event,
        }
    }

    #[test]
    fn model_messages_reconstruct_from_context_injected_events() {
        let transcript = vec![AgentMessage::user("hello")];
        let events = vec![
            envelope(
                1,
                AgentEvent::ContextInjected {
                    turn_id: "turn-1".into(),
                    text: "you are a helper".into(),
                    provenance: ContextProvenance::new(
                        "bot.conversation",
                        crate::CONTEXT_SOURCE_PERSONA,
                        "persona",
                        "1",
                    ),
                },
            ),
            envelope(
                2,
                AgentEvent::ContextInjected {
                    turn_id: "turn-1".into(),
                    text: "群聊：alice: hi".into(),
                    provenance: ContextProvenance::new(
                        "bot.conversation",
                        CONTEXT_SOURCE_ICL,
                        "group",
                        "1",
                    ),
                },
            ),
            envelope(
                3,
                AgentEvent::UserMessage {
                    turn_id: "turn-1".into(),
                    content: "hello".into(),
                    metadata: None,
                },
            ),
        ];
        let derived = derive_model_messages(&events, "turn-1", &transcript);
        assert_eq!(derived[0].content, "you are a helper");
        assert_eq!(derived[1].content, "群聊：alice: hi");
        assert_eq!(derived[2].content, "hello");

        let trajectory = project_session_trajectory("session", &events);
        assert_eq!(
            trajectory.groups[0].source_kind,
            crate::CONTEXT_SOURCE_PERSONA
        );
        assert_eq!(trajectory.groups[1].source_kind, CONTEXT_SOURCE_ICL);
        assert_eq!(trajectory.groups[2].source_kind, "user");
    }
}
