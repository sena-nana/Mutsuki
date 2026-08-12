//! Legacy transcript compatibility owned by the Runtime domain.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mutsuki_agent_contracts::{
    AgentError, AgentEvent, AgentEventEnvelope, AgentMessage, AgentResult, AgentRole, AgentSession,
    AgentSessionAppendRequest, AgentSessionCreateRequest, AgentSessionForkRequest,
    AgentSessionGetRequest, AgentSessionSnapshotRequest,
};
use mutsuki_agent_sdk::{session_cell_ref, session_resource_ref};

const SESSION_OWNER_ID: &str = "mutsuki.agent.runtime.session";
type SessionEventSubscribers = BTreeMap<String, BTreeMap<u64, Sender<Vec<AgentEventEnvelope>>>>;

pub trait SessionPersistence: Send + Sync {
    fn load(&self) -> AgentResult<Vec<AgentSession>>;
    fn store(&self, session: &AgentSession) -> AgentResult<()>;
}

#[derive(Clone)]
pub struct SessionStore {
    inner: Arc<SessionStoreInner>,
}

struct SessionStoreInner {
    next_id: AtomicU64,
    next_subscriber: AtomicU64,
    sessions: Mutex<BTreeMap<String, AgentSession>>,
    subscribers: Mutex<SessionEventSubscribers>,
    persistence: Option<Arc<dyn SessionPersistence>>,
}

pub struct SessionEventSubscription {
    store: SessionStore,
    session_id: String,
    subscriber_id: u64,
    receiver: Receiver<Vec<AgentEventEnvelope>>,
}

impl SessionEventSubscription {
    pub fn next_timeout(&self, timeout: Duration) -> AgentResult<Option<Vec<AgentEventEnvelope>>> {
        match self.receiver.recv_timeout(timeout) {
            Ok(events) => Ok(Some(events)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(AgentError::new(
                "agent.session.event_stream_closed",
                "session event stream closed before the subscription was dropped",
            )),
        }
    }
}

impl Drop for SessionEventSubscription {
    fn drop(&mut self) {
        let Ok(mut subscribers) = self.store.inner.subscribers.lock() else {
            return;
        };
        let Some(session_subscribers) = subscribers.get_mut(&self.session_id) else {
            return;
        };
        session_subscribers.remove(&self.subscriber_id);
        if session_subscribers.is_empty() {
            subscribers.remove(&self.session_id);
        }
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self {
            inner: Arc::new(SessionStoreInner::default()),
        }
    }
}

impl Default for SessionStoreInner {
    fn default() -> Self {
        Self {
            next_id: AtomicU64::new(0),
            next_subscriber: AtomicU64::new(0),
            sessions: Mutex::new(BTreeMap::new()),
            subscribers: Mutex::new(BTreeMap::new()),
            persistence: None,
        }
    }
}

impl SessionStore {
    pub fn with_persistence(persistence: Arc<dyn SessionPersistence>) -> AgentResult<Self> {
        let loaded = persistence.load()?;
        let mut sessions = BTreeMap::new();
        let mut next_id = 0;
        for session in loaded {
            if session.session_id.trim().is_empty() || session.profile_id.trim().is_empty() {
                return Err(AgentError::invalid_input(
                    "persisted sessions require non-empty session_id and profile_id",
                ));
            }
            if sessions
                .insert(session.session_id.clone(), session)
                .is_some()
            {
                return Err(AgentError::invalid_input(
                    "persisted session ids must be unique",
                ));
            }
        }
        for session_id in sessions.keys() {
            if let Some(id) = session_id
                .strip_prefix("agent-session-")
                .and_then(|id| id.parse::<u64>().ok())
            {
                next_id = next_id.max(id);
            }
        }
        Ok(Self {
            inner: Arc::new(SessionStoreInner {
                next_id: AtomicU64::new(next_id),
                next_subscriber: AtomicU64::new(0),
                sessions: Mutex::new(sessions),
                subscribers: Mutex::new(BTreeMap::new()),
                persistence: Some(persistence),
            }),
        })
    }

