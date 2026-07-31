use std::collections::{BTreeMap, BTreeSet};
use std::thread;
use std::time::{Duration, Instant};

use mutsuki_agent_contracts::{
    AGENT_WIRE_SUPPORTED_FEATURES, AGENT_WIRE_VERSION, AgentEventEnvelope, AgentEventPage,
    AgentSession, AgentSessionCreateRequest, AgentWireError, AgentWireHello, AgentWireNegotiation,
    AgentWireRequest, AgentWireRequestEnvelope, AgentWireResponse, AgentWireResponseEnvelope,
    PermissionDecision, ResourceRef, SessionSnapshotRef, SessionVersion,
};
use mutsuki_link_core::{
    Connection, ProtocolId, RequestReplay, TransportError, TransportErrorKind,
};

pub const AGENT_LINK_PROTOCOL_ID: &str = "mutsuki.agent.wire";
pub const DEFAULT_LINK_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
pub const MAX_EVENT_PAGE_SIZE: u32 = 1_000;
pub const MAX_RESOURCE_CHUNK_SIZE: u32 = 4 * 1024 * 1024;

pub trait AgentClientBackend {
    fn request(
        &mut self,
        request: AgentWireRequestEnvelope,
    ) -> Result<AgentWireResponseEnvelope, AgentWireError>;
}

pub trait InProcessAgentService {
    fn dispatch(
        &mut self,
        request: AgentWireRequestEnvelope,
    ) -> Result<AgentWireResponseEnvelope, AgentWireError>;
}

pub struct InProcessAgentClient<S> {
    service: S,
}

impl<S> InProcessAgentClient<S> {
    pub const fn new(service: S) -> Self {
        Self { service }
    }

    pub fn into_service(self) -> S {
        self.service
    }
}

impl<S: InProcessAgentService> AgentClientBackend for InProcessAgentClient<S> {
    fn request(
        &mut self,
        request: AgentWireRequestEnvelope,
    ) -> Result<AgentWireResponseEnvelope, AgentWireError> {
        dispatch_agent_request(&mut self.service, request)
    }
}

pub struct AgentClient<B> {
    backend: B,
    next_request_id: u64,
    hello: AgentWireHello,
    negotiation: Option<AgentWireNegotiation>,
}

impl<B: AgentClientBackend> AgentClient<B> {
    pub fn new(backend: B) -> Self {
        Self::with_hello(
            backend,
            AgentWireHello {
                version: AGENT_WIRE_VERSION,
                required_features: vec!["monotonic-events".into()],
                optional_features: vec![
                    "approval-binding".into(),
                    "event-resume".into(),
                    "resource-ref".into(),
                ],
            },
        )
    }

    pub fn with_hello(backend: B, hello: AgentWireHello) -> Self {
        Self {
            backend,
            next_request_id: 1,
            hello,
            negotiation: None,
        }
    }

    pub fn negotiate(&mut self) -> Result<&AgentWireNegotiation, AgentWireError> {
        if self.negotiation.is_none() {
            let response = self.dispatch(AgentWireRequest::Negotiate)?;
            let AgentWireResponse::Negotiated(negotiation) = response else {
                return Err(protocol_error(
                    "agent.wire.unexpected_response",
                    "Negotiate did not return a negotiation response",
                    false,
                ));
            };
            if negotiation.version != self.hello.version
                || self
                    .hello
                    .required_features
                    .iter()
                    .any(|feature| !negotiation.enabled_features.contains(feature))
            {
                return Err(protocol_error(
                    "agent.wire.negotiation_mismatch",
                    "server negotiation does not satisfy client requirements",
                    false,
                ));
            }
            self.negotiation = Some(negotiation);
        }
        Ok(self.negotiation.as_ref().expect("negotiation was set"))
    }

    pub fn start_session(
        &mut self,
        request: AgentSessionCreateRequest,
    ) -> Result<AgentSession, AgentWireError> {
        self.ensure_negotiated()?;
        match self.dispatch(AgentWireRequest::StartSession { request })? {
            AgentWireResponse::Session(session) => Ok(session),
            _ => Err(unexpected("StartSession")),
        }
    }

    pub fn get_session(&mut self, session_id: &str) -> Result<AgentSession, AgentWireError> {
        self.ensure_negotiated()?;
        match self.dispatch(AgentWireRequest::GetSession {
            session_id: session_id.into(),
        })? {
            AgentWireResponse::Session(session) => Ok(session),
            _ => Err(unexpected("GetSession")),
        }
    }

    pub fn submit_turn(
        &mut self,
        session_id: &str,
        expected_version: SessionVersion,
        turn_id: &str,
        messages: Vec<mutsuki_agent_contracts::AgentMessage>,
        idempotency_key: &str,
    ) -> Result<SessionVersion, AgentWireError> {
        self.ensure_negotiated()?;
        accepted_version(
            self.dispatch(AgentWireRequest::SubmitTurn {
                session_id: session_id.into(),
                expected_version,
                turn_id: turn_id.into(),
                messages,
                idempotency_key: idempotency_key.into(),
            })?,
            session_id,
            "SubmitTurn",
        )
    }

