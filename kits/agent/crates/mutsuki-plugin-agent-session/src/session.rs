use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mutsuki_agent_protocol::{
    AgentError, AgentResult, AgentSession, AgentSessionAppendRequest, AgentSessionCreateRequest,
    AgentSessionGetRequest, AgentSessionSnapshotRequest,
};
use mutsuki_agent_sdk::{session_cell_ref, session_resource_ref};

use crate::PLUGIN_ID;

#[derive(Clone, Default)]
pub struct SessionStore {
    inner: Arc<SessionStoreInner>,
}

#[derive(Default)]
struct SessionStoreInner {
    next_id: AtomicU64,
    sessions: Mutex<BTreeMap<String, AgentSession>>,
}

impl SessionStore {
    pub fn create(&self, request: AgentSessionCreateRequest) -> AgentResult<AgentSession> {
        if request.profile_id.trim().is_empty() {
            return Err(AgentError::invalid_input("profile_id is required"));
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let session_id = format!("agent-session-{id}");
        let mut session = AgentSession::new(
            session_id.clone(),
            request.profile_id,
            session_resource_ref(PLUGIN_ID, &session_id),
            session_cell_ref(PLUGIN_ID, &session_id),
        );
        session.title = request.title;
        self.inner
            .sessions
            .lock()
            .expect("session store mutex poisoned")
            .insert(session_id, session.clone());
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
        let mut sessions = self
            .inner
            .sessions
            .lock()
            .expect("session store mutex poisoned");
        let session = sessions.get_mut(&request.session_id).ok_or_else(|| {
            AgentError::not_found(format!("session `{}` not found", request.session_id))
        })?;
        session.messages.extend(request.messages);
        session.turn_count += 1;
        Ok(session.clone())
    }

    pub fn snapshot(&self, request: AgentSessionSnapshotRequest) -> AgentResult<AgentSession> {
        self.get(AgentSessionGetRequest {
            session_id: request.session_id,
        })
    }
}
