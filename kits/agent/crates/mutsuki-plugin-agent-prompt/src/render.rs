use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mutsuki_agent_protocol::{
    AgentError, AgentPromptGetRequest, AgentPromptRenderRequest, AgentPromptRenderResult,
    AgentPromptTemplate, AgentResult,
};
use serde_json::Value;

#[derive(Clone, Default)]
pub struct PromptRegistry {
    templates: Arc<Mutex<BTreeMap<String, AgentPromptTemplate>>>,
}

impl PromptRegistry {
    pub fn register(&self, template: AgentPromptTemplate) -> AgentResult<()> {
        if template.template_id.trim().is_empty() {
            return Err(AgentError::invalid_input("template_id is required"));
        }
        self.templates
            .lock()
            .expect("prompt registry mutex poisoned")
            .insert(template.template_id.clone(), template);
        Ok(())
    }

    pub fn get(&self, request: AgentPromptGetRequest) -> AgentResult<AgentPromptTemplate> {
        self.templates
            .lock()
            .expect("prompt registry mutex poisoned")
            .get(&request.template_id)
            .cloned()
            .ok_or_else(|| {
                AgentError::not_found(format!("prompt `{}` not found", request.template_id))
            })
    }

    pub fn render(
        &self,
        request: AgentPromptRenderRequest,
    ) -> AgentResult<AgentPromptRenderResult> {
        let template = self.get(AgentPromptGetRequest {
            template_id: request.template_id.clone(),
        })?;
        let mut text = template.body;
        for variable in template.variables {
            let marker = format!("{{{{{variable}}}}}");
            let value = request
                .variables
                .get(&variable)
                .map(render_value)
                .unwrap_or_default();
            text = text.replace(&marker, &value);
        }
        Ok(AgentPromptRenderResult {
            template_id: request.template_id,
            text,
        })
    }
}

fn render_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
