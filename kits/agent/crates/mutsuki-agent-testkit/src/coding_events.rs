use std::sync::Mutex;

use mutsuki_agent_contracts::{
    AgentEvent, AgentEventEnvelope, AgentEventMeta, AgentEventPage, ArtifactRef, CodingCommandRef,
    DocumentVersion, EditorDocumentRef, EditorWorkspaceRef, FileChangeDescriptor, FileChangeStatus,
    PlanState, PlanStep, PlanStepStatus, ResourceRef, TodoItem, TodoItemStatus, TodoState,
};
use mutsuki_runtime_contracts::{
    ResourceAccess, ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic,
};

/// Deterministic coding-event recorder for wire projection / timeline replay tests.
#[derive(Debug, Default)]
pub struct CodingEventLog {
    session_id: String,
    sequence: Mutex<u64>,
    events: Mutex<Vec<AgentEventEnvelope>>,
}

impl CodingEventLog {
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            sequence: Mutex::new(0),
            events: Mutex::new(Vec::new()),
        }
    }

    pub fn emit(&self, meta: AgentEventMeta, event: AgentEvent) -> AgentEventEnvelope {
        let mut sequence = self.sequence.lock().expect("coding event sequence");
        *sequence += 1;
        let envelope = AgentEventEnvelope {
            session_id: self.session_id.clone(),
            sequence: *sequence,
            meta,
            event,
        };
        self.events
            .lock()
            .expect("coding event log")
            .push(envelope.clone());
        envelope
    }

    pub fn page(&self, after_sequence: u64) -> AgentEventPage {
        let events = self
            .events
            .lock()
            .expect("coding event log")
            .iter()
            .filter(|event| event.sequence > after_sequence)
            .cloned()
            .collect::<Vec<_>>();
        let next_sequence = events
            .last()
            .map(|event| event.sequence)
            .unwrap_or(after_sequence);
        AgentEventPage {
            events,
            next_sequence,
            lost: 0,
            truncated: false,
        }
    }

    pub fn events(&self) -> Vec<AgentEventEnvelope> {
        self.events.lock().expect("coding event log").clone()
    }
}

