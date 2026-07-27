use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mutsuki_agent_protocol::{
    AgentError, AgentResult, AgentToolDescriptor, AgentToolListRequest, AgentToolListResult,
};

#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: Arc<Mutex<BTreeMap<String, AgentToolDescriptor>>>,
}

impl ToolRegistry {
    pub fn register(&self, descriptor: AgentToolDescriptor) -> AgentResult<()> {
        if descriptor.name.trim().is_empty() {
            return Err(AgentError::invalid_input("tool name is required"));
        }
        if descriptor.target_protocol_id.trim().is_empty() {
            return Err(AgentError::invalid_input("target_protocol_id is required"));
        }
        self.tools
            .lock()
            .expect("tool registry mutex poisoned")
            .insert(descriptor.name.clone(), descriptor);
        Ok(())
    }

    pub fn list(&self, _request: AgentToolListRequest) -> AgentToolListResult {
        AgentToolListResult {
            tools: self
                .tools
                .lock()
                .expect("tool registry mutex poisoned")
                .values()
                .cloned()
                .collect(),
        }
    }

    pub fn get(&self, name: &str) -> AgentResult<AgentToolDescriptor> {
        self.tools
            .lock()
            .expect("tool registry mutex poisoned")
            .get(name)
            .cloned()
            .ok_or_else(|| AgentError::not_found(format!("tool `{name}` not registered")))
    }
}