    pub fn create(&self, request: AgentSessionCreateRequest) -> AgentResult<AgentSession> {
        if request.profile_id.trim().is_empty() {
            return Err(AgentError::invalid_input("profile_id is required"));
        }
        let session_id = match request.session_id {
            Some(session_id) if session_id.trim().is_empty() => {
                return Err(AgentError::invalid_input("session_id must not be empty"));
            }
            Some(session_id) => session_id,
            None => {
                let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed) + 1;
                format!("agent-session-{id}")
            }
        };
        let mut sessions = self
            .inner
            .sessions
            .lock()
            .expect("session store mutex poisoned");
        if let Some(existing) = sessions.get(&session_id) {
            if existing.profile_id == request.profile_id {
                return Ok(existing.clone());
            }
            return Err(AgentError::invalid_input(format!(
                "session `{session_id}` already belongs to profile `{}`",
                existing.profile_id
            )));
        }
        let mut session = AgentSession::new(
            session_id.clone(),
            request.profile_id,
            session_resource_ref(SESSION_OWNER_ID, &session_id),
            session_cell_ref(SESSION_OWNER_ID, &session_id),
        );
        session.title = request.title;
        self.persist(&session)?;
        sessions.insert(session_id, session.clone());
        Ok(session)
    }

    pub fn get(&self, request: AgentSessionGetRequest) -> AgentResult<AgentSession> {
        self.inner
            .sessions
            .lock()
            .expect("session store mutex poisoned")
            .get(&request.session_id)
            .cloned()
            .ok_or_else(|| {
                AgentError::not_found(format!("session `{}` not found", request.session_id))
            })
    }

    pub fn append(&self, request: AgentSessionAppendRequest) -> AgentResult<AgentSession> {
        let session_id = request.session_id.clone();
        let appended_events = request.events.clone();
        let mut sessions = self
            .inner
            .sessions
            .lock()
            .expect("session store mutex poisoned");
        let session = sessions.get(&request.session_id).ok_or_else(|| {
            AgentError::not_found(format!("session `{}` not found", request.session_id))
        })?;
        let mut updated = session.clone();
        updated.messages.extend(request.messages);
        for event in request.events {
            if event.session_id != updated.session_id
                || event.sequence != updated.next_event_sequence.saturating_add(1)
            {
                return Err(AgentError::new(
                    "agent.session.event_sequence",
                    "session events must be contiguous and bind the target session",
                ));
            }
            updated.next_event_sequence = event.sequence;
            updated.events.push(event);
        }
        if request.advance_turn {
            updated.turn_count += 1;
        }
        self.persist(&updated)?;
        sessions.insert(request.session_id, updated.clone());
        drop(sessions);
        self.publish_events(&session_id, &appended_events);
        Ok(updated)
    }

    pub fn subscribe_events(
        &self,
        session_id: &str,
        after_sequence: u64,
    ) -> AgentResult<SessionEventSubscription> {
        let sessions = self
            .inner
            .sessions
            .lock()
            .expect("session store mutex poisoned");
        let replay = sessions
            .get(session_id)
            .ok_or_else(|| AgentError::not_found(format!("session `{session_id}` not found")))?
            .events
            .iter()
            .filter(|event| event.sequence > after_sequence)
            .cloned()
            .collect::<Vec<_>>();
        let subscriber_id = self
            .inner
            .next_subscriber
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let (sender, receiver) = mpsc::channel();
        self.inner
            .subscribers
            .lock()
            .expect("session subscriber mutex poisoned")
            .entry(session_id.to_string())
            .or_default()
            .insert(subscriber_id, sender.clone());
        drop(sessions);
        if !replay.is_empty() {
            let _ = sender.send(replay);
        }
        Ok(SessionEventSubscription {
            store: self.clone(),
            session_id: session_id.to_string(),
            subscriber_id,
            receiver,
        })
    }

