use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mutsuki_web_protocol::{
    EventEnvelope, EventSubscription, JsonValue, ProtocolError, ProtocolResult, ResourceBudgets,
};
use std::sync::Mutex;
use tokio::sync::Notify;
use uuid::Uuid;

use crate::BridgeMetrics;

#[derive(Debug, Clone)]
pub struct BridgeSession {
    pub session_id: Uuid,
    pub principal_id: String,
    pub capabilities: Vec<String>,
    pub safe_mode: bool,
}

#[derive(Debug, Clone)]
pub struct AuthGrant {
    pub principal_id: String,
    pub capabilities: Vec<String>,
}

impl BridgeSession {
    pub fn require_capability(&self, capability: &str) -> ProtocolResult<()> {
        if self
            .capabilities
            .iter()
            .any(|cap| cap == "*" || cap == capability)
        {
            Ok(())
        } else {
            Err(ProtocolError::CapabilityDenied(capability.to_string()))
        }
    }
}

/// Authentication policy. Tokens are never returned to clients or logs.
#[derive(Debug, Clone)]
pub enum AuthPolicy {
    /// Local loopback deployments may use a shared static token list.
    Local {
        accepted_tokens: Vec<String>,
        default_capabilities: Vec<String>,
        allow_unauthenticated: bool,
    },
    /// Remote deployments require a non-empty token and optional TLS marker.
    Remote {
        accepted_tokens: Vec<String>,
        default_capabilities: Vec<String>,
        require_tls: bool,
        tls_enabled: bool,
    },
}

impl AuthPolicy {
    /// Unauthenticated loopback sessions get a minimal read-only set.
    ///
    /// Does **not** grant `*`, recovery writes, runtime writes, or config writes.
    /// Token-authenticated local consoles use explicit capability lists instead.
    pub fn open_local() -> Self {
        Self::Local {
            accepted_tokens: vec![],
            default_capabilities: vec!["host.read".into(), "recovery.read".into()],
            allow_unauthenticated: true,
        }
    }

    pub fn allow_local(default_capabilities: Vec<String>) -> Self {
        Self::Local {
            accepted_tokens: vec!["local-dev".into()],
            default_capabilities,
            allow_unauthenticated: false,
        }
    }

    pub fn remote(accepted_tokens: Vec<String>, tls_enabled: bool) -> Self {
        Self::Remote {
            accepted_tokens,
            default_capabilities: vec!["host.read".into(), "recovery.read".into()],
            require_tls: true,
            tls_enabled,
        }
    }

    pub fn authenticate(&self, token: Option<&str>) -> ProtocolResult<AuthGrant> {
        match self {
            Self::Local {
                accepted_tokens,
                default_capabilities,
                allow_unauthenticated,
            } => {
                if *allow_unauthenticated && token.is_none() {
                    return Ok(AuthGrant {
                        principal_id: "local-readonly".into(),
                        capabilities: default_capabilities.clone(),
                    });
                }
                let token = token.ok_or(ProtocolError::Unauthenticated)?;
                if token_accepted(accepted_tokens, token) {
                    Ok(AuthGrant {
                        principal_id: "local-web-console".into(),
                        capabilities: default_capabilities.clone(),
                    })
                } else {
                    Err(ProtocolError::Unauthenticated)
                }
            }
            Self::Remote {
                accepted_tokens,
                default_capabilities,
                require_tls,
                tls_enabled,
            } => {
                if *require_tls && !*tls_enabled {
                    return Err(ProtocolError::Internal("remote auth requires TLS".into()));
                }
                let token = token.ok_or(ProtocolError::Unauthenticated)?;
                if token_accepted(accepted_tokens, token) {
                    Ok(AuthGrant {
                        principal_id: "remote-web-console".into(),
                        capabilities: default_capabilities.clone(),
                    })
                } else {
                    Err(ProtocolError::Unauthenticated)
                }
            }
        }
    }
}

/// Decides whether a presented token is on the accepted list.
///
/// An empty accepted list means "no token can authenticate", never "every token authenticates".
/// The capability set attached to a policy is chosen independently of the token list, so a
/// fail-open list would hand whatever capabilities that policy carries to any caller that guesses
/// a token exists at all.
///
/// The comparison is constant-time: an unauthenticated caller may retry indefinitely, and a
/// byte-by-byte early exit turns each retry into a probe for the next byte of the token.
fn token_accepted(accepted_tokens: &[String], presented: &str) -> bool {
    let mut accepted = false;
    for candidate in accepted_tokens {
        accepted |= constant_time_eq(candidate.as_bytes(), presented.as_bytes());
    }
    accepted
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut diff = u8::from(left.len() != right.len());
    for index in 0..left.len().max(right.len()) {
        let lhs = left.get(index).copied().unwrap_or(0);
        let rhs = right.get(index).copied().unwrap_or(0);
        diff |= lhs ^ rhs;
    }
    diff == 0
}

struct SessionState {
    session: BridgeSession,
    subscriptions: HashMap<Uuid, EventSubscription>,
    outbound: std::collections::VecDeque<EventEnvelope>,
    event_ready: Arc<Notify>,
    sequence: u64,
}

