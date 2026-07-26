use serde::{Deserialize, Serialize};

use crate::{RuntimeError, Task, TaskHandle};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuntimeDomainId(String);

impl RuntimeDomainId {
    pub fn new(value: impl Into<String>) -> Result<Self, RuntimeError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(RuntimeError::new(
                crate::ERR_RUNTIME_HOST_FAILED,
                "runtime.domain",
                "runtime.domain_id.empty",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RuntimeDomainId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainTaskHandle {
    pub domain_id: RuntimeDomainId,
    pub task: TaskHandle,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrossDomainTaskRequest {
    pub request_id: String,
    pub source_domain: RuntimeDomainId,
    pub target_domain: RuntimeDomainId,
    pub task: Task,
    pub timeout_ms: u64,
    pub idempotency_key: String,
    pub max_attempts: u32,
}

impl CrossDomainTaskRequest {
    pub fn validate(&self) -> Result<(), RuntimeError> {
        if self.request_id.trim().is_empty()
            || self.idempotency_key.trim().is_empty()
            || self.timeout_ms == 0
            || self.max_attempts == 0
        {
            return Err(RuntimeError::new(
                crate::ERR_RUNTIME_HOST_FAILED,
                "runtime.domain",
                format!("cross_domain.request.{}", self.request_id),
            ));
        }
        Ok(())
    }
}
