use serde_json::Value;

use mutsuki_agent_protocol::{AgentMessage, AgentRole};

#[derive(Clone, Debug)]
pub struct MessageBuilder {
    message: AgentMessage,
}

impl MessageBuilder {
    pub fn new(role: AgentRole, content: impl Into<String>) -> Self {
        Self {
            message: AgentMessage {
                role,
                content: content.into(),
                name: None,
                metadata: None,
            },
        }
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.message.name = Some(name.into());
        self
    }

    pub fn metadata(mut self, metadata: Value) -> Self {
        self.message.metadata = Some(metadata);
        self
    }

    pub fn build(self) -> AgentMessage {
        self.message
    }
}
