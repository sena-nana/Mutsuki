use mutsuki_agent_contracts::{
    AgentMemoryActivateRequest, AgentMemoryDeleteRequest, AgentMemoryDisableRequest,
    AgentMemoryOpportunityRequest, AgentMemoryQueryRequest, AgentMemoryReviseRequest,
    AgentMemoryWriteRequest, MemoryAdoptionFeedback,
};
use mutsuki_runtime_sdk::{AsyncRunnerContext, CallFuture};

use crate::{
    AgentMemoryActivateProtocol, AgentMemoryDeleteProtocol, AgentMemoryDisableProtocol,
    AgentMemoryFeedbackProtocol, AgentMemoryOpportunityProtocol, AgentMemoryQueryProtocol,
    AgentMemoryReviseProtocol, AgentMemoryWriteProtocol,
};

#[derive(Clone)]
pub struct MemoryClient {
    ctx: AsyncRunnerContext,
}

impl MemoryClient {
    pub fn new(ctx: AsyncRunnerContext) -> Self {
        Self { ctx }
    }

    pub fn query(&self, request: AgentMemoryQueryRequest) -> CallFuture<AgentMemoryQueryProtocol> {
        self.ctx.call::<AgentMemoryQueryProtocol>(request)
    }

    pub fn write(&self, request: AgentMemoryWriteRequest) -> CallFuture<AgentMemoryWriteProtocol> {
        self.ctx.call::<AgentMemoryWriteProtocol>(request)
    }

    pub fn activate(
        &self,
        request: AgentMemoryActivateRequest,
    ) -> CallFuture<AgentMemoryActivateProtocol> {
        self.ctx.call::<AgentMemoryActivateProtocol>(request)
    }

    pub fn disable(
        &self,
        request: AgentMemoryDisableRequest,
    ) -> CallFuture<AgentMemoryDisableProtocol> {
        self.ctx.call::<AgentMemoryDisableProtocol>(request)
    }

    pub fn delete(
        &self,
        request: AgentMemoryDeleteRequest,
    ) -> CallFuture<AgentMemoryDeleteProtocol> {
        self.ctx.call::<AgentMemoryDeleteProtocol>(request)
    }

    pub fn revise(
        &self,
        request: AgentMemoryReviseRequest,
    ) -> CallFuture<AgentMemoryReviseProtocol> {
        self.ctx.call::<AgentMemoryReviseProtocol>(request)
    }

    pub fn opportunity(
        &self,
        request: AgentMemoryOpportunityRequest,
    ) -> CallFuture<AgentMemoryOpportunityProtocol> {
        self.ctx.call::<AgentMemoryOpportunityProtocol>(request)
    }

    pub fn feedback(
        &self,
        request: MemoryAdoptionFeedback,
    ) -> CallFuture<AgentMemoryFeedbackProtocol> {
        self.ctx.call::<AgentMemoryFeedbackProtocol>(request)
    }
}
