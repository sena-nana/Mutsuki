use std::sync::{Arc, Mutex};

use mutsuki_agent_protocol::{
    AgentContext, AgentContextBuildRequest, AgentError, AgentMemoryRecord, AgentResult,
    AgentToolDescriptor,
};

#[derive(Clone, Default)]
pub struct ContextBuilder {
    tools: Arc<Mutex<Vec<AgentToolDescriptor>>>,
    memories: Arc<Mutex<Vec<AgentMemoryRecord>>>,
    system_prompt: Arc<Mutex<Option<String>>>,
}

impl ContextBuilder {
    pub fn set_tools(&self, tools: Vec<AgentToolDescriptor>) {
        *self.tools.lock().expect("context builder mutex poisoned") = tools;
    }

    pub fn set_memories(&self, memories: Vec<AgentMemoryRecord>) {
        *self
            .memories
            .lock()
            .expect("context builder mutex poisoned") = memories;
    }

    pub fn set_system_prompt(&self, prompt: impl Into<String>) {
        *self
            .system_prompt
            .lock()
            .expect("context builder mutex poisoned") = Some(prompt.into());
    }

    pub fn build(&self, request: AgentContextBuildRequest) -> AgentResult<AgentContext> {
        if request.profile_id.trim().is_empty() {
            return Err(AgentError::invalid_input("profile_id is required"));
        }
        let tools = self
            .tools
            .lock()
            .expect("context builder mutex poisoned")
            .clone();
        let memories = self
            .memories
            .lock()
            .expect("context builder mutex poisoned")
            .clone();
        let rendered_prompt = self
            .system_prompt
            .lock()
            .expect("context builder mutex poisoned")
            .clone();
        Ok(AgentContext {
            profile_id: request.profile_id,
            messages: request.messages,
            tools,
            memories,
            rendered_prompt,
        })
    }
}