fn details_ref(slot: &str) -> ResourceRef {
    ResourceRef {
        ref_id: format!("coding-details:{slot}"),
        resource_id: ResourceId {
            kind_id: "mutsuki.agent.coding.details".into(),
            slot_id: slot.into(),
            generation: 1,
            version: 1,
        },
        semantic: ResourceSemantic::FrozenValue,
        provider_id: "mutsuki.agent.testkit.coding_events".into(),
        resource_kind: "mutsuki.agent.coding.details".into(),
        schema: "mutsuki.agent.coding.details@1".into(),
        version: 1,
        generation: 1,
        access: ResourceAccess::Blob {
            store_id: "coding-fake".into(),
            key: slot.into(),
        },
        size_hint: Some(256),
        content_hash: None,
        lifetime: ResourceLifetime::BorrowedUntilTaskEnd,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}

/// Emits a deterministic coding-run event sequence covering #125 acceptance kinds.
pub fn emit_deterministic_coding_run(log: &CodingEventLog, turn_id: &str) {
    let workspace = EditorWorkspaceRef {
        workspace_id: "ws".into(),
        folders: vec!["/workspace".into()],
        metadata: serde_json::json!({}),
    };
    let document = EditorDocumentRef {
        workspace_id: "ws".into(),
        uri: "file:///workspace/main.rs".into(),
    };

    log.emit(
        AgentEventMeta::new("evt-plan", "plan updated").with_turn(turn_id),
        AgentEvent::PlanUpdated {
            turn_id: turn_id.into(),
            plan: PlanState {
                plan_id: "plan-1".into(),
                revision: 1,
                source: Some("tool".into()),
                steps: vec![PlanStep {
                    step_id: "s1".into(),
                    title: "edit main".into(),
                    status: PlanStepStatus::InProgress,
                    detail: None,
                }],
            },
        },
    );
    log.emit(
        AgentEventMeta::new("evt-todo", "todo updated").with_turn(turn_id),
        AgentEvent::TodoUpdated {
            turn_id: turn_id.into(),
            todo: TodoState {
                todo_id: "todo-1".into(),
                revision: 1,
                items: vec![TodoItem {
                    item_id: "t1".into(),
                    title: "apply patch".into(),
                    status: TodoItemStatus::Pending,
                    priority: 1,
                    relation: Some("plan-1/s1".into()),
                }],
            },
        },
    );
    log.emit(
        AgentEventMeta::new("evt-cmd-start", "command started").with_turn(turn_id),
        AgentEvent::CommandStarted {
            turn_id: turn_id.into(),
            command: CodingCommandRef {
                command_id: "cmd-1".into(),
                command: "cargo".into(),
                args: vec!["check".into()],
                cwd: Some("/workspace".into()),
            },
        },
    );
    log.emit(
        AgentEventMeta::new("evt-cmd-exit", "command exited").with_turn(turn_id),
        AgentEvent::CommandExited {
            turn_id: turn_id.into(),
            command_id: "cmd-1".into(),
            exit_code: 0,
            summary: "cargo check ok".into(),
        },
    );

    let change = FileChangeDescriptor {
        change_id: "chg-1".into(),
        workspace: workspace.clone(),
        document: document.clone(),
        base_version: DocumentVersion(3),
        status: FileChangeStatus::Proposed,
        summary: "update main".into(),
        edits: Vec::new(),
        details: Some(details_ref("diff-1")),
        rejection_reason: None,
    };
    log.emit(
        AgentEventMeta {
            details: change.details.clone(),
            ..AgentEventMeta::new("evt-file", "file change proposed").with_turn(turn_id)
        },
        AgentEvent::FileChangeProposed {
            turn_id: turn_id.into(),
            change: change.clone(),
        },
    );
    let mut applied = change;
    applied.status = FileChangeStatus::Applied;
    log.emit(
        AgentEventMeta::new("evt-file-applied", "file change applied").with_turn(turn_id),
        AgentEvent::FileChangeApplied {
            turn_id: turn_id.into(),
            change: applied,
        },
    );
    log.emit(
        AgentEventMeta::new("evt-artifact", "artifact produced").with_turn(turn_id),
        AgentEvent::ArtifactProduced {
            turn_id: turn_id.into(),
            artifact: ArtifactRef {
                artifact_id: "art-1".into(),
                media_type: "text/plain".into(),
                summary: "check log".into(),
                content_ref: Some(details_ref("artifact-1")),
                kind: Some("report".into()),
                size_bytes: Some(128),
                content_hash: Some("sha256:demo".into()),
                provenance: Some("command:cmd-1".into()),
                open_hint: Some("preview".into()),
                action_hint: Some("open".into()),
            },
        },
    );
    log.emit(
        AgentEventMeta::new("evt-final", "final response").with_turn(turn_id),
        AgentEvent::FinalResponse {
            turn_id: turn_id.into(),
            summary: "done".into(),
            result: None,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_coding_run_emits_required_event_kinds() {
        let log = CodingEventLog::new("session-1");
        emit_deterministic_coding_run(&log, "turn-1");
        let page = log.page(0);
        assert_eq!(page.events.len(), 8);
        assert_eq!(page.next_sequence, 8);
        let kinds = page
            .events
            .iter()
            .map(|event| match &event.event {
                AgentEvent::PlanUpdated { .. } => "plan",
                AgentEvent::TodoUpdated { .. } => "todo",
                AgentEvent::CommandStarted { .. } => "command_started",
                AgentEvent::CommandExited { .. } => "command_exited",
                AgentEvent::FileChangeProposed { .. } => "file_proposed",
                AgentEvent::FileChangeApplied { .. } => "file_applied",
                AgentEvent::ArtifactProduced { .. } => "artifact",
                AgentEvent::FinalResponse { .. } => "final",
                other => panic!("unexpected event: {other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                "plan",
                "todo",
                "command_started",
                "command_exited",
                "file_proposed",
                "file_applied",
                "artifact",
                "final",
            ]
        );
        // Replay from mid-sequence stays monotonic and gap-free for the page window.
        let resumed = log.page(4);
        assert_eq!(resumed.events.first().unwrap().sequence, 5);
        assert!(
            page.events
                .iter()
                .all(|event| !event.meta.event_id.is_empty())
        );
    }

    #[test]
    fn performance_smoke_emit_and_page() {
        let log = CodingEventLog::new("session-perf");
        let started = std::time::Instant::now();
        for index in 0..500 {
            emit_deterministic_coding_run(&log, &format!("turn-{index}"));
        }
        let page = log.page(0);
        assert_eq!(page.events.len(), 4000);
        assert!(
            started.elapsed().as_millis() < 750,
            "coding event smoke exceeded budget"
        );
    }
}