    pub fn cancel_turn(
        &mut self,
        session_id: &str,
        turn_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError> {
        self.ensure_negotiated()?;
        accepted_version(
            self.dispatch(AgentWireRequest::CancelTurn {
                session_id: session_id.into(),
                turn_id: turn_id.into(),
                expected_version,
            })?,
            session_id,
            "CancelTurn",
        )
    }

    pub fn approve_action(
        &mut self,
        decision: PermissionDecision,
    ) -> Result<SessionVersion, AgentWireError> {
        let session_id = decision.session_id.clone();
        self.ensure_negotiated()?;
        accepted_version(
            self.dispatch(AgentWireRequest::ApproveAction { decision })?,
            &session_id,
            "ApproveAction",
        )
    }

    pub fn reject_action(
        &mut self,
        decision: PermissionDecision,
    ) -> Result<SessionVersion, AgentWireError> {
        let session_id = decision.session_id.clone();
        self.ensure_negotiated()?;
        accepted_version(
            self.dispatch(AgentWireRequest::RejectAction { decision })?,
            &session_id,
            "RejectAction",
        )
    }

    pub fn subscribe_session_events(
        &mut self,
        session_id: &str,
        after_sequence: u64,
        limit: u32,
    ) -> Result<AgentEventPage, AgentWireError> {
        self.ensure_negotiated()?;
        match self.dispatch(AgentWireRequest::SubscribeSessionEvents {
            session_id: session_id.into(),
            after_sequence,
            limit,
        })? {
            AgentWireResponse::Events(events) => Ok(events),
            _ => Err(unexpected("SubscribeSessionEvents")),
        }
    }

    pub fn resume_session_events(
        &mut self,
        session_id: &str,
        after_sequence: u64,
    ) -> Result<AgentEventPage, AgentWireError> {
        self.ensure_negotiated()?;
        match self.dispatch(AgentWireRequest::ResumeSession {
            session_id: session_id.into(),
            after_sequence,
        })? {
            AgentWireResponse::Events(events) => Ok(events),
            _ => Err(unexpected("ResumeSession")),
        }
    }

    pub fn fork_session(
        &mut self,
        source_session_id: &str,
        target_session_id: &str,
        snapshot: SessionSnapshotRef,
    ) -> Result<SessionVersion, AgentWireError> {
        self.ensure_negotiated()?;
        accepted_version(
            self.dispatch(AgentWireRequest::ForkSession {
                source_session_id: source_session_id.into(),
                target_session_id: target_session_id.into(),
                snapshot: Box::new(snapshot),
            })?,
            target_session_id,
            "ForkSession",
        )
    }

    pub fn close_session(
        &mut self,
        session_id: &str,
        expected_version: SessionVersion,
    ) -> Result<(), AgentWireError> {
        self.ensure_negotiated()?;
        match self.dispatch(AgentWireRequest::CloseSession {
            session_id: session_id.into(),
            expected_version,
        })? {
            AgentWireResponse::Closed => Ok(()),
            _ => Err(unexpected("CloseSession")),
        }
    }

    pub fn list_sessions(
        &mut self,
        after_session_id: Option<String>,
        limit: u32,
    ) -> Result<(Vec<String>, Option<String>), AgentWireError> {
        self.ensure_negotiated()?;
        match self.dispatch(AgentWireRequest::ListSessions {
            after_session_id,
            limit,
        })? {
            AgentWireResponse::Sessions {
                session_ids,
                next_session_id,
            } => Ok((session_ids, next_session_id)),
            _ => Err(unexpected("ListSessions")),
        }
    }

    pub fn read_resource(
        &mut self,
        resource: ResourceRef,
        offset: u64,
        length: u32,
    ) -> Result<(Vec<u8>, bool), AgentWireError> {
        self.ensure_negotiated()?;
        let expected_ref = resource.ref_id.clone();
        match self.dispatch(AgentWireRequest::ReadResource {
            resource: Box::new(resource),
            offset,
            length,
        })? {
            AgentWireResponse::ResourceChunk {
                resource,
                offset: returned_offset,
                bytes,
                eof,
            } if resource.ref_id == expected_ref && returned_offset == offset => Ok((bytes, eof)),
            AgentWireResponse::ResourceChunk { .. } => Err(protocol_error(
                "agent.resource.chunk_mismatch",
                "resource chunk identity or offset does not match the request",
                false,
            )),
            _ => Err(unexpected("ReadResource")),
        }
    }

    pub fn runtime_capabilities(&mut self) -> Result<BTreeMap<String, String>, AgentWireError> {
        self.ensure_negotiated()?;
        match self.dispatch(AgentWireRequest::ListRuntimeCapabilities)? {
            AgentWireResponse::Capabilities(capabilities) => Ok(capabilities),
            _ => Err(unexpected("ListRuntimeCapabilities")),
        }
    }

    pub fn into_backend(self) -> B {
        self.backend
    }

    fn ensure_negotiated(&mut self) -> Result<(), AgentWireError> {
        self.negotiate().map(|_| ())
    }

    fn dispatch(&mut self, request: AgentWireRequest) -> Result<AgentWireResponse, AgentWireError> {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.checked_add(1).ok_or_else(|| {
            protocol_error(
                "agent.wire.request_id_exhausted",
                "agent request id space is exhausted",
                false,
            )
        })?;
        let envelope = self.backend.request(AgentWireRequestEnvelope {
            request_id,
            hello: self.hello.clone(),
            request,
        })?;
        if envelope.request_id != request_id {
            return Err(protocol_error(
                "agent.wire.response_id_mismatch",
                "response request id does not match the request",
                false,
            ));
        }
        envelope.response
    }
}

fn accepted_version(
    response: AgentWireResponse,
    expected_session_id: &str,
    operation: &str,
) -> Result<SessionVersion, AgentWireError> {
    match response {
        AgentWireResponse::Accepted {
            session_id,
            version,
        } if session_id == expected_session_id => Ok(version),
        AgentWireResponse::Accepted { .. } => Err(protocol_error(
            "agent.wire.session_id_mismatch",
            format!("{operation} response belongs to a different session"),
            false,
        )),
        _ => Err(unexpected(operation)),
    }
}

#[derive(Clone, Debug)]
struct PendingRequest {
    replay: RequestReplay,
    bytes: Vec<u8>,
}

pub struct AgentLinkClient<C> {
    connection: C,
    next_request_id: u64,
    pending: BTreeMap<u64, PendingRequest>,
    buffered: BTreeMap<u64, AgentWireResponseEnvelope>,
    response_timeout: Duration,
}

impl<C: Connection> AgentLinkClient<C> {
    pub fn new(connection: C) -> Self {
        Self {
            connection,
            next_request_id: 1,
            pending: BTreeMap::new(),
            buffered: BTreeMap::new(),
            response_timeout: DEFAULT_LINK_RESPONSE_TIMEOUT,
        }
    }

