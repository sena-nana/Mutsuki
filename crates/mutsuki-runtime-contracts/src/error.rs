use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ScalarValue;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeError {
    pub code: String,
    pub source: String,
    pub route: String,
    #[serde(default)]
    pub lost_capability: Option<String>,
    #[serde(default)]
    pub recovery: Option<String>,
    #[serde(default)]
    pub cause: Option<Box<RuntimeError>>,
    #[serde(default)]
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

pub const ERR_AGENT_NOT_FOUND: &str = "agent.not_found";
pub const ERR_OPERATION_NOT_FOUND: &str = "operation.not_found";
pub const ERR_RUNTIME_BACKEND_FAILED: &str = "runtime.backend_failed";
pub const ERR_RUNTIME_BACKEND_GENERATION_MISMATCH: &str = "runtime.backend_generation_mismatch";
pub const ERR_SCOPE_NO_MATCH: &str = "scope.no_match";
pub const ERR_SOURCE_UNREGISTERED: &str = "source.unregistered";