    fn publish_events(&self, session_id: &str, events: &[AgentEventEnvelope]) {
        if events.is_empty() {
            return;
        }
        let mut subscribers = self
            .inner
            .subscribers
            .lock()
            .expect("session subscriber mutex poisoned");
        let Some(session_subscribers) = subscribers.get_mut(session_id) else {
            return;
        };
        session_subscribers.retain(|_, sender| sender.send(events.to_vec()).is_ok());
        if session_subscribers.is_empty() {
            subscribers.remove(session_id);
        }
    }

    pub fn snapshot(&self, request: AgentSessionSnapshotRequest) -> AgentResult<AgentSession> {
        self.get(AgentSessionGetRequest {
            session_id: request.session_id,
        })
    }

    pub fn fork(&self, request: AgentSessionForkRequest) -> AgentResult<AgentSession> {
        if request.source_session_id.trim().is_empty()
            || request.target_session_id.trim().is_empty()
        {
            return Err(AgentError::invalid_input(
                "source_session_id and target_session_id are required",
            ));
        }
        let mut sessions = self
            .inner
            .sessions
            .lock()
            .expect("session store mutex poisoned");
        if sessions.contains_key(&request.target_session_id) {
            return Err(AgentError::invalid_input(format!(
                "session `{}` already exists",
                request.target_session_id
            )));
        }
        let source = sessions.get(&request.source_session_id).ok_or_else(|| {
            AgentError::not_found(format!("session `{}` not found", request.source_session_id))
        })?;
        let mut forked = AgentSession::new(
            request.target_session_id.clone(),
            source.profile_id.clone(),
            session_resource_ref(SESSION_OWNER_ID, &request.target_session_id),
            session_cell_ref(SESSION_OWNER_ID, &request.target_session_id),
        );
        forked.title = request.title.or_else(|| source.title.clone());
        let (messages, events, turn_count, next_event_sequence) = request
            .through_turn_id
            .as_deref()
            .map(|turn_id| session_through_turn(source, turn_id))
            .transpose()?
            .unwrap_or_else(|| {
                (
                    source.messages.clone(),
                    source.events.clone(),
                    source.turn_count,
                    source.next_event_sequence,
                )
            });
        forked.messages = messages;
        forked.turn_count = turn_count;
        forked.events = events
            .iter()
            .cloned()
            .map(|mut event| {
                event.session_id = request.target_session_id.clone();
                event
            })
            .collect();
        forked.next_event_sequence = next_event_sequence;
        self.persist(&forked)?;
        sessions.insert(request.target_session_id, forked.clone());
        Ok(forked)
    }

    fn persist(&self, session: &AgentSession) -> AgentResult<()> {
        if let Some(persistence) = &self.inner.persistence {
            persistence.store(session)?;
        }
        Ok(())
    }
}

