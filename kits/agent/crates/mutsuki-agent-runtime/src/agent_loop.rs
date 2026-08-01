//! Agent loop policy owned by the Runtime domain.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mutsuki_agent_contracts::{AgentError, AgentResult, AgentRunRequest};

#[derive(Clone, Default)]
pub struct AgentLoop {
    default_model: Option<String>,
    active_turns: Arc<Mutex<BTreeMap<String, String>>>,
}

/// Runtime-owned fence for the single active turn allowed by a session.
#[derive(Debug)]
pub struct AgentTurnLease {
    session_id: String,
    turn_id: String,
    active_turns: Arc<Mutex<BTreeMap<String, String>>>,
}

impl Drop for AgentTurnLease {
    fn drop(&mut self) {
        let Ok(mut active_turns) = self.active_turns.lock() else {
            return;
        };
        if active_turns.get(&self.session_id) == Some(&self.turn_id) {
            active_turns.remove(&self.session_id);
        }
    }
}

impl AgentLoop {
    pub fn with_default_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = Some(model.into());
        self
    }

    #[doc(hidden)]
    pub fn acquire_turn(
        &self,
        session_id: impl Into<String>,
        turn_id: impl Into<String>,
    ) -> AgentResult<AgentTurnLease> {
        let session_id = session_id.into();
        let turn_id = turn_id.into();
        if session_id.trim().is_empty() || turn_id.trim().is_empty() {
            return Err(AgentError::invalid_input(
                "session_id and turn_id are required for a session turn",
            ));
        }
        let mut active_turns = self.active_turns.lock().map_err(|_| {
            AgentError::new(
                "agent.runtime.state_poisoned",
                "active turn registry lock poisoned",
            )
        })?;
        if let Some(active_turn_id) = active_turns.get(&session_id) {
            return Err(AgentError::new(
                "agent.turn.already_active",
                format!("session `{session_id}` is already running turn `{active_turn_id}`"),
            ));
        }
        active_turns.insert(session_id.clone(), turn_id.clone());
        drop(active_turns);
        Ok(AgentTurnLease {
            session_id,
            turn_id,
            active_turns: Arc::clone(&self.active_turns),
        })
    }

    #[doc(hidden)]
    pub fn validate(&self, request: &AgentRunRequest) -> AgentResult<String> {
        if request.profile_id.trim().is_empty() {
            return Err(AgentError::invalid_input("profile_id is required"));
        }
        let model = request
            .model
            .as_ref()
            .or(self.default_model.as_ref())
            .filter(|model| !model.trim().is_empty())
            .cloned()
            .ok_or_else(|| {
                AgentError::provider_unavailable(
                    "AgentLoop requires an explicitly configured model",
                )
            })?;
        Ok(model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_turn_lease_fences_concurrent_turns_and_releases_on_drop() {
        let agent_loop = AgentLoop::default();
        let first = agent_loop.acquire_turn("session-1", "turn-1").unwrap();
        let conflict = agent_loop.acquire_turn("session-1", "turn-2").unwrap_err();
        assert_eq!(conflict.code, "agent.turn.already_active");

        let other_session = agent_loop.acquire_turn("session-2", "turn-1").unwrap();
        drop(other_session);
        drop(first);

        agent_loop.acquire_turn("session-1", "turn-2").unwrap();
    }
}
