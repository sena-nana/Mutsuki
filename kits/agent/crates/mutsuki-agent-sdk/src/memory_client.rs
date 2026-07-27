use mutsuki_agent_protocol::{
    AgentMemoryActivateRequest, AgentMemoryQueryRequest, AgentMemoryWriteRequest,
};
use mutsuki_runtime_sdk::{AsyncRunnerContext, CallFuture};

use crate::{AgentMemoryActivateProtocol, AgentMemoryQueryProtocol, AgentMemoryWriteProtocol};

#[derive(Clone)]
pub struct MemoryClient {
    ctx: AsyncRunnerContext,
}

impl MemoryClient {
    pub fn new(ctx: AsyncRunnerContext) -> Self {
        Self { ctx }
    }

    pub fn query(&self, request: AgentMemoryQueryRequest) -> CallFuture {
        self.ctx.call::<AgentMemoryQueryProtocol>(request)
    }

    pub fn write(&self, request: AgentMemoryWriteRequest) -> CallFuture {
        self.ctx.call::<AgentMemoryWriteProtocol>(request)
    }

    pub fn activate(&self, request: AgentMemoryActivateRequest) -> CallFuture {
        self.ctx.call::<AgentMemoryActivateProtocol>(request)
    }
}
