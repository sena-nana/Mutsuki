use serde::Serialize;

use mutsuki_runtime_sdk::{AsyncRunnerContext, CallFuture};

#[derive(Clone)]
pub struct AgentToolContext {
    ctx: AsyncRunnerContext,
}

impl AgentToolContext {
    pub fn new(ctx: AsyncRunnerContext) -> Self {
        Self { ctx }
    }

    pub fn task_id(&self) -> &str {
        self.ctx.task_id()
    }

    /// Nested calls inherit `trace_id` / `correlation_id` from the runner context.
    pub fn call<P>(&self, input: impl Serialize) -> CallFuture
    where
        P: mutsuki_runtime_sdk::SdkProtocol,
    {
        self.ctx.call::<P>(input)
    }

    pub fn call_raw(
        &self,
        protocol_id: impl Into<String>,
        payload: serde_json::Value,
    ) -> CallFuture {
        self.ctx.call_raw(protocol_id, payload)
    }
}
