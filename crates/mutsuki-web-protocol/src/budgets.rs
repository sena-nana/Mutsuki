//! Explicit resource budgets. Missing limits must fail structured, never silently grow.

use serde::{Deserialize, Serialize};

/// Default budgets for a local development / single-user host.
pub const DEFAULT_BUDGETS: ResourceBudgets = ResourceBudgets {
    max_connections: 64,
    max_sessions: 64,
    max_subscriptions_per_session: 64,
    max_rpc_inflight_per_session: 16,
    max_payload_bytes: 256 * 1024,
    max_ws_queue_depth: 128,
    max_event_queue_depth: 256,
    max_static_cache_bytes: 64 * 1024 * 1024,
    max_extensions: 128,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceBudgets {
    pub max_connections: usize,
    pub max_sessions: usize,
    pub max_subscriptions_per_session: usize,
    pub max_rpc_inflight_per_session: usize,
    pub max_payload_bytes: usize,
    pub max_ws_queue_depth: usize,
    pub max_event_queue_depth: usize,
    pub max_static_cache_bytes: usize,
    pub max_extensions: usize,
}

impl Default for ResourceBudgets {
    fn default() -> Self {
        DEFAULT_BUDGETS
    }
}
