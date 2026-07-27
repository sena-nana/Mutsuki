use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mutsuki_agent_contracts::{
    AgentError, AgentSessionCheckpoint, AgentSessionEvent, SessionSnapshotRef,
};

#[derive(Clone, Debug, PartialEq)]
pub struct AgentSessionRecovery {
    pub checkpoint: AgentSessionCheckpoint,
    pub events_after_checkpoint: Vec<(u64, AgentSessionEvent)>,
}

pub trait AgentSessionStore: Send + Sync {
    fn append_event(&self, session_id: &str, event: AgentSessionEvent) -> Result<u64, AgentError>;

    fn save_checkpoint(&self, checkpoint: AgentSessionCheckpoint) -> Result<(), AgentError>;

    fn load_checkpoint(
        &self,
        session_id: &str,
    ) -> Result<Option<AgentSessionCheckpoint>, AgentError>;

    fn events_after(
        &self,
        session_id: &str,
        sequence: u64,
        limit: usize,
    ) -> Result<Vec<(u64, AgentSessionEvent)>, AgentError>;

    fn list_sessions(&self, limit: usize) -> Result<Vec<String>, AgentError>;

    fn fork_session(
        &self,
        source_session_id: &str,
        target_session_id: &str,
        snapshot: SessionSnapshotRef,
    ) -> Result<(), AgentError>;

    fn close_session(&self, session_id: &str) -> Result<(), AgentError>;

    fn resume_session(&self, session_id: &str) -> Result<Option<AgentSessionRecovery>, AgentError>;
}

#[derive(Clone, Default)]
pub struct InMemoryAgentSessionStore {
    inner: Arc<Mutex<BTreeMap<String, SessionRecord>>>,
}

#[derive(Default)]
struct SessionRecord {
    events: Vec<AgentSessionEvent>,
    checkpoint: Option<(usize, AgentSessionCheckpoint)>,
    closed: bool,
}

impl AgentSessionStore for InMemoryAgentSessionStore {
    fn append_event(&self, session_id: &str, event: AgentSessionEvent) -> Result<u64, AgentError> {
        validate_session_id(session_id)?;
        let mut sessions = self.inner.lock().expect("agent session store mutex");
        let session = sessions.entry(session_id.into()).or_default();
        if session.closed {
            return Err(AgentError::new(
                "agent.session.closed",
                "cannot append to a closed session",
            ));
        }
        session.events.push(event);
        Ok(session.events.len() as u64)
    }

    fn save_checkpoint(&self, checkpoint: AgentSessionCheckpoint) -> Result<(), AgentError> {
        validate_session_id(&checkpoint.session_id)?;
        if checkpoint.snapshot.session_id != checkpoint.session_id
            || checkpoint.snapshot.version != checkpoint.version
        {
            return Err(AgentError::invalid_input(
                "checkpoint snapshot must belong to the same session and version",
            ));
        }
        let mut sessions = self.inner.lock().expect("agent session store mutex");
        let session = sessions.entry(checkpoint.session_id.clone()).or_default();
        if session.closed {
            return Err(AgentError::new(
                "agent.session.closed",
                "cannot checkpoint a closed session",
            ));
        }
        if session
            .checkpoint
            .as_ref()
            .is_some_and(|(_, current)| current.version.0 > checkpoint.version.0)
        {
            return Err(AgentError::new(
                "agent.session.version_conflict",
                "checkpoint version cannot move backwards",
            ));
        }
        session.checkpoint = Some((session.events.len(), checkpoint));
        Ok(())
    }

    fn load_checkpoint(
        &self,
        session_id: &str,
    ) -> Result<Option<AgentSessionCheckpoint>, AgentError> {
        validate_session_id(session_id)?;
        Ok(self
            .inner
            .lock()
            .expect("agent session store mutex")
            .get(session_id)
            .and_then(|session| session.checkpoint.as_ref())
            .map(|(_, checkpoint)| checkpoint.clone()))
    }

    fn events_after(
        &self,
        session_id: &str,
        sequence: u64,
        limit: usize,
    ) -> Result<Vec<(u64, AgentSessionEvent)>, AgentError> {
        validate_session_id(session_id)?;
        let sessions = self.inner.lock().expect("agent session store mutex");
        let Some(session) = sessions.get(session_id) else {
            return Ok(Vec::new());
        };
        Ok(session
            .events
            .iter()
            .enumerate()
            .skip(sequence as usize)
            .take(limit)
            .map(|(index, event)| ((index + 1) as u64, event.clone()))
            .collect())
    }

    fn list_sessions(&self, limit: usize) -> Result<Vec<String>, AgentError> {
        Ok(self
            .inner
            .lock()
            .expect("agent session store mutex")
            .iter()
            .filter(|(_, session)| !session.closed)
            .take(limit)
            .map(|(session_id, _)| session_id.clone())
            .collect())
    }