pub struct SessionManager {
    budgets: ResourceBudgets,
    sessions: Mutex<HashMap<Uuid, SessionState>>,
    active: AtomicU64,
}

impl SessionManager {
    pub fn new(budgets: ResourceBudgets) -> Self {
        Self {
            budgets,
            sessions: Mutex::new(HashMap::new()),
            active: AtomicU64::new(0),
        }
    }

    pub fn active_count(&self) -> usize {
        self.active.load(Ordering::Relaxed) as usize
    }

    pub fn create(
        &self,
        capabilities: Vec<String>,
        safe_mode: bool,
    ) -> ProtocolResult<BridgeSession> {
        self.create_authenticated("internal", capabilities, safe_mode)
    }

    /// # Panics
    ///
    /// Panics if the session registry lock is poisoned.
    pub fn create_authenticated(
        &self,
        principal_id: impl Into<String>,
        capabilities: Vec<String>,
        safe_mode: bool,
    ) -> ProtocolResult<BridgeSession> {
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.len() >= self.budgets.max_sessions {
            return Err(ProtocolError::BudgetExceeded(format!(
                "max_sessions={}",
                self.budgets.max_sessions
            )));
        }
        let session = BridgeSession {
            session_id: Uuid::new_v4(),
            principal_id: principal_id.into(),
            capabilities,
            safe_mode,
        };
        sessions.insert(
            session.session_id,
            SessionState {
                session: session.clone(),
                subscriptions: HashMap::new(),
                outbound: std::collections::VecDeque::new(),
                event_ready: Arc::new(Notify::new()),
                sequence: 0,
            },
        );
        self.active.fetch_add(1, Ordering::Relaxed);
        Ok(session)
    }

    /// # Panics
    ///
    /// Panics if the session registry lock is poisoned.
    pub fn get(&self, session_id: Uuid) -> Option<BridgeSession> {
        self.sessions
            .lock()
            .unwrap()
            .get(&session_id)
            .map(|state| state.session.clone())
    }

    /// # Panics
    ///
    /// Panics if the session registry lock is poisoned.
    pub fn subscribe(
        &self,
        session_id: Uuid,
        subscription: EventSubscription,
    ) -> ProtocolResult<()> {
        let mut sessions = self.sessions.lock().unwrap();
        let state = sessions
            .get_mut(&session_id)
            .ok_or(ProtocolError::Unauthenticated)?;
        if state.subscriptions.len() >= self.budgets.max_subscriptions_per_session {
            return Err(ProtocolError::BudgetExceeded(format!(
                "max_subscriptions_per_session={}",
                self.budgets.max_subscriptions_per_session
            )));
        }
        state
            .subscriptions
            .insert(subscription.subscription_id, subscription);
        Ok(())
    }

    /// # Panics
    ///
    /// Panics if the session registry lock is poisoned.
    pub fn unsubscribe(&self, session_id: Uuid, subscription_id: Uuid) {
        if let Some(state) = self.sessions.lock().unwrap().get_mut(&session_id) {
            state.subscriptions.remove(&subscription_id);
        }
    }

    /// # Panics
    ///
    /// Panics if the session registry lock is poisoned.
    pub fn fanout(
        &self,
        topic: &str,
        payload: JsonValue,
        metrics: &BridgeMetrics,
    ) -> ProtocolResult<u64> {
        let mut sessions = self.sessions.lock().unwrap();
        let mut delivered = 0u64;
        let mut ready = Vec::new();
        for state in sessions.values_mut() {
            let matches: Vec<Uuid> = state
                .subscriptions
                .values()
                .filter(|sub| sub.topic == topic)
                .map(|sub| sub.subscription_id)
                .collect();
            let mut session_delivered = false;
            for subscription_id in matches {
                if state.outbound.len() >= self.budgets.max_event_queue_depth {
                    metrics.inc_events_dropped();
                    continue;
                }
                state.sequence += 1;
                state.outbound.push_back(EventEnvelope {
                    subscription_id,
                    topic: topic.to_string(),
                    sequence: state.sequence,
                    payload: payload.clone(),
                });
                delivered += 1;
                session_delivered = true;
            }
            if session_delivered {
                ready.push(state.event_ready.clone());
            }
        }
        drop(sessions);
        for notifier in ready {
            notifier.notify_one();
        }
        Ok(delivered)
    }

    /// # Panics
    ///
    /// Panics if the session registry lock is poisoned.
    pub fn event_notifier(&self, session_id: Uuid) -> Option<Arc<Notify>> {
        self.sessions
            .lock()
            .unwrap()
            .get(&session_id)
            .map(|state| state.event_ready.clone())
    }

    /// # Panics
    ///
    /// Panics if the session registry lock is poisoned.
    pub fn drain_events(&self, session_id: Uuid) -> Vec<EventEnvelope> {
        let mut sessions = self.sessions.lock().unwrap();
        let Some(state) = sessions.get_mut(&session_id) else {
            return Vec::new();
        };
        state.outbound.drain(..).collect()
    }

    /// # Panics
    ///
    /// Panics if the session registry lock is poisoned.
    pub fn close(&self, session_id: Uuid) {
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.remove(&session_id).is_some() {
            self.active.fetch_sub(1, Ordering::Relaxed);
        }
    }
}
