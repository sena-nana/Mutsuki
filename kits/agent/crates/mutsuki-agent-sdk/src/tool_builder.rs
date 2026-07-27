use serde_json::Value;

use mutsuki_agent_protocol::{AgentToolDescriptor, ToolSideEffect};
use mutsuki_runtime_sdk::SdkProtocol;

#[derive(Clone, Debug)]
pub struct ToolBuilder {
    descriptor: AgentToolDescriptor,
}

impl ToolBuilder {
    pub fn new(
        name: impl Into<String>,
        target_protocol_id: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            descriptor: AgentToolDescriptor::new(name, target_protocol_id, description),
        }
    }

    pub fn for_protocol<P>(name: impl Into<String>, description: impl Into<String>) -> Self
    where
        P: SdkProtocol,
    {
        Self::new(name, P::PROTOCOL_ID, description)
    }

    pub fn target_protocol<P>(mut self) -> Self
    where
        P: SdkProtocol,
    {
        self.descriptor.target_protocol_id = P::PROTOCOL_ID.into();
        self
    }

    pub fn input_schema(mut self, schema: Value) -> Self {
        self.descriptor.input_schema = schema;
        self
    }

    pub fn output_schema(mut self, schema: Value) -> Self {
        self.descriptor.output_schema = schema;
        self
    }

    pub fn side_effect(mut self, side_effect: ToolSideEffect) -> Self {
        self.descriptor.side_effect = side_effect;
        self
    }

    pub fn requires_approval(mut self, requires_approval: bool) -> Self {
        self.descriptor.requires_approval = requires_approval;
        self
    }

    pub fn permission(mut self, permission: impl Into<String>) -> Self {
        self.descriptor.permissions.push(permission.into());
        self
    }

    pub fn build(self) -> AgentToolDescriptor {
        self.descriptor
    }
}
