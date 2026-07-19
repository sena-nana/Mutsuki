use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ScalarValue;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeError {
    pub code: String,
    pub source: String,
    pub route: String,
    pub lost_capability: Option<String>,
    pub recovery: Option<String>,
    pub cause: Option<Box<RuntimeError>>,
    pub evidence: BTreeMap<String, ScalarValue>,
}

impl RuntimeError {
    pub fn new(
        code: impl Into<String>,
        source: impl Into<String>,
        route: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            source: source.into(),
            route: route.into(),
            lost_capability: None,
            recovery: None,
            cause: None,
            evidence: BTreeMap::new(),
        }
    }
}

pub const ERR_PLUGIN_DISABLED: &str = "plugin.disabled";
pub const ERR_PLUGIN_NOT_FOUND: &str = "plugin.not_found";
pub const ERR_RUNTIME_HOST_FAILED: &str = "runtime.host_failed";
pub const ERR_RUNTIME_HOST_GENERATION_MISMATCH: &str = "runtime.host_generation_mismatch";
pub const ERR_RUNTIME_NOT_ACCEPTING: &str = "runtime.not_accepting";
pub const ERR_RUNTIME_ABORTED: &str = "runtime.aborted";
pub const ERR_PORTABLE_SCHEMA_UNSUPPORTED: &str = "portable.schema_unsupported";
pub const ERR_CHECKPOINT_INCOMPATIBLE: &str = "checkpoint.incompatible";
pub const ERR_EXECUTION_NO_VARIANT: &str = "execution.no_variant";
pub const ERR_CAPABILITY_EXHAUSTED: &str = "capability.exhausted";
pub const ERR_TASK_NOT_FOUND: &str = "task.not_found";
pub const ERR_TASK_DUPLICATE: &str = "task.duplicate";
pub const ERR_TASK_CLAIM_CONFLICT: &str = "task.claim_conflict";
pub const ERR_TASK_EXPIRED: &str = "task.expired";
pub const ERR_TASK_DEAD_LETTER: &str = "task.dead_letter";
pub const ERR_TASK_UNSUPPORTED: &str = "task.unsupported";
pub const ERR_RUNNER_NOT_FOUND: &str = "runner.not_found";
pub const ERR_RUNNER_PURITY_VIOLATION: &str = "runner.purity_violation";
pub const ERR_RUNNER_AWAITABLE_UNSUPPORTED: &str = "runner.awaitable_unsupported";
pub const ERR_REGISTRY_FROZEN: &str = "registry.frozen";
pub const ERR_REGISTRY_UNAUTHORIZED: &str = "registry.unauthorized";
pub const ERR_REGISTRY_GENERATION_MISMATCH: &str = "registry.generation_mismatch";
pub const ERR_STATE_CONFLICT: &str = "state.conflict";
pub const ERR_RESOURCE_NOT_FOUND: &str = "resource.not_found";
pub const ERR_RESOURCE_UNSUPPORTED: &str = "resource.unsupported";
pub const ERR_RESOURCE_LEASE_EXPIRED: &str = "resource.lease_expired";
pub const ERR_RESOURCE_GENERATION_MISMATCH: &str = "resource.generation_mismatch";
pub const ERR_RELOAD_BLOCKED: &str = "plugin.reload_blocked";
