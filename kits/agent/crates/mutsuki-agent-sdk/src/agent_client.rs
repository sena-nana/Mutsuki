use mutsuki_agent_protocol::{AgentMessage, AgentRunRequest};
use mutsuki_runtime_sdk::{AsyncRunnerContext, CallFuture};

use crate::AgentRunProtocol;

#[derive(Clone)]
pub struct AgentClient {
    ctx: AsyncRunnerContext,
}

impl AgentClient {
    pub fn new(ctx: AsyncRunnerContext) -> Self {
        Self { ctx }
    }

    pub fn run(&self, request: AgentRunRequest) -> CallFuture {
        self.ctx.call::<AgentRunProtocol>(request)
    }

    pub fn run_text(&self, profile_id: impl Into<String>, text: impl Into<String>) -> AgentRunCall {
        AgentRunCall {
            ctx: self.ctx.clone(),
            request: AgentRunRequest::new(profile_id, vec![AgentMessage::user(text)]),
        }
    }
}

pub struct AgentRunCall {
    ctx: AsyncRunnerContext,
    request: AgentRunRequest,
}

impl AgentRunCall {
    pub fn profile(mut self, profile_id: impl Into<String>) -> Self {
        self.request.profile_id = profile_id.into();
        self
    }

    pub fn session(mut self, session_id: impl Into<String>) -> Self {
        self.request.session_id = Some(session_id.into());
        self
    }

    pub fn max_steps(mut self, max_steps: u32) -> Self {
        self.request.max_steps = max_steps;
        self
    }

    pub fn stream(mut self, stream: bool) -> Self {
        self.request.stream = stream;
        self
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.request.model = Some(model.into());
        self
    }

    pub fn call(self) -> CallFuture {
        self.ctx.call::<AgentRunProtocol>(self.request)
    }
}
