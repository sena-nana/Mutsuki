//! Agent loop policy owned by the Runtime domain.

use mutsuki_agent_contracts::{AgentError, AgentResult, AgentRunRequest};

#[derive(Clone, Default)]
pub struct AgentLoop {
    default_model: Option<String>,
}

impl AgentLoop {
    pub fn with_default_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = Some(model.into());
        self
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