    fn fork_session(
        &self,
        source_session_id: &str,
        target_session_id: &str,
        snapshot: SessionSnapshotRef,
    ) -> Result<(), AgentError> {
        validate_session_id(source_session_id)?;
        validate_session_id(target_session_id)?;
        if snapshot.session_id != target_session_id {
            return Err(AgentError::invalid_input(
                "fork snapshot must be rebound to the target session",
            ));
        }
        let mut sessions = self.inner.lock().expect("agent session store mutex");
        if sessions.contains_key(target_session_id) {
            return Err(AgentError::new(
                "agent.session.already_exists",
                "fork target session already exists",
            ));
        }
        let source = sessions
            .get(source_session_id)
            .ok_or_else(|| AgentError::not_found("fork source session was not found"))?;
        let source_checkpoint = source
            .checkpoint
            .as_ref()
            .map(|(_, checkpoint)| checkpoint)
            .ok_or_else(|| AgentError::not_found("fork source checkpoint was not found"))?;
        let checkpoint = AgentSessionCheckpoint {
            session_id: target_session_id.into(),
            profile_id: source_checkpoint.profile_id.clone(),
            version: snapshot.version,
            budget: source_checkpoint.budget.clone(),
            state: snapshot.snapshot.clone(),
            snapshot,
            pending_approvals: Vec::new(),
            plugin_generations: source_checkpoint.plugin_generations.clone(),
            attempts: BTreeMap::new(),
            coordinator: None,
            degraded_reason: None,
        };
        sessions.insert(
            target_session_id.into(),
            SessionRecord {
                events: Vec::new(),
                checkpoint: Some((0, checkpoint)),
                closed: false,
            },
        );
        Ok(())
    }

    fn close_session(&self, session_id: &str) -> Result<(), AgentError> {
        validate_session_id(session_id)?;
        let mut sessions = self.inner.lock().expect("agent session store mutex");
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| AgentError::not_found("session was not found"))?;
        session.closed = true;
        Ok(())
    }

    fn resume_session(&self, session_id: &str) -> Result<Option<AgentSessionRecovery>, AgentError> {
        validate_session_id(session_id)?;
        let sessions = self.inner.lock().expect("agent session store mutex");
        let Some(session) = sessions.get(session_id) else {
            return Ok(None);
        };
        let Some((checkpoint_sequence, checkpoint)) = &session.checkpoint else {
            return Ok(None);
        };
        Ok(Some(AgentSessionRecovery {
            checkpoint: checkpoint.clone(),
            events_after_checkpoint: session
                .events
                .iter()
                .enumerate()
                .skip(*checkpoint_sequence)
                .map(|(index, event)| ((index + 1) as u64, event.clone()))
                .collect(),
        }))
    }
}

fn validate_session_id(session_id: &str) -> Result<(), AgentError> {
    if session_id.trim().is_empty() {
        Err(AgentError::invalid_input("session id is required"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mutsuki_agent_contracts::{ResourceRef, SessionVersion};
    use mutsuki_runtime_contracts::{
        ResourceAccess, ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic,
    };

    use super::*;

    fn resource(session_id: &str, slot: &str, version: u64) -> ResourceRef {
        ResourceRef {
            ref_id: format!("{session_id}:{slot}:{version}"),
            resource_id: ResourceId {
                kind_id: "agent.session".into(),
                slot_id: slot.into(),
                generation: 1,
                version,
            },
            semantic: ResourceSemantic::FrozenValue,
            provider_id: "test".into(),
            resource_kind: "agent.session".into(),
            schema: "agent.session@1".into(),
            version,
            generation: 1,
            access: ResourceAccess::Inline,
            size_hint: None,
            content_hash: None,
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        }
    }

    fn snapshot(session_id: &str, version: u64) -> SessionSnapshotRef {
        SessionSnapshotRef {
            session_id: session_id.into(),
            version: SessionVersion(version),
            snapshot: resource(session_id, "snapshot", version),
            base: None,
            deltas: Vec::new(),
        }
    }

    fn checkpoint(session_id: &str, version: u64) -> AgentSessionCheckpoint {
        AgentSessionCheckpoint {
            session_id: session_id.into(),
            profile_id: "profile".into(),
            version: SessionVersion(version),
            budget: Default::default(),
            state: resource(session_id, "state", version),
            snapshot: snapshot(session_id, version),
            pending_approvals: Vec::new(),
            plugin_generations: BTreeMap::from([("plugin".into(), 7)]),
            attempts: BTreeMap::new(),
            coordinator: None,
            degraded_reason: None,
        }
    }

    #[test]
    fn checkpoint_resume_list_fork_and_close_preserve_recovery_boundaries() {
        let store = InMemoryAgentSessionStore::default();
        store
            .append_event(
                "source",
                AgentSessionEvent::TurnStarted {
                    turn_id: "turn-1".into(),
                },
            )
            .unwrap();
        store.save_checkpoint(checkpoint("source", 1)).unwrap();
        store
            .append_event(
                "source",
                AgentSessionEvent::TurnCompleted {
                    turn_id: "turn-1".into(),
                    result: None,
                },
            )
            .unwrap();

        let recovery = store.resume_session("source").unwrap().unwrap();
        assert_eq!(recovery.checkpoint.version, SessionVersion(1));
        assert_eq!(recovery.events_after_checkpoint.len(), 1);
        assert_eq!(recovery.events_after_checkpoint[0].0, 2);
        assert_eq!(store.list_sessions(10).unwrap(), vec!["source"]);

        store
            .fork_session("source", "fork", snapshot("fork", 1))
            .unwrap();
        let fork = store.resume_session("fork").unwrap().unwrap();
        assert!(fork.events_after_checkpoint.is_empty());
        assert_eq!(fork.checkpoint.plugin_generations["plugin"], 7);
        assert!(fork.checkpoint.pending_approvals.is_empty());

        store.close_session("source").unwrap();
        assert_eq!(store.list_sessions(10).unwrap(), vec!["fork"]);
        assert_eq!(
            store
                .append_event(
                    "source",
                    AgentSessionEvent::TurnCancelled {
                        turn_id: "turn-2".into()
                    }
                )
                .unwrap_err()
                .code,
            "agent.session.closed"
        );
    }
}