    pub fn with_response_timeout(mut self, timeout: Duration) -> Self {
        self.response_timeout = timeout;
        self
    }

    pub fn send(&mut self, request: AgentWireRequest) -> Result<u64, AgentLinkError> {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.checked_add(1).ok_or_else(|| {
            AgentLinkError::Protocol("agent request id space is exhausted".into())
        })?;
        self.send_envelope(AgentWireRequestEnvelope {
            request_id,
            hello: default_hello(),
            request,
        })?;
        Ok(request_id)
    }

    pub fn send_envelope(
        &mut self,
        envelope: AgentWireRequestEnvelope,
    ) -> Result<(), AgentLinkError> {
        if self.pending.contains_key(&envelope.request_id)
            || self.buffered.contains_key(&envelope.request_id)
        {
            return Err(AgentLinkError::Protocol(
                "request id is already pending".into(),
            ));
        }
        let replay = replay_policy(&envelope.request);
        let bytes = serde_json::to_vec(&envelope)
            .map_err(|error| AgentLinkError::Protocol(error.to_string()))?;
        self.send_bytes(&bytes)?;
        self.pending
            .insert(envelope.request_id, PendingRequest { replay, bytes });
        Ok(())
    }

    pub fn try_receive(&mut self) -> Result<Option<AgentWireResponseEnvelope>, AgentLinkError> {
        if let Some(request_id) = self.buffered.keys().next().copied() {
            return Ok(self.buffered.remove(&request_id));
        }
        let Some(bytes) = try_receive_control_message(&mut self.connection)? else {
            return Ok(None);
        };
        let envelope: AgentWireResponseEnvelope = serde_json::from_slice(&bytes)
            .map_err(|error| AgentLinkError::Protocol(error.to_string()))?;
        if self.pending.remove(&envelope.request_id).is_none() {
            return Err(AgentLinkError::Protocol(
                "response request id is unknown or duplicated".into(),
            ));
        }
        Ok(Some(envelope))
    }

    pub fn pending_replay(&self, request_id: u64) -> Option<RequestReplay> {
        self.pending.get(&request_id).map(|pending| pending.replay)
    }

    pub fn reconnect(&mut self, connection: C) -> Result<AgentReconnectReport, AgentLinkError> {
        self.connection = connection;
        self.buffered.clear();
        let mut report = AgentReconnectReport::default();
        let request_ids = self.pending.keys().copied().collect::<Vec<_>>();
        for request_id in request_ids {
            let pending = self
                .pending
                .get(&request_id)
                .expect("pending request id came from the map")
                .clone();
            match pending.replay {
                RequestReplay::Idempotent => {
                    self.send_bytes(&pending.bytes)?;
                    report.replayed.push(request_id);
                }
                RequestReplay::ApplicationDecides => {
                    report.requires_application_decision.push(request_id);
                }
                RequestReplay::Never => {
                    self.pending.remove(&request_id);
                    report.abandoned.push(request_id);
                }
            }
        }
        Ok(report)
    }

    pub fn retry_pending(&mut self, request_id: u64) -> Result<(), AgentLinkError> {
        let bytes = self
            .pending
            .get(&request_id)
            .ok_or_else(|| AgentLinkError::Protocol("pending request was not found".into()))?
            .bytes
            .clone();
        self.send_bytes(&bytes)
    }

    pub fn reconnect_resume_request(
        session_id: impl Into<String>,
        last_seen_sequence: u64,
    ) -> AgentWireRequest {
        AgentWireRequest::ResumeSession {
            session_id: session_id.into(),
            after_sequence: last_seen_sequence,
        }
    }

    pub fn take_connection(self) -> C {
        self.connection
    }

    fn send_bytes(&mut self, bytes: &[u8]) -> Result<(), AgentLinkError> {
        self.connection
            .open_control_stream(protocol_id()?)?
            .try_send(bytes)?;
        Ok(())
    }

