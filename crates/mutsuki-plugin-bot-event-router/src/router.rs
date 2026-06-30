use std::collections::BTreeMap;

use mutsuki_bot_protocol::{BOT_EVENT_HANDLE_PROTOCOL_ID, BOT_EVENT_INGEST_PROTOCOL_ID, BotEvent};
use mutsuki_runtime_contracts::{
    ERR_RUNTIME_HOST_FAILED, ExecutionClass, RunnerDescriptor, RunnerPurity, RunnerResult,
    RuntimeError, ScalarValue, Task,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeFailure, RuntimeResult};
use serde_json::json;

use crate::{BotEventSubscription, build_dispatch_task, matches_subscription};

pub const BOT_EVENT_ROUTER_PLUGIN_ID: &str = "mutsuki.bot.router.event";
pub const BOT_EVENT_ROUTER_RUNNER_ID: &str = "mutsuki.bot.router.event.ingest";

pub struct BotEventRouter {
    subscriptions: Vec<BotEventSubscription>,
    next_sequence: u64,
}

impl BotEventRouter {
    pub fn new(subscriptions: Vec<BotEventSubscription>) -> Self {
        Self {
            subscriptions,
            next_sequence: 0,
        }
    }

    pub fn route(
        &mut self,
        event: &BotEvent,
        registry_generation: u64,
    ) -> Result<Vec<Task>, serde_json::Error> {
        let mut tasks = Vec::new();
        for subscription in &self.subscriptions {
            if matches_subscription(event, subscription) {
                self.next_sequence += 1;
                tasks.push(build_dispatch_task(
                    event,
                    subscription,
                    self.next_sequence,
                    registry_generation,
                )?);
            }
        }
        Ok(tasks)
    }
}

pub struct BotEventRouterRunner {
    descriptor: RunnerDescriptor,
    router: BotEventRouter,
}

impl BotEventRouterRunner {
    pub fn new(plugin_generation: u64, subscriptions: Vec<BotEventSubscription>) -> Self {
        Self {
            descriptor: router_descriptor(plugin_generation),
            router: BotEventRouter::new(subscriptions),
        }
    }
}

impl Runner for BotEventRouterRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn step(&mut self, ctx: RunnerContext, tasks: Vec<Task>) -> RuntimeResult<Vec<RunnerResult>> {
        tasks
            .into_iter()
            .map(|task| {
                let event: BotEvent = serde_json::from_value(task.payload.clone())
                    .map_err(|error| failure("mutsuki.bot.router.event.decode", error))?;
                let dispatch_tasks = self
                    .router
                    .route(&event, ctx.registry_generation)
                    .map_err(|error| failure("mutsuki.bot.router.event.dispatch", error))?;
                let mut result = RunnerResult::completed(task.task_id);
                result.tasks = dispatch_tasks;
                Ok(result)
            })
            .collect()
    }
}

pub fn router_descriptor(plugin_generation: u64) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: BOT_EVENT_ROUTER_RUNNER_ID.into(),
        plugin_id: BOT_EVENT_ROUTER_PLUGIN_ID.into(),
        plugin_generation,
        accepted_protocol_ids: vec![BOT_EVENT_INGEST_PROTOCOL_ID.into()],
        purity: RunnerPurity::Pure,
        execution_class: ExecutionClass::Orchestration,
        input_schema: json!({
            "type": "object",
            "required": ["event_id", "platform", "kind", "target"]
        }),
        output_schema: json!({
            "tasks": [BOT_EVENT_HANDLE_PROTOCOL_ID]
        }),
        metadata: BTreeMap::from([(
            "description".into(),
            ScalarValue::String("Bot event subscription router".into()),
        )]),
        contract_surfaces: vec![
            format!("runner:{BOT_EVENT_ROUTER_RUNNER_ID}"),
            format!("task_protocol:{BOT_EVENT_INGEST_PROTOCOL_ID}"),
        ],
    }
}

fn failure(route: impl Into<String>, error: impl std::fmt::Display) -> RuntimeFailure {
    let mut runtime_error = RuntimeError::new(
        ERR_RUNTIME_HOST_FAILED,
        BOT_EVENT_ROUTER_PLUGIN_ID,
        route.into(),
    );
    runtime_error
        .evidence
        .insert("message".into(), ScalarValue::String(error.to_string()));
    RuntimeFailure::new(runtime_error)
}
