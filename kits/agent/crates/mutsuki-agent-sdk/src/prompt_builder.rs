use std::collections::BTreeMap;

use serde_json::Value;

use mutsuki_agent_protocol::AgentPromptRenderRequest;

#[derive(Clone, Debug)]
pub struct PromptBuilder {
    template_id: String,
    variables: BTreeMap<String, Value>,
}

impl PromptBuilder {
    pub fn new(template_id: impl Into<String>) -> Self {
        Self {
            template_id: template_id.into(),
            variables: BTreeMap::new(),
        }
    }

    pub fn variable(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.variables.insert(name.into(), value.into());
        self
    }

    pub fn build(self) -> AgentPromptRenderRequest {
        AgentPromptRenderRequest {
            template_id: self.template_id,
            variables: self.variables,
        }
    }
}
