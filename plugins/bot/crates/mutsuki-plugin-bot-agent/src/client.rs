#[allow(clippy::wildcard_imports)]
use super::*;

pub trait AgentBridgeClient: Send {
    /// Loads one Agent session snapshot.
    ///
    /// # Errors
    ///
    /// Returns the Agent wire error reported by the session owner.
    fn get_session(&mut self, session_id: &str) -> Result<AgentSession, AgentWireError>;
    /// Creates one Agent session.
    ///
    /// # Errors
    ///
    /// Returns the Agent wire error reported by the session owner.
    fn start_session(
        &mut self,
        request: AgentSessionCreateRequest,
    ) -> Result<AgentSession, AgentWireError>;
    /// Submits an idempotent Agent turn against the expected session version.
    ///
    /// # Errors
    ///
    /// Returns a typed wire error for stale versions, invalid input, or execution failure.
    fn submit_turn(
        &mut self,
        session_id: &str,
        expected_version: SessionVersion,
        turn_id: &str,
        messages: Vec<AgentMessage>,
        idempotency_key: &str,
    ) -> Result<SessionVersion, AgentWireError>;
    /// Cancels an Agent turn with optimistic session versioning.
    ///
    /// # Errors
    ///
    /// Returns the typed Agent wire failure.
    fn cancel_turn(
        &mut self,
        session_id: &str,
        turn_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError>;
    /// Forks an Agent session into an owner-provided target identity.
    ///
    /// # Errors
    ///
    /// Returns the typed Agent wire failure.
    fn fork_session(
        &mut self,
        source_session_id: &str,
        target_session_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError>;
    /// Reads committed Agent events after the supplied sequence.
    ///
    /// # Errors
    ///
    /// Returns the typed Agent wire failure.
    fn events(
        &mut self,
        session_id: &str,
        after_sequence: u64,
    ) -> Result<AgentEventPage, AgentWireError>;
}

impl<B: AgentClientBackend + Send> AgentBridgeClient for AgentClient<B> {
    fn get_session(&mut self, session_id: &str) -> Result<AgentSession, AgentWireError> {
        self.get_session(session_id)
    }

    fn start_session(
        &mut self,
        request: AgentSessionCreateRequest,
    ) -> Result<AgentSession, AgentWireError> {
        self.start_session(request)
    }

    fn submit_turn(
        &mut self,
        session_id: &str,
        expected_version: SessionVersion,
        turn_id: &str,
        messages: Vec<AgentMessage>,
        idempotency_key: &str,
    ) -> Result<SessionVersion, AgentWireError> {
        self.submit_turn(
            session_id,
            expected_version,
            turn_id,
            messages,
            idempotency_key,
        )
    }

    fn cancel_turn(
        &mut self,
        session_id: &str,
        turn_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError> {
        self.cancel_turn(session_id, turn_id, expected_version)
    }

    fn fork_session(
        &mut self,
        source_session_id: &str,
        target_session_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError> {
        let session = self.get_session(source_session_id)?;
        self.fork_session(
            source_session_id,
            target_session_id,
            SessionSnapshotRef {
                session_id: source_session_id.into(),
                version: expected_version,
                snapshot: session.resource,
                base: None,
                deltas: Vec::new(),
            },
        )
    }

    fn events(
        &mut self,
        session_id: &str,
        after_sequence: u64,
    ) -> Result<AgentEventPage, AgentWireError> {
        self.resume_session_events(session_id, after_sequence)
    }
}