    fn receive_for(
        &mut self,
        request_id: u64,
    ) -> Result<AgentWireResponseEnvelope, AgentLinkError> {
        if let Some(response) = self.buffered.remove(&request_id) {
            return Ok(response);
        }
        let deadline = Instant::now() + self.response_timeout;
        loop {
            if let Some(response) = self.try_receive()? {
                if response.request_id == request_id {
                    return Ok(response);
                }
                self.buffered.insert(response.request_id, response);
            }
            if Instant::now() >= deadline {
                return Err(AgentLinkError::Timeout(request_id));
            }
            thread::yield_now();
        }
    }
}

impl<C: Connection> AgentClientBackend for AgentLinkClient<C> {
    fn request(
        &mut self,
        request: AgentWireRequestEnvelope,
    ) -> Result<AgentWireResponseEnvelope, AgentWireError> {
        let request_id = request.request_id;
        self.send_envelope(request).map_err(AgentWireError::from)?;
        self.receive_for(request_id).map_err(AgentWireError::from)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentReconnectReport {
    pub replayed: Vec<u64>,
    pub requires_application_decision: Vec<u64>,
    pub abandoned: Vec<u64>,
}

pub struct AgentLinkServer<C, S> {
    connection: C,
    service: S,
}

impl<C: Connection, S: InProcessAgentService> AgentLinkServer<C, S> {
    pub const fn new(connection: C, service: S) -> Self {
        Self {
            connection,
            service,
        }
    }

    pub fn serve_once(&mut self) -> Result<bool, AgentLinkError> {
        let Some(bytes) = try_receive_control_message(&mut self.connection)? else {
            return Ok(false);
        };
        let request: AgentWireRequestEnvelope = serde_json::from_slice(&bytes)
            .map_err(|error| AgentLinkError::Protocol(error.to_string()))?;
        let request_id = request.request_id;
        let response = dispatch_agent_request(&mut self.service, request).unwrap_or_else(|error| {
            AgentWireResponseEnvelope {
                request_id,
                response: Err(error),
            }
        });
        let bytes = serde_json::to_vec(&response)
            .map_err(|error| AgentLinkError::Protocol(error.to_string()))?;
        self.connection
            .open_control_stream(protocol_id()?)?
            .try_send(&bytes)?;
        Ok(true)
    }

    pub fn into_parts(self) -> (C, S) {
        (self.connection, self.service)
    }
}

pub fn dispatch_agent_request<S: InProcessAgentService>(
    service: &mut S,
    request: AgentWireRequestEnvelope,
) -> Result<AgentWireResponseEnvelope, AgentWireError> {
    validate_envelope(&request)?;
    if matches!(&request.request, AgentWireRequest::Negotiate) {
        let negotiation = negotiate(&request.hello)?;
        return Ok(AgentWireResponseEnvelope {
            request_id: request.request_id,
            response: Ok(AgentWireResponse::Negotiated(negotiation)),
        });
    }
    let request_id = request.request_id;
    match service.dispatch(request) {
        Ok(response) if response.request_id == request_id => Ok(response),
        Ok(_) => Err(protocol_error(
            "agent.wire.response_id_mismatch",
            "service response request id does not match the request",
            false,
        )),
        Err(error) => Ok(AgentWireResponseEnvelope {
            request_id,
            response: Err(error),
        }),
    }
}

fn validate_envelope(request: &AgentWireRequestEnvelope) -> Result<(), AgentWireError> {
    if request.request_id == 0 {
        return Err(protocol_error(
            "agent.wire.invalid_request_id",
            "request id must be non-zero",
            false,
        ));
    }
    negotiate(&request.hello)?;
    let non_empty = |value: &str, field: &str| {
        if value.trim().is_empty() {
            Err(protocol_error(
                "agent.wire.invalid_request",
                format!("{field} must not be empty"),
                false,
            ))
        } else {
            Ok(())
        }
    };
    match &request.request {
        AgentWireRequest::Negotiate | AgentWireRequest::ListRuntimeCapabilities => Ok(()),
        AgentWireRequest::StartSession { request } => non_empty(&request.profile_id, "profile_id"),
        AgentWireRequest::GetSession { session_id }
        | AgentWireRequest::CloseSession { session_id, .. } => non_empty(session_id, "session_id"),
        AgentWireRequest::SubmitTurn {
            session_id,
            turn_id,
            ..
        }
        | AgentWireRequest::CancelTurn {
            session_id,
            turn_id,
            ..
        } => {
            non_empty(session_id, "session_id")?;
            non_empty(turn_id, "turn_id")
        }
        AgentWireRequest::ApproveAction { decision }
        | AgentWireRequest::RejectAction { decision } => {
            non_empty(&decision.session_id, "session_id")?;
            non_empty(&decision.turn_id, "turn_id")?;
            non_empty(&decision.action_id, "action_id")
        }
        AgentWireRequest::SubscribeSessionEvents {
            session_id, limit, ..
        } => {
            non_empty(session_id, "session_id")?;
            if *limit == 0 || *limit > MAX_EVENT_PAGE_SIZE {
                Err(protocol_error(
                    "agent.wire.event_limit",
                    "event page limit must be between 1 and 1000",
                    false,
                ))
            } else {
                Ok(())
            }
        }
        AgentWireRequest::ResumeSession { session_id, .. } => non_empty(session_id, "session_id"),
        AgentWireRequest::ForkSession {
            source_session_id,
            target_session_id,
            snapshot,
        } => {
            non_empty(source_session_id, "source_session_id")?;
            non_empty(target_session_id, "target_session_id")?;
            if snapshot.session_id != *target_session_id {
                Err(protocol_error(
                    "agent.wire.snapshot_session_mismatch",
                    "fork snapshot must be bound to the target session",
                    false,
                ))
            } else {
                Ok(())
            }
        }
        AgentWireRequest::ListSessions { limit, .. } => {
            if *limit == 0 || *limit > MAX_EVENT_PAGE_SIZE {
                Err(protocol_error(
                    "agent.wire.session_limit",
                    "session page limit must be between 1 and 1000",
                    false,
                ))
            } else {
                Ok(())
            }
        }
        AgentWireRequest::ReadResource {
            resource, length, ..
        } => {
            non_empty(&resource.ref_id, "resource.ref_id")?;
            if *length == 0 || *length > MAX_RESOURCE_CHUNK_SIZE {
                Err(protocol_error(
                    "agent.wire.resource_chunk_limit",
                    "resource chunk length must be between 1 byte and 4 MiB",
                    false,
                ))
            } else {
                Ok(())
            }
        }
    }
}

fn negotiate(hello: &AgentWireHello) -> Result<AgentWireNegotiation, AgentWireError> {
    if hello.version != AGENT_WIRE_VERSION {
        return Err(protocol_error(
            "agent.wire.unsupported_version",
            format!(
                "wire version {} is unsupported; expected {AGENT_WIRE_VERSION}",
                hello.version
            ),
            false,
        ));
    }
    let supported = AGENT_WIRE_SUPPORTED_FEATURES
        .into_iter()
        .collect::<BTreeSet<_>>();
    if let Some(feature) = hello
        .required_features
        .iter()
        .find(|feature| !supported.contains(feature.as_str()))
    {
        return Err(protocol_error(
            "agent.wire.unsupported_feature",
            format!("required feature `{feature}` is unsupported"),
            false,
        ));
    }
    let mut enabled = hello
        .required_features
        .iter()
        .chain(hello.optional_features.iter())
        .filter(|feature| supported.contains(feature.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    enabled.sort();
    enabled.dedup();
    Ok(AgentWireNegotiation {
        version: AGENT_WIRE_VERSION,
        enabled_features: enabled,
    })
}

#[derive(Debug)]
pub enum AgentLinkError {
    Transport(TransportError),
    Protocol(String),
    Timeout(u64),
}

impl From<TransportError> for AgentLinkError {
    fn from(value: TransportError) -> Self {
        Self::Transport(value)
    }
}

impl From<AgentLinkError> for AgentWireError {
    fn from(value: AgentLinkError) -> Self {
        match value {
            AgentLinkError::Transport(error) => {
                protocol_error("agent.link.transport", format!("{error:?}"), true)
            }
            AgentLinkError::Protocol(message) => {
                protocol_error("agent.link.protocol", message, false)
            }
            AgentLinkError::Timeout(request_id) => protocol_error(
                "agent.link.timeout",
                format!("request {request_id} timed out"),
                true,
            ),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AgentEventSequence {
    last_seen: u64,
}

impl AgentEventSequence {
    pub const fn after(last_seen: u64) -> Self {
        Self { last_seen }
    }

    pub fn observe(&mut self, event: &AgentEventEnvelope) -> Result<(), AgentWireError> {
        let expected = self.last_seen.saturating_add(1);
        if event.sequence != expected {
            return Err(protocol_error(
                if event.sequence <= self.last_seen {
                    "agent.event.duplicate_or_out_of_order"
                } else {
                    "agent.event.gap"
                },
                format!(
                    "expected event sequence {expected}, received {}",
                    event.sequence
                ),
                true,
            ));
        }
        self.last_seen = event.sequence;
        Ok(())
    }

    pub const fn last_seen(&self) -> u64 {
        self.last_seen
    }
}

#[derive(Clone, Debug)]
pub struct AgentEventCursor {
    session_id: String,
    page_size: u32,
    sequence: AgentEventSequence,
}

impl AgentEventCursor {
    pub fn new(
        session_id: impl Into<String>,
        after_sequence: u64,
        page_size: u32,
    ) -> Result<Self, AgentWireError> {
        if page_size == 0 || page_size > MAX_EVENT_PAGE_SIZE {
            return Err(protocol_error(
                "agent.wire.event_limit",
                "event page limit must be between 1 and 1000",
                false,
            ));
        }
        Ok(Self {
            session_id: session_id.into(),
            page_size,
            sequence: AgentEventSequence::after(after_sequence),
        })
    }

    pub fn poll<B: AgentClientBackend>(
        &mut self,
        client: &mut AgentClient<B>,
    ) -> Result<Vec<AgentEventEnvelope>, AgentWireError> {
        let page = client.subscribe_session_events(
            &self.session_id,
            self.sequence.last_seen(),
            self.page_size,
        )?;
        self.observe_page(page)
    }

    pub fn resume<B: AgentClientBackend>(
        &mut self,
        client: &mut AgentClient<B>,
    ) -> Result<Vec<AgentEventEnvelope>, AgentWireError> {
        let page = client.resume_session_events(&self.session_id, self.sequence.last_seen())?;
        self.observe_page(page)
    }

    pub const fn last_seen(&self) -> u64 {
        self.sequence.last_seen()
    }

    fn observe_page(
        &mut self,
        page: AgentEventPage,
    ) -> Result<Vec<AgentEventEnvelope>, AgentWireError> {
        if page.lost > 0 || page.truncated {
            return Err(protocol_error(
                "agent.event.history_lost",
                format!("event history lost {} entries", page.lost),
                true,
            ));
        }
        for event in &page.events {
            if event.session_id != self.session_id {
                return Err(protocol_error(
                    "agent.event.session_mismatch",
                    "event belongs to a different session",
                    false,
                ));
            }
            self.sequence.observe(event)?;
        }
        if page.next_sequence != self.sequence.last_seen() {
            return Err(protocol_error(
                "agent.event.cursor_mismatch",
                "server event cursor does not match the observed sequence",
                true,
            ));
        }
        Ok(page.events)
    }
}

fn protocol_id() -> Result<ProtocolId, AgentLinkError> {
    ProtocolId::new(AGENT_LINK_PROTOCOL_ID)
        .map_err(|error| AgentLinkError::Protocol(error.to_string()))
}

fn try_receive_control_message<C: Connection>(
    connection: &mut C,
) -> Result<Option<Vec<u8>>, AgentLinkError> {
    match connection
        .open_control_stream(protocol_id()?)?
        .try_receive()
    {
        Ok(message) => Ok(message),
        Err(error) if error.kind == TransportErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn replay_policy(request: &AgentWireRequest) -> RequestReplay {
    match request {
        AgentWireRequest::Negotiate
        | AgentWireRequest::GetSession { .. }
        | AgentWireRequest::SubscribeSessionEvents { .. }
        | AgentWireRequest::ResumeSession { .. }
        | AgentWireRequest::ListSessions { .. }
        | AgentWireRequest::ReadResource { .. }
        | AgentWireRequest::ListRuntimeCapabilities => RequestReplay::Idempotent,
        AgentWireRequest::SubmitTurn {
            idempotency_key, ..
        } if !idempotency_key.trim().is_empty() => RequestReplay::ApplicationDecides,
        _ => RequestReplay::Never,
    }
}

fn default_hello() -> AgentWireHello {
    AgentWireHello {
        version: AGENT_WIRE_VERSION,
        required_features: vec!["monotonic-events".into()],
        optional_features: vec![
            "approval-binding".into(),
            "event-resume".into(),
            "resource-ref".into(),
        ],
    }
}

fn unexpected(operation: &str) -> AgentWireError {
    protocol_error(
        "agent.wire.unexpected_response",
        format!("{operation} returned an unexpected response type"),
        false,
    )
}

fn protocol_error(
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
    use mutsuki_agent_contracts::{
        AgentEvent, AgentMessage, AgentSessionCreateRequest, PermissionDecisionKind,
        ResourceCellRef,
    };
    use mutsuki_link_core::{ConnectContext, TransportBudget};
    use mutsuki_link_core::{EndpointId, MemoryTransportConfig, memory_transport_pair};
    use mutsuki_link_local::{LocalAddress, LocalListener, connect as connect_local};
    use mutsuki_runtime_contracts::{
        ResourceAccess, ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic,
    };

    use super::*;

    #[derive(Default)]
    struct TestService;

    impl InProcessAgentService for TestService {
        fn dispatch(
            &mut self,
            request: AgentWireRequestEnvelope,
        ) -> Result<AgentWireResponseEnvelope, AgentWireError> {
            let response = match request.request {
                AgentWireRequest::StartSession { request } => AgentWireResponse::Session(
                    AgentSession::new("session", request.profile_id, resource(), cell()),
                ),
                AgentWireRequest::GetSession { session_id } => AgentWireResponse::Session(
                    AgentSession::new(session_id, "profile", resource(), cell()),
                ),
                AgentWireRequest::SubmitTurn {
                    session_id,
                    expected_version,
                    ..
                }
                | AgentWireRequest::CancelTurn {
                    session_id,
                    expected_version,
                    ..
                } => AgentWireResponse::Accepted {
                    session_id,
                    version: SessionVersion(expected_version.0 + 1),
                },
                AgentWireRequest::ApproveAction { decision }
                | AgentWireRequest::RejectAction { decision } => AgentWireResponse::Accepted {
                    session_id: decision.session_id,
                    version: SessionVersion(decision.version + 1),
                },
                AgentWireRequest::SubscribeSessionEvents {
                    session_id,
                    after_sequence,
                    ..
                }
                | AgentWireRequest::ResumeSession {
                    session_id,
                    after_sequence,
                } => {
                    let sequence = after_sequence + 1;
                    AgentWireResponse::Events(AgentEventPage {
                        events: vec![AgentEventEnvelope {
                            session_id,
                            sequence,
                            meta: Default::default(),
                            event: AgentEvent::Cancelled {
                                turn_id: "turn".into(),
                            },
                        }],
                        next_sequence: sequence,
                        lost: 0,
                        truncated: false,
                    })
                }
                AgentWireRequest::ListRuntimeCapabilities => AgentWireResponse::Capabilities(
                    BTreeMap::from([("host".into(), "in-process".into())]),
                ),
                AgentWireRequest::ForkSession {
                    target_session_id,
                    snapshot,
                    ..
                } => AgentWireResponse::Accepted {
                    session_id: target_session_id,
                    version: snapshot.version,
                },
                AgentWireRequest::CloseSession { .. } => AgentWireResponse::Closed,
                AgentWireRequest::ListSessions { .. } => AgentWireResponse::Sessions {
                    session_ids: vec!["session".into()],
                    next_session_id: None,
                },
                AgentWireRequest::ReadResource {
                    resource, offset, ..
                } => AgentWireResponse::ResourceChunk {
                    resource: *resource,
                    offset,
                    bytes: b"value".to_vec(),
                    eof: true,
                },
                other => {
                    return Err(protocol_error(
                        "agent.test.unsupported",
                        format!("unsupported test request: {other:?}"),
                        false,
                    ));
                }
            };
            Ok(AgentWireResponseEnvelope {
                request_id: request.request_id,
                response: Ok(response),
            })
        }
    }

    struct ProcessService {
        process_id: u32,
    }

    impl InProcessAgentService for ProcessService {
        fn dispatch(
            &mut self,
            request: AgentWireRequestEnvelope,
        ) -> Result<AgentWireResponseEnvelope, AgentWireError> {
            if matches!(&request.request, AgentWireRequest::ListRuntimeCapabilities) {
                return Ok(AgentWireResponseEnvelope {
                    request_id: request.request_id,
                    response: Ok(AgentWireResponse::Capabilities(BTreeMap::from([(
                        "server_process_id".into(),
                        self.process_id.to_string(),
                    )]))),
                });
            }
            TestService.dispatch(request)
        }
    }

    fn endpoint(value: u8) -> EndpointId {
        EndpointId::from_bytes([value; 16])
    }

    fn resource() -> ResourceRef {
        ResourceRef {
            ref_id: "agent:test:1".into(),
            resource_id: ResourceId {
                kind_id: "agent.test".into(),
                slot_id: "test".into(),
                generation: 1,
                version: 1,
            },
            semantic: ResourceSemantic::FrozenValue,
            provider_id: "test".into(),
            resource_kind: "agent.test".into(),
            schema: "agent.test@1".into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::Inline,
            size_hint: Some(5),
            content_hash: None,
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        }
    }

    fn cell() -> ResourceCellRef {
        ResourceCellRef {
            cell_id: "agent:test".into(),
            resource_kind: "agent.test".into(),
            owner_plugin_id: "test".into(),
            schema: "agent.test@1".into(),
            generation: 1,
            health: "ready".into(),
            reload_policy: "drain".into(),
        }
    }

    fn snapshot(session_id: &str) -> SessionSnapshotRef {
        SessionSnapshotRef {
            session_id: session_id.into(),
            version: SessionVersion(1),
            snapshot: resource(),
            base: None,
            deltas: Vec::new(),
        }
    }

    fn decision(kind: PermissionDecisionKind) -> PermissionDecision {
        PermissionDecision {
            session_id: "session".into(),
            turn_id: "turn".into(),
            action_id: "action".into(),
            version: 2,
            decision: kind,
        }
    }

    fn submit_turn() -> AgentWireRequest {
        AgentWireRequest::SubmitTurn {
            session_id: "session".into(),
            expected_version: SessionVersion(1),
            turn_id: "turn".into(),
            messages: vec![AgentMessage::user("hello")],
            idempotency_key: "turn-key".into(),
        }
    }

    fn envelope(request_id: u64, request: AgentWireRequest) -> AgentWireRequestEnvelope {
        AgentWireRequestEnvelope {
            request_id,
            hello: default_hello(),
            request,
        }
    }

    #[test]
    fn in_process_facade_negotiates_and_streams_monotonic_events() {
        let backend = InProcessAgentClient::new(TestService);
        let mut client = AgentClient::new(backend);
        assert!(
            client
                .negotiate()
                .unwrap()
                .enabled_features
                .contains(&"resource-ref".into())
        );
        assert_eq!(
            client
                .start_session(AgentSessionCreateRequest {
                    profile_id: "profile".into(),
                    title: None,
                })
                .unwrap()
                .session_id,
            "session"
        );
        assert_eq!(client.get_session("session").unwrap().profile_id, "profile");
        assert_eq!(
            client
                .submit_turn(
                    "session",
                    SessionVersion(1),
                    "turn",
                    vec![AgentMessage::user("hello")],
                    "turn-key",
                )
                .unwrap(),
            SessionVersion(2)
        );
        assert_eq!(
            client
                .cancel_turn("session", "turn", SessionVersion(2))
                .unwrap(),
            SessionVersion(3)
        );
        assert_eq!(
            client
                .approve_action(decision(PermissionDecisionKind::Approved))
                .unwrap(),
            SessionVersion(3)
        );
        assert_eq!(
            client
                .reject_action(decision(PermissionDecisionKind::Rejected))
                .unwrap(),
            SessionVersion(3)
        );
        assert_eq!(
            client
                .fork_session("session", "fork", snapshot("fork"))
                .unwrap(),
            SessionVersion(1)
        );
        assert_eq!(client.list_sessions(None, 10).unwrap().0, vec!["session"]);
        assert_eq!(
            client.read_resource(resource(), 0, 5).unwrap(),
            (b"value".to_vec(), true)
        );
        let mut cursor = AgentEventCursor::new("session", 0, 10).unwrap();
        let events = cursor.poll(&mut client).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(cursor.last_seen(), 1);
        let resumed = cursor.resume(&mut client).unwrap();
        assert_eq!(resumed[0].sequence, 2);
        assert_eq!(client.runtime_capabilities().unwrap()["host"], "in-process");
        client.close_session("session", SessionVersion(3)).unwrap();
    }

    #[test]
    fn generic_facade_round_trips_over_memory_link() {
        let (left, right) =
            memory_transport_pair(endpoint(10), endpoint(11), MemoryTransportConfig::default());
        let server = thread::spawn(move || {
            let mut server = AgentLinkServer::new(right, TestService);
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut handled = 0;
            while handled < 4 {
                if server.serve_once().unwrap() {
                    handled += 1;
                } else {
                    assert!(Instant::now() < deadline, "Link server timed out");
                    thread::yield_now();
                }
            }
        });
        let backend = AgentLinkClient::new(left).with_response_timeout(Duration::from_secs(2));
        let mut client = AgentClient::new(backend);
        client.negotiate().unwrap();
        assert_eq!(
            client
                .submit_turn(
                    "session",
                    SessionVersion(1),
                    "turn",
                    vec![AgentMessage::user("hello")],
                    "turn-key",
                )
                .unwrap(),
            SessionVersion(2)
        );
        let mut cursor = AgentEventCursor::new("session", 0, 10).unwrap();
        assert_eq!(cursor.poll(&mut client).unwrap()[0].sequence, 1);
        assert_eq!(client.runtime_capabilities().unwrap()["host"], "in-process");
        server.join().unwrap();
    }

    #[test]
    fn local_link_round_trip_crosses_process_boundary() {
        const CHILD_ENV: &str = "MUTSUKI_AGENT_LINK_CROSS_PROCESS_CHILD";
        const ADDRESS_ENV: &str = "MUTSUKI_AGENT_LINK_CROSS_PROCESS_ADDRESS";
        const READY_ENV: &str = "MUTSUKI_AGENT_LINK_CROSS_PROCESS_READY";
        if std::env::var_os(CHILD_ENV).is_some() {
            let address = LocalAddress(std::env::var(ADDRESS_ENV).expect("child address"));
            let ready = std::path::PathBuf::from(std::env::var_os(READY_ENV).expect("ready path"));
            let runtime = tokio::runtime::Runtime::new().expect("child runtime");
            runtime.block_on(async move {
                let budget = TransportBudget {
                    idle_timeout: None,
                    ..TransportBudget::default()
                };
                let listener = LocalListener::bind(&address, endpoint(42), budget)
                    .expect("child listener binds");
                std::fs::write(&ready, b"ready").expect("child readiness written");
                let connection = listener
                    .accept(endpoint(41))
                    .await
                    .expect("child accepts client");
                let mut server = AgentLinkServer::new(
                    connection,
                    ProcessService {
                        process_id: std::process::id(),
                    },
                );
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut handled = 0;
                while handled < 3 {
                    if server.serve_once().expect("child serves Agent request") {
                        handled += 1;
                    } else {
                        assert!(Instant::now() < deadline, "child Agent server timed out");
                        tokio::task::yield_now().await;
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            });
            return;
        }

        let nonce = format!("{}-{}", std::process::id(), now_test_nanos());
        let address = format!("mutsuki-agent-link-{nonce}");
        let ready = std::env::temp_dir().join(format!("mutsuki-agent-link-{nonce}.ready"));
        let mut child =
            std::process::Command::new(std::env::current_exe().expect("current test executable"))
                .arg("--exact")
                .arg("tests::local_link_round_trip_crosses_process_boundary")
                .arg("--nocapture")
                .env(CHILD_ENV, "1")
                .env(ADDRESS_ENV, &address)
                .env(READY_ENV, &ready)
                .spawn()
                .expect("cross-process Agent server starts");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            assert!(
                Instant::now() < deadline,
                "cross-process Agent server did not become ready"
            );
            thread::yield_now();
        }

        let runtime = tokio::runtime::Runtime::new().expect("client runtime");
        let connection = runtime
            .block_on(connect_local(
                &LocalAddress(address),
                endpoint(41),
                endpoint(42),
                TransportBudget {
                    idle_timeout: None,
                    ..TransportBudget::default()
                },
                &ConnectContext::default(),
            ))
            .expect("client connects over local MutsukiLink");
        let backend =
            AgentLinkClient::new(connection).with_response_timeout(Duration::from_secs(5));
        let mut client = AgentClient::new(backend);
        let session = client
            .start_session(AgentSessionCreateRequest {
                profile_id: "cross-process".into(),
                title: None,
            })
            .expect("session starts across process boundary");
        assert_eq!(session.profile_id, "cross-process");
        let server_process_id = client
            .runtime_capabilities()
            .expect("capabilities cross process")["server_process_id"]
            .parse::<u32>()
            .expect("server PID is numeric");
        assert_ne!(server_process_id, std::process::id());
        assert!(
            child.wait().expect("child wait").success(),
            "cross-process Agent server failed"
        );
        std::fs::remove_file(&ready).expect("readiness file removed");
    }

    fn now_test_nanos() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos()
    }

    #[test]
    fn link_server_uses_same_dispatch_and_reconnect_never_replays_turns() {
        let (left, right) =
            memory_transport_pair(endpoint(1), endpoint(2), MemoryTransportConfig::default());
        let mut client = AgentLinkClient::new(left);
        let mut server = AgentLinkServer::new(right, TestService);

        client
            .send_envelope(envelope(1, AgentWireRequest::Negotiate))
            .unwrap();
        assert!(server.serve_once().unwrap());
        assert!(matches!(
            client.try_receive().unwrap().unwrap().response,
            Ok(AgentWireResponse::Negotiated(_))
        ));

        let read_id = client
            .send(AgentWireRequest::GetSession {
                session_id: "session".into(),
            })
            .unwrap();
        let turn_id = client.send(submit_turn()).unwrap();
        let cancel_id = client
            .send(AgentWireRequest::CancelTurn {
                session_id: "session".into(),
                turn_id: "turn".into(),
                expected_version: SessionVersion(1),
            })
            .unwrap();
        assert_eq!(
            client.pending_replay(read_id),
            Some(RequestReplay::Idempotent)
        );
        assert_eq!(
            client.pending_replay(turn_id),
            Some(RequestReplay::ApplicationDecides)
        );
        assert_eq!(client.pending_replay(cancel_id), Some(RequestReplay::Never));

        let (replacement, mut peer) =
            memory_transport_pair(endpoint(3), endpoint(4), MemoryTransportConfig::default());
        let report = client.reconnect(replacement).unwrap();
        assert_eq!(report.replayed, vec![read_id]);
        assert_eq!(report.requires_application_decision, vec![turn_id]);
        assert_eq!(report.abandoned, vec![cancel_id]);
        let replayed = try_receive_control_message(&mut peer).unwrap().unwrap();
        let replayed: AgentWireRequestEnvelope = serde_json::from_slice(&replayed).unwrap();
        assert_eq!(replayed.request_id, read_id);
        assert!(try_receive_control_message(&mut peer).unwrap().is_none());
    }

    #[test]
    fn security_limits_feature_negotiation_and_event_gaps_are_rejected() {
        let mut service = TestService;
        let unsupported = dispatch_agent_request(
            &mut service,
            AgentWireRequestEnvelope {
                request_id: 1,
                hello: AgentWireHello {
                    version: AGENT_WIRE_VERSION,
                    required_features: vec!["unknown".into()],
                    optional_features: Vec::new(),
                },
                request: AgentWireRequest::Negotiate,
            },
        )
        .unwrap_err();
        assert_eq!(unsupported.code, "agent.wire.unsupported_feature");

        let invalid_limit = dispatch_agent_request(
            &mut service,
            envelope(
                2,
                AgentWireRequest::SubscribeSessionEvents {
                    session_id: "session".into(),
                    after_sequence: 0,
                    limit: MAX_EVENT_PAGE_SIZE + 1,
                },
            ),
        )
        .unwrap_err();
        assert_eq!(invalid_limit.code, "agent.wire.event_limit");

        let mut sequence = AgentEventSequence::default();
        let event = |value| AgentEventEnvelope {
            session_id: "session".into(),
            sequence: value,
            meta: Default::default(),
            event: AgentEvent::Cancelled {
                turn_id: "turn".into(),
            },
        };
        sequence.observe(&event(1)).unwrap();
        assert_eq!(
            sequence.observe(&event(1)).unwrap_err().code,
            "agent.event.duplicate_or_out_of_order"
        );
        assert_eq!(
            sequence.observe(&event(3)).unwrap_err().code,
            "agent.event.gap"
        );
    }
}