fn session_through_turn(
    source: &AgentSession,
    through_turn_id: &str,
) -> AgentResult<(Vec<AgentMessage>, Vec<AgentEventEnvelope>, u64, u64)> {
    let through_turn_id = through_turn_id.trim();
    if through_turn_id.is_empty() {
        return Err(AgentError::invalid_input(
            "through_turn_id must not be empty",
        ));
    }
    let next_event_sequence = source
        .events
        .iter()
        .filter(|event| event.meta.turn_id.as_deref() == Some(through_turn_id))
        .map(|event| event.sequence)
        .max()
        .ok_or_else(|| AgentError::not_found(format!("turn `{through_turn_id}` not found")))?;
    let events = source
        .events
        .iter()
        .filter(|event| event.sequence <= next_event_sequence)
        .cloned()
        .collect::<Vec<_>>();
    let turn_count = events
        .iter()
        .filter(|event| matches!(event.event, AgentEvent::UserMessage { .. }))
        .count() as u64;
    if turn_count == 0 {
        return Err(AgentError::invalid_input(format!(
            "turn `{through_turn_id}` has no user-message boundary"
        )));
    }
    let mut seen_user_messages = 0_u64;
    let mut messages = Vec::new();
    for message in &source.messages {
        if message.role == AgentRole::User {
            if seen_user_messages == turn_count {
                break;
            }
            seen_user_messages = seen_user_messages.saturating_add(1);
        }
        messages.push(message.clone());
    }
    if seen_user_messages != turn_count {
        return Err(AgentError::invalid_input(format!(
            "turn `{through_turn_id}` transcript boundary is incomplete"
        )));
    }
    Ok((messages, events, turn_count, next_event_sequence))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_agent_contracts::AgentEventMeta;

    #[derive(Default)]
    struct MemoryPersistence {
        sessions: Mutex<BTreeMap<String, AgentSession>>,
        fail_store: std::sync::atomic::AtomicBool,
    }

    impl SessionPersistence for MemoryPersistence {
        fn load(&self) -> AgentResult<Vec<AgentSession>> {
            Ok(self.sessions.lock().unwrap().values().cloned().collect())
        }

        fn store(&self, session: &AgentSession) -> AgentResult<()> {
            if self.fail_store.load(std::sync::atomic::Ordering::Acquire) {
                return Err(AgentError::new(
                    "agent.session.persistence_failed",
                    "injected persistence failure",
                ));
            }
            self.sessions
                .lock()
                .unwrap()
                .insert(session.session_id.clone(), session.clone());
            Ok(())
        }
    }

    #[test]
    fn explicit_migration_session_id_is_idempotent_and_profile_bound() {
        let store = SessionStore::default();
        let request = AgentSessionCreateRequest {
            session_id: Some("agentkit-from-legacy:claude:session-1".into()),
            profile_id: "coding".into(),
            title: Some("Migrated".into()),
        };
        let created = store.create(request.clone()).unwrap();
        let replayed = store.create(request).unwrap();
        assert_eq!(created.session_id, replayed.session_id);
        assert_eq!(created.resource, replayed.resource);

        let conflict = store
            .create(AgentSessionCreateRequest {
                session_id: Some(created.session_id),
                profile_id: "different".into(),
                title: None,
            })
            .unwrap_err();
        assert_eq!(conflict.code, "agent.invalid_input");
    }

    #[test]
    fn persistence_restores_transcript_events_and_fork_lineage() {
        let persistence = Arc::new(MemoryPersistence::default());
        let store = SessionStore::with_persistence(persistence.clone()).unwrap();
        let session = store
            .create(AgentSessionCreateRequest {
                session_id: Some("session-1".into()),
                profile_id: "coding".into(),
                title: None,
            })
            .unwrap();
        let event = mutsuki_agent_contracts::AgentEventEnvelope {
            session_id: session.session_id.clone(),
            sequence: 1,
            meta: Default::default(),
            event: mutsuki_agent_contracts::AgentEvent::TurnState {
                turn_id: "turn-1".into(),
                status: "completed".into(),
            },
        };
        store
            .append(AgentSessionAppendRequest {
                session_id: session.session_id.clone(),
                messages: vec![mutsuki_agent_contracts::AgentMessage::user("hello")],
                events: vec![event],
                advance_turn: true,
            })
            .unwrap();

        let restored = SessionStore::with_persistence(persistence).unwrap();
        let snapshot = restored
            .snapshot(AgentSessionSnapshotRequest {
                session_id: session.session_id.clone(),
            })
            .unwrap();
        assert_eq!(snapshot.messages.len(), 1);
        assert_eq!(snapshot.events.len(), 1);
        assert_eq!(snapshot.next_event_sequence, 1);
        let fork = restored
            .fork(AgentSessionForkRequest {
                source_session_id: session.session_id,
                target_session_id: "session-2".into(),
                title: None,
                through_turn_id: None,
            })
            .unwrap();
        assert_eq!(fork.messages.len(), 1);
        assert_eq!(fork.events[0].session_id, "session-2");
    }

    #[test]
    fn fork_can_stop_at_a_durable_turn_boundary() {
        let store = SessionStore::default();
        let session = store
            .create(AgentSessionCreateRequest {
                session_id: Some("session-source".into()),
                profile_id: "coding".into(),
                title: None,
            })
            .unwrap();
        for (sequence, turn_id, content) in [(1, "turn-1", "first"), (2, "turn-2", "second")] {
            store
                .append(AgentSessionAppendRequest {
                    session_id: session.session_id.clone(),
                    messages: vec![
                        AgentMessage::user(content),
                        AgentMessage::assistant(format!("answer-{content}")),
                    ],
                    events: vec![AgentEventEnvelope {
                        session_id: session.session_id.clone(),
                        sequence,
                        meta: AgentEventMeta::new(format!("event-{sequence}"), "turn")
                            .with_turn(turn_id),
                        event: AgentEvent::UserMessage {
                            turn_id: turn_id.into(),
                            content: content.into(),
                            metadata: None,
                        },
                    }],
                    advance_turn: true,
                })
                .unwrap();
        }

        let fork = store
            .fork(AgentSessionForkRequest {
                source_session_id: session.session_id,
                target_session_id: "session-fork".into(),
                title: None,
                through_turn_id: Some("turn-1".into()),
            })
            .unwrap();
        assert_eq!(fork.turn_count, 1);
        assert_eq!(fork.messages.len(), 2);
        assert_eq!(fork.messages[0].content, "first");
        assert_eq!(fork.events.len(), 1);
        assert_eq!(fork.events[0].session_id, "session-fork");
        assert_eq!(fork.next_event_sequence, 1);
    }

    #[test]
    fn persistence_failure_does_not_publish_partial_transcript_state() {
        let persistence = Arc::new(MemoryPersistence::default());
        let store = SessionStore::with_persistence(persistence.clone()).unwrap();
        let session = store
            .create(AgentSessionCreateRequest {
                session_id: Some("session-1".into()),
                profile_id: "coding".into(),
                title: None,
            })
            .unwrap();
        persistence
            .fail_store
            .store(true, std::sync::atomic::Ordering::Release);
        assert!(
            store
                .append(AgentSessionAppendRequest {
                    session_id: session.session_id.clone(),
                    messages: vec![mutsuki_agent_contracts::AgentMessage::user("lost")],
                    events: Vec::new(),
                    advance_turn: true,
                })
                .is_err()
        );
        assert!(
            store
                .get(AgentSessionGetRequest {
                    session_id: session.session_id,
                })
                .unwrap()
                .messages
                .is_empty()
        );
    }

    #[test]
    fn event_subscription_delivers_committed_events_and_replays_from_sequence() {
        let store = SessionStore::default();
        let session = store
            .create(AgentSessionCreateRequest {
                session_id: Some("session-stream".into()),
                profile_id: "coding".into(),
                title: None,
            })
            .unwrap();
        let subscription = store.subscribe_events(&session.session_id, 0).unwrap();
        let event = |sequence, status: &str| AgentEventEnvelope {
            session_id: session.session_id.clone(),
            sequence,
            meta: Default::default(),
            event: mutsuki_agent_contracts::AgentEvent::TurnState {
                turn_id: "turn-1".into(),
                status: status.into(),
            },
        };
        store
            .append(AgentSessionAppendRequest {
                session_id: session.session_id.clone(),
                messages: Vec::new(),
                events: vec![event(1, "running")],
                advance_turn: false,
            })
            .unwrap();
        assert_eq!(
            subscription
                .next_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap()[0]
                .sequence,
            1
        );
        drop(subscription);
        store
            .append(AgentSessionAppendRequest {
                session_id: session.session_id.clone(),
                messages: Vec::new(),
                events: vec![event(2, "completed")],
                advance_turn: true,
            })
            .unwrap();
        let resumed = store.subscribe_events(&session.session_id, 1).unwrap();
        assert_eq!(
            resumed
                .next_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap()[0]
                .sequence,
            2
        );
    }
}
