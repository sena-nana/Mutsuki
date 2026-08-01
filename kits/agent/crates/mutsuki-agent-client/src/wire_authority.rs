use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mutsuki_agent_contracts::{
    AgentEventEnvelope, AgentEventPage, AgentMessage, AgentSession, AgentSessionCreateRequest,
    AgentWireError, AgentWireRequest, AgentWireRequestEnvelope, AgentWireResponse,
    AgentWireResponseEnvelope, PermissionDecision, PermissionDecisionKind, ResourceRef,
    SessionSnapshotRef, SessionVersion,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::InProcessAgentService;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentWireTurnOutput {
    pub events: Vec<AgentEventEnvelope>,
    pub next_sequence: u64,
    #[serde(default)]
    pub payload: Value,
}

pub trait AgentWireRuntime: Send + Sync {
    fn start_session(
        &self,
        session_id: &str,
        request: AgentSessionCreateRequest,
    ) -> Result<AgentSession, AgentWireError>;
    fn submit_turn(
        &self,
        session_id: &str,
        turn_id: &str,
        messages: &[AgentMessage],
    ) -> Result<AgentWireTurnOutput, AgentWireError>;
    fn cancel_turn(&self, session_id: &str, turn_id: &str) -> Result<(), AgentWireError>;
    fn apply_permission(
        &self,
        decision: &PermissionDecision,
    ) -> Result<AgentWireTurnOutput, AgentWireError>;
    fn events_after(
        &self,
        session_id: &str,
        after_sequence: u64,
    ) -> Result<Vec<AgentEventEnvelope>, AgentWireError>;
    fn fork_session(
        &self,
        source_session_id: &str,
        target_session_id: &str,
    ) -> Result<AgentSession, AgentWireError>;
    fn read_resource(
        &self,
        resource: &ResourceRef,
        offset: u64,
        length: u32,
    ) -> Result<(Vec<u8>, bool), AgentWireError>;
    fn capabilities(&self) -> Result<BTreeMap<String, String>, AgentWireError>;
}

pub trait AgentWireStateStore: Send + Sync {
    fn load(&self) -> Result<Vec<(String, Value)>, AgentWireError>;
    fn store(&self, session_id: &str, state: &Value) -> Result<(), AgentWireError>;
}

#[derive(Clone, Default)]
pub struct InMemoryAgentWireStateStore {
    states: Arc<Mutex<BTreeMap<String, Value>>>,
}

impl AgentWireStateStore for InMemoryAgentWireStateStore {
    fn load(&self) -> Result<Vec<(String, Value)>, AgentWireError> {
        Ok(self
            .states
            .lock()
            .map_err(|_| {
                wire_error(
                    "agent.wire.store_poisoned",
                    "state store lock poisoned",
                    false,
                )
            })?
            .iter()
            .map(|(session_id, state)| (session_id.clone(), state.clone()))
            .collect())
    }

    fn store(&self, session_id: &str, state: &Value) -> Result<(), AgentWireError> {
        self.states
            .lock()
            .map_err(|_| {
                wire_error(
                    "agent.wire.store_poisoned",
                    "state store lock poisoned",
                    false,
                )
            })?
            .insert(session_id.to_string(), state.clone());
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TurnReplay {
    turn_id: String,
    messages: Vec<AgentMessage>,
    version: SessionVersion,
    output: AgentWireTurnOutput,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ApprovalReplay {
    decision: PermissionDecision,
    version: SessionVersion,
    output: AgentWireTurnOutput,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WireSessionState {
    session: AgentSession,
    version: SessionVersion,
    idempotency: BTreeMap<String, TurnReplay>,
    approvals: BTreeMap<String, ApprovalReplay>,
    cancelled_turns: BTreeMap<String, SessionVersion>,
    fork_source: Option<(String, SessionVersion)>,
    closed: bool,
}

pub struct AgentWireAuthority<R, P> {
    runtime: R,
    persistence: P,
    sessions: BTreeMap<String, WireSessionState>,
    next_session: u64,
}

impl<R: AgentWireRuntime, P: AgentWireStateStore> AgentWireAuthority<R, P> {
    pub fn new(runtime: R, persistence: P) -> Result<Self, AgentWireError> {
        let mut sessions = BTreeMap::new();
        for (session_id, value) in persistence.load()? {
            let state: WireSessionState = serde_json::from_value(value).map_err(|error| {
                wire_error(
                    "agent.session.restore_invalid",
                    format!("decode persisted session `{session_id}`: {error}"),
                    false,
                )
            })?;
            if state.session.session_id != session_id {
                return Err(wire_error(
                    "agent.session.restore_identity_mismatch",
                    "persisted session key does not match the session",
                    false,
                ));
            }
            sessions.insert(session_id, state);
        }
        let next_session = u64::try_from(sessions.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        Ok(Self {
            runtime,
            persistence,
            sessions,
            next_session,
        })
    }

    pub fn runtime(&self) -> &R {
        &self.runtime
    }

    pub fn attach_session(
        &mut self,
        session: AgentSession,
    ) -> Result<AgentSession, AgentWireError> {
        if let Some(existing) = self.sessions.get(&session.session_id) {
            if existing.session.profile_id != session.profile_id {
                return Err(wire_error(
                    "agent.session.binding_conflict",
                    "session is already attached with a different profile",
                    false,
                ));
            }
            return Ok(existing.session.clone());
        }
        let session_id = session.session_id.clone();
        self.sessions.insert(
            session_id.clone(),
            WireSessionState {
                session: session.clone(),
                version: SessionVersion(1),
                idempotency: BTreeMap::new(),
                approvals: BTreeMap::new(),
                cancelled_turns: BTreeMap::new(),
                fork_source: None,
                closed: false,
            },
        );
        self.persist(&session_id)?;
        Ok(session)
    }

    pub fn get_session(&self, session_id: &str) -> Result<AgentSession, AgentWireError> {
        self.record(session_id).map(|record| record.session.clone())
    }

    pub fn current_version(&self, session_id: &str) -> Result<SessionVersion, AgentWireError> {
        self.record(session_id).map(|record| record.version)
    }

    pub fn submit(
        &mut self,
        session_id: &str,
        expected_version: SessionVersion,
        turn_id: &str,
        messages: Vec<AgentMessage>,
        idempotency_key: &str,
    ) -> Result<(SessionVersion, AgentWireTurnOutput), AgentWireError> {
        if idempotency_key.trim().is_empty() {
            return Err(wire_error(
                "agent.turn.idempotency_required",
                "idempotency_key must not be empty",
                false,
            ));
        }
        if let Some(replay) = self.record(session_id)?.idempotency.get(idempotency_key) {
            if replay.turn_id == turn_id && replay.messages == messages {
                return Ok((replay.version, replay.output.clone()));
            }
            return Err(wire_error(
                "agent.turn.idempotency_conflict",
                "idempotency_key was already used for a different turn payload",
                false,
            ));
        }
        self.require_open_version(session_id, expected_version)?;
        if !messages
            .iter()
            .any(|message| !message.content.trim().is_empty())
        {
            return Err(wire_error(
                "agent.turn.message_required",
                "turn must contain a non-empty message",
                false,
            ));
        }
        let output = self.runtime.submit_turn(session_id, turn_id, &messages)?;
        let version = SessionVersion(expected_version.0.saturating_add(1));
        let record = self.record_mut(session_id)?;
        record.session.messages.extend(messages.iter().cloned());
        record.session.turn_count = record.session.turn_count.saturating_add(1);
        record.session.events.extend(output.events.iter().cloned());
        record.session.next_event_sequence = output.next_sequence;
        record.session.cell.generation = version.0;
        record.version = version;
        record.idempotency.insert(
            idempotency_key.to_string(),
            TurnReplay {
                turn_id: turn_id.to_string(),
                messages,
                version,
                output: output.clone(),
            },
        );
        self.persist(session_id)?;
        Ok((version, output))
    }

    pub fn apply_permission(
        &mut self,
        decision: PermissionDecision,
    ) -> Result<(SessionVersion, AgentWireTurnOutput), AgentWireError> {
        let session_id = decision.session_id.clone();
        let key = format!("{}:{}", decision.action_id, decision.version);
        if let Some(replay) = self.record(&session_id)?.approvals.get(&key) {
            if replay.decision == decision {
                return Ok((replay.version, replay.output.clone()));
            }
            return Err(wire_error(
                "agent.approval.idempotency_conflict",
                "approval was already resolved with a different decision",
                false,
            ));
        }
        if self.record(&session_id)?.closed {
            return Err(wire_error(
                "agent.session.closed",
                "session is closed",
                false,
            ));
        }
        let output = self.runtime.apply_permission(&decision)?;
        let record = self.record_mut(&session_id)?;
        record.version = SessionVersion(record.version.0.saturating_add(1));
        let version = record.version;
        record.session.events.extend(output.events.iter().cloned());
        record.session.next_event_sequence = output.next_sequence;
        record.session.cell.generation = version.0;
        record.approvals.insert(
            key,
            ApprovalReplay {
                decision,
                version,
                output: output.clone(),
            },
        );
        self.persist(&session_id)?;
        Ok((version, output))
    }

    pub fn events(
        &self,
        session_id: &str,
        after_sequence: u64,
        limit: u32,
    ) -> Result<AgentEventPage, AgentWireError> {
        self.record(session_id)?;
        let events = self
            .runtime
            .events_after(session_id, after_sequence)?
            .into_iter()
            .take(limit as usize)
            .collect::<Vec<_>>();
        Ok(AgentEventPage {
            next_sequence: events
                .last()
                .map(|event| event.sequence)
                .unwrap_or(after_sequence),
            events,
            lost: 0,
            truncated: false,
        })
    }

    pub fn cancel(
        &mut self,
        session_id: &str,
        turn_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError> {
        if let Some(version) = self.record(session_id)?.cancelled_turns.get(turn_id) {
            return Ok(*version);
        }
        self.require_open_version(session_id, expected_version)?;
        self.runtime.cancel_turn(session_id, turn_id)?;
        let record = self.record_mut(session_id)?;
        record.version = SessionVersion(record.version.0.saturating_add(1));
        record.session.cell.generation = record.version.0;
        let version = record.version;
        record.cancelled_turns.insert(turn_id.to_string(), version);
        self.persist(session_id)?;
        Ok(version)
    }

    fn start(
        &mut self,
        mut request: AgentSessionCreateRequest,
    ) -> Result<AgentSession, AgentWireError> {
        let session_id = match request.session_id.clone() {
            Some(session_id) => session_id,
            None => loop {
                let candidate = format!("native-wire-{}", self.next_session);
                self.next_session = self.next_session.checked_add(1).ok_or_else(|| {
                    wire_error(
                        "agent.session.id_exhausted",
                        "session id space exhausted",
                        false,
                    )
                })?;
                if !self.sessions.contains_key(&candidate) {
                    break candidate;
                }
            },
        };
        request.session_id = Some(session_id.clone());
        let session = self.runtime.start_session(&session_id, request)?;
        self.attach_session(session)
    }

    fn fork(
        &mut self,
        source_session_id: &str,
        target_session_id: &str,
        snapshot: &SessionSnapshotRef,
    ) -> Result<SessionVersion, AgentWireError> {
        let source = self.record(source_session_id)?;
        if snapshot.session_id != source_session_id || snapshot.version != source.version {
            return Err(wire_error(
                "agent.session.snapshot_conflict",
                "fork snapshot does not match the current source session",
                true,
            ));
        }
        if self.sessions.contains_key(target_session_id) {
            return Err(wire_error(
                "agent.session.already_exists",
                "fork target already exists",
                false,
            ));
        }
        let source_session = source.session.clone();
        let runtime_session = self
            .runtime
            .fork_session(source_session_id, target_session_id)?;
        let mut session = source_session;
        session.session_id = target_session_id.to_string();
        session.resource = runtime_session.resource;
        session.cell = runtime_session.cell;
        let mut state = WireSessionState {
            session,
            version: SessionVersion(1),
            idempotency: BTreeMap::new(),
            approvals: BTreeMap::new(),
            cancelled_turns: BTreeMap::new(),
            fork_source: Some((source_session_id.to_string(), snapshot.version)),
            closed: false,
        };
        state.session.cell.generation = 1;
        self.sessions.insert(target_session_id.to_string(), state);
        self.persist(target_session_id)?;
        Ok(SessionVersion(1))
    }

    fn record(&self, session_id: &str) -> Result<&WireSessionState, AgentWireError> {
        self.sessions
            .get(session_id)
            .ok_or_else(|| wire_error("agent.session.not_found", "session was not found", false))
    }

    fn record_mut(&mut self, session_id: &str) -> Result<&mut WireSessionState, AgentWireError> {
        self.sessions
            .get_mut(session_id)
            .ok_or_else(|| wire_error("agent.session.not_found", "session was not found", false))
    }

    fn require_open_version(
        &self,
        session_id: &str,
        expected: SessionVersion,
    ) -> Result<(), AgentWireError> {
        let record = self.record(session_id)?;
        if record.closed {
            return Err(wire_error(
                "agent.session.closed",
                "session is closed",
                false,
            ));
        }
        if record.version != expected {
            return Err(wire_error(
                "agent.session.version_conflict",
                format!(
                    "expected session version {}, current version is {}",
                    expected.0, record.version.0
                ),
                true,
            ));
        }
        Ok(())
    }

    fn persist(&self, session_id: &str) -> Result<(), AgentWireError> {
        let value = serde_json::to_value(self.record(session_id)?).map_err(|error| {
            wire_error("agent.session.persist_encode", error.to_string(), false)
        })?;
        self.persistence.store(session_id, &value)
    }

    fn response(
        request_id: u64,
        response: AgentWireResponse,
    ) -> Result<AgentWireResponseEnvelope, AgentWireError> {
        Ok(AgentWireResponseEnvelope {
            request_id,
            response: Ok(response),
        })
    }
}

impl<R: AgentWireRuntime, P: AgentWireStateStore> InProcessAgentService
    for AgentWireAuthority<R, P>
{
    fn dispatch(
        &mut self,
        envelope: AgentWireRequestEnvelope,
    ) -> Result<AgentWireResponseEnvelope, AgentWireError> {
        let request_id = envelope.request_id;
        let response = match envelope.request {
            AgentWireRequest::Negotiate => {
                return Err(wire_error(
                    "agent.wire.invalid_dispatch",
                    "negotiation is handled by the Agent Client dispatcher",
                    false,
                ));
            }
            AgentWireRequest::StartSession { request } => {
                AgentWireResponse::Session(self.start(request)?)
            }
            AgentWireRequest::GetSession { session_id } => {
                AgentWireResponse::Session(self.get_session(&session_id)?)
            }
            AgentWireRequest::SubmitTurn {
                session_id,
                expected_version,
                turn_id,
                messages,
                idempotency_key,
            } => AgentWireResponse::Accepted {
                session_id: session_id.clone(),
                version: self
                    .submit(
                        &session_id,
                        expected_version,
                        &turn_id,
                        messages,
                        &idempotency_key,
                    )?
                    .0,
            },
            AgentWireRequest::CancelTurn {
                session_id,
                turn_id,
                expected_version,
            } => AgentWireResponse::Accepted {
                session_id: session_id.clone(),
                version: self.cancel(&session_id, &turn_id, expected_version)?,
            },
            AgentWireRequest::ApproveAction { decision } => {
                if decision.decision != PermissionDecisionKind::Approved {
                    return Err(wire_error(
                        "agent.approval.invalid_decision",
                        "approve_action requires an approved decision",
                        false,
                    ));
                }
                let session_id = decision.session_id.clone();
                AgentWireResponse::Accepted {
                    session_id,
                    version: self.apply_permission(decision)?.0,
                }
            }
            AgentWireRequest::RejectAction { decision } => {
                if decision.decision != PermissionDecisionKind::Rejected {
                    return Err(wire_error(
                        "agent.approval.invalid_decision",
                        "reject_action requires a rejected decision",
                        false,
                    ));
                }
                let session_id = decision.session_id.clone();
                AgentWireResponse::Accepted {
                    session_id,
                    version: self.apply_permission(decision)?.0,
                }
            }
            AgentWireRequest::SubscribeSessionEvents {
                session_id,
                after_sequence,
                limit,
            } => AgentWireResponse::Events(self.events(&session_id, after_sequence, limit)?),
            AgentWireRequest::ResumeSession {
                session_id,
                after_sequence,
            } => AgentWireResponse::Events(self.events(&session_id, after_sequence, u32::MAX)?),
            AgentWireRequest::ForkSession {
                source_session_id,
                target_session_id,
                snapshot,
            } => AgentWireResponse::Accepted {
                session_id: target_session_id.clone(),
                version: self.fork(&source_session_id, &target_session_id, &snapshot)?,
            },
            AgentWireRequest::CloseSession {
                session_id,
                expected_version,
            } => {
                self.require_open_version(&session_id, expected_version)?;
                self.record_mut(&session_id)?.closed = true;
                self.persist(&session_id)?;
                AgentWireResponse::Closed
            }
            AgentWireRequest::ListSessions {
                after_session_id,
                limit,
            } => {
                let mut ids = self
                    .sessions
                    .keys()
                    .filter(|id| after_session_id.as_ref().is_none_or(|after| *id > after))
                    .take(limit as usize + 1)
                    .cloned()
                    .collect::<Vec<_>>();
                let has_more = ids.len() > limit as usize;
                ids.truncate(limit as usize);
                AgentWireResponse::Sessions {
                    next_session_id: if has_more { ids.last().cloned() } else { None },
                    session_ids: ids,
                }
            }
            AgentWireRequest::ReadResource {
                resource,
                offset,
                length,
            } => {
                let (bytes, eof) = self.runtime.read_resource(&resource, offset, length)?;
                AgentWireResponse::ResourceChunk {
                    resource: *resource,
                    offset,
                    bytes,
                    eof,
                }
            }
            AgentWireRequest::ListRuntimeCapabilities => {
                AgentWireResponse::Capabilities(self.runtime.capabilities()?)
            }
        };
        Self::response(request_id, response)
    }
}

pub fn wire_error(
    code: impl Into<String>,
    message: impl Into<String>,
    retryable: bool,
) -> AgentWireError {
    AgentWireError {
        code: code.into(),
        message: message.into(),
        retryable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentClient, InProcessAgentClient};
    use mutsuki_agent_contracts::{AGENT_WIRE_VERSION, AgentEvent, AgentEventMeta, AgentWireHello};
    use mutsuki_runtime_contracts::{
        ResourceAccess, ResourceCellRef, ResourceId, ResourceLifetime, ResourceSealState,
        ResourceSemantic,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone, Default)]
    struct FakeRuntime {
        sessions: Arc<Mutex<BTreeMap<String, AgentSession>>>,
        events: Arc<Mutex<BTreeMap<String, Vec<AgentEventEnvelope>>>>,
        submits: Arc<AtomicUsize>,
        approvals: Arc<AtomicUsize>,
        cancels: Arc<AtomicUsize>,
    }

    impl AgentWireRuntime for FakeRuntime {
        fn start_session(
            &self,
            session_id: &str,
            request: AgentSessionCreateRequest,
        ) -> Result<AgentSession, AgentWireError> {
            let mut session = AgentSession::new(
                session_id,
                request.profile_id,
                resource(session_id),
                cell(session_id, 1),
            );
            session.title = request.title;
            self.sessions
                .lock()
                .unwrap()
                .insert(session_id.to_string(), session.clone());
            Ok(session)
        }

        fn submit_turn(
            &self,
            session_id: &str,
            turn_id: &str,
            _messages: &[AgentMessage],
        ) -> Result<AgentWireTurnOutput, AgentWireError> {
            self.submits.fetch_add(1, Ordering::SeqCst);
            self.emit(
                session_id,
                AgentEvent::TurnState {
                    turn_id: turn_id.to_string(),
                    status: "completed".into(),
                },
            )
        }

        fn cancel_turn(&self, _session_id: &str, _turn_id: &str) -> Result<(), AgentWireError> {
            self.cancels.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn apply_permission(
            &self,
            decision: &PermissionDecision,
        ) -> Result<AgentWireTurnOutput, AgentWireError> {
            self.approvals.fetch_add(1, Ordering::SeqCst);
            self.emit(
                &decision.session_id,
                AgentEvent::TurnState {
                    turn_id: decision.turn_id.clone(),
                    status: "resumed".into(),
                },
            )
        }

        fn events_after(
            &self,
            session_id: &str,
            after_sequence: u64,
        ) -> Result<Vec<AgentEventEnvelope>, AgentWireError> {
            Ok(self
                .events
                .lock()
                .unwrap()
                .get(session_id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|event| event.sequence > after_sequence)
                .collect())
        }

        fn fork_session(
            &self,
            source_session_id: &str,
            target_session_id: &str,
        ) -> Result<AgentSession, AgentWireError> {
            let mut session = self
                .sessions
                .lock()
                .unwrap()
                .get(source_session_id)
                .cloned()
                .ok_or_else(|| wire_error("agent.session.not_found", "source missing", false))?;
            session.session_id = target_session_id.to_string();
            session.resource = resource(target_session_id);
            session.cell = cell(target_session_id, 1);
            self.sessions
                .lock()
                .unwrap()
                .insert(target_session_id.to_string(), session.clone());
            Ok(session)
        }

        fn read_resource(
            &self,
            _resource: &ResourceRef,
            _offset: u64,
            _length: u32,
        ) -> Result<(Vec<u8>, bool), AgentWireError> {
            Ok((Vec::new(), true))
        }

        fn capabilities(&self) -> Result<BTreeMap<String, String>, AgentWireError> {
            Ok(BTreeMap::from([("backend".into(), "fake".into())]))
        }
    }

    impl FakeRuntime {
        fn emit(
            &self,
            session_id: &str,
            event: AgentEvent,
        ) -> Result<AgentWireTurnOutput, AgentWireError> {
            let mut events = self.events.lock().unwrap();
            let log = events.entry(session_id.to_string()).or_default();
            let sequence = log.len() as u64 + 1;
            let envelope = AgentEventEnvelope {
                session_id: session_id.to_string(),
                sequence,
                meta: AgentEventMeta::new(format!("event-{sequence}"), "test event"),
                event,
            };
            log.push(envelope.clone());
            Ok(AgentWireTurnOutput {
                events: vec![envelope],
                next_sequence: sequence,
                payload: Value::Null,
            })
        }
    }

    fn resource(session_id: &str) -> ResourceRef {
        ResourceRef {
            ref_id: format!("resource:{session_id}"),
            resource_id: ResourceId {
                kind_id: "mutsuki.agent.session".into(),
                slot_id: session_id.into(),
                generation: 1,
                version: 1,
            },
            semantic: ResourceSemantic::VersionedSnapshot,
            provider_id: "test".into(),
            resource_kind: "mutsuki.agent.session".into(),
            schema: "mutsuki.agent.session@1".into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::Inline,
            size_hint: None,
            content_hash: None,
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        }
    }

    fn cell(session_id: &str, generation: u64) -> ResourceCellRef {
        ResourceCellRef {
            cell_id: format!("cell:{session_id}"),
            resource_kind: "mutsuki.agent.session".into(),
            owner_plugin_id: "test".into(),
            schema: "mutsuki.agent.session@1".into(),
            generation,
            health: "ready".into(),
            reload_policy: "persistent".into(),
        }
    }

    fn request() -> AgentSessionCreateRequest {
        AgentSessionCreateRequest {
            session_id: Some("session-1".into()),
            profile_id: "coding".into(),
            title: None,
        }
    }

    fn envelope(request_id: u64, request: AgentWireRequest) -> AgentWireRequestEnvelope {
        AgentWireRequestEnvelope {
            request_id,
            hello: AgentWireHello {
                version: AGENT_WIRE_VERSION,
                required_features: vec!["monotonic-events".into()],
                optional_features: Vec::new(),
            },
            request,
        }
    }

    #[test]
    fn authority_fences_duplicate_turn_approval_and_cancel_execution() {
        let runtime = FakeRuntime::default();
        let store = InMemoryAgentWireStateStore::default();
        let mut authority = AgentWireAuthority::new(runtime.clone(), store).unwrap();
        authority.start(request()).unwrap();
        let messages = vec![AgentMessage::user("hello")];
        let first = authority
            .submit(
                "session-1",
                SessionVersion(1),
                "turn-1",
                messages.clone(),
                "key-1",
            )
            .unwrap();
        let duplicate = authority
            .submit("session-1", SessionVersion(1), "turn-1", messages, "key-1")
            .unwrap();
        assert_eq!(first, duplicate);
        assert_eq!(runtime.submits.load(Ordering::SeqCst), 1);

        let decision = PermissionDecision {
            session_id: "session-1".into(),
            turn_id: "turn-1".into(),
            action_id: "action-1".into(),
            version: 1,
            decision: PermissionDecisionKind::Approved,
        };
        let approval = authority.apply_permission(decision.clone()).unwrap();
        assert_eq!(authority.apply_permission(decision).unwrap(), approval);
        assert_eq!(runtime.approvals.load(Ordering::SeqCst), 1);

        let version = authority.current_version("session-1").unwrap();
        let cancelled = authority.cancel("session-1", "turn-1", version).unwrap();
        assert_eq!(
            authority.cancel("session-1", "turn-1", version).unwrap(),
            cancelled
        );
        assert_eq!(runtime.cancels.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn authority_state_restores_and_fork_starts_independent_version_lineage() {
        let runtime = FakeRuntime::default();
        let store = InMemoryAgentWireStateStore::default();
        {
            let mut authority = AgentWireAuthority::new(runtime.clone(), store.clone()).unwrap();
            authority.start(request()).unwrap();
            authority
                .submit(
                    "session-1",
                    SessionVersion(1),
                    "turn-1",
                    vec![AgentMessage::user("hello")],
                    "key-1",
                )
                .unwrap();
        }
        let mut restored = AgentWireAuthority::new(runtime, store).unwrap();
        assert_eq!(
            restored.current_version("session-1").unwrap(),
            SessionVersion(2)
        );
        let snapshot = SessionSnapshotRef {
            session_id: "session-1".into(),
            version: SessionVersion(2),
            snapshot: resource("session-1"),
            base: None,
            deltas: Vec::new(),
        };
        assert_eq!(
            restored.fork("session-1", "session-2", &snapshot).unwrap(),
            SessionVersion(1)
        );
        assert_eq!(
            restored.current_version("session-2").unwrap(),
            SessionVersion(1)
        );
        assert_eq!(restored.get_session("session-2").unwrap().turn_count, 1);
    }

    #[test]
    fn authority_rejects_permission_methods_with_mismatched_decisions() {
        let runtime = FakeRuntime::default();
        let mut authority =
            AgentWireAuthority::new(runtime.clone(), InMemoryAgentWireStateStore::default())
                .unwrap();
        authority.start(request()).unwrap();
        let mut decision = PermissionDecision {
            session_id: "session-1".into(),
            turn_id: "turn-1".into(),
            action_id: "action-1".into(),
            version: 1,
            decision: PermissionDecisionKind::Rejected,
        };

        let approval = authority.dispatch(envelope(
            1,
            AgentWireRequest::ApproveAction {
                decision: decision.clone(),
            },
        ));
        assert_eq!(
            approval.unwrap_err().code,
            "agent.approval.invalid_decision"
        );

        decision.decision = PermissionDecisionKind::Approved;
        let rejection =
            authority.dispatch(envelope(2, AgentWireRequest::RejectAction { decision }));
        assert_eq!(
            rejection.unwrap_err().code,
            "agent.approval.invalid_decision"
        );
        assert_eq!(runtime.approvals.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn authority_only_returns_a_session_cursor_when_another_page_exists() {
        let mut authority = AgentWireAuthority::new(
            FakeRuntime::default(),
            InMemoryAgentWireStateStore::default(),
        )
        .unwrap();
        for session_id in ["session-1", "session-2", "session-3"] {
            let mut request = request();
            request.session_id = Some(session_id.into());
            authority.start(request).unwrap();
        }

        let first = authority
            .dispatch(envelope(
                1,
                AgentWireRequest::ListSessions {
                    after_session_id: None,
                    limit: 2,
                },
            ))
            .unwrap();
        assert_eq!(
            first.response,
            Ok(AgentWireResponse::Sessions {
                session_ids: vec!["session-1".into(), "session-2".into()],
                next_session_id: Some("session-2".into()),
            })
        );

        let last = authority
            .dispatch(envelope(
                2,
                AgentWireRequest::ListSessions {
                    after_session_id: Some("session-2".into()),
                    limit: 2,
                },
            ))
            .unwrap();
        assert_eq!(
            last.response,
            Ok(AgentWireResponse::Sessions {
                session_ids: vec!["session-3".into()],
                next_session_id: None,
            })
        );
    }

    #[test]
    fn in_process_client_uses_the_same_authority_contract() {
        let authority = AgentWireAuthority::new(
            FakeRuntime::default(),
            InMemoryAgentWireStateStore::default(),
        )
        .unwrap();
        let mut client = AgentClient::new(InProcessAgentClient::new(authority));
        let session = client.start_session(request()).unwrap();
        assert_eq!(session.session_id, "session-1");
        assert_eq!(client.runtime_capabilities().unwrap()["backend"], "fake");
    }
}
