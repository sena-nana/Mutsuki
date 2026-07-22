use std::collections::BTreeMap;

use mutsuki_bot_protocol::{
    BOT_EVENT_HANDLE_PROTOCOL_ID, BOT_EVENT_INGEST_PROTOCOL_ID, BotEvent, BotEventSubscription,
};
use mutsuki_runtime_contracts::{
    CompletionBatch, ERR_RUNTIME_HOST_FAILED, ExecutionClass, InvocationMode, OrderingRequirement,
    PluginManifest, RunnerBatchCapability, RunnerConcurrency, RunnerControlCapability,
    RunnerDescriptor, RunnerMode, RunnerOrderingCapability, RunnerPayloadCapability, RunnerPurity,
    RunnerResourceCapability, RunnerResult, RunnerSideEffect, RuntimeError, ScalarValue, Task,
    WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::{PluginBuilder, map_work_batch_entries};
use serde_json::json;

use crate::{build_dispatch_task, matches_subscription};

pub const BOT_EVENT_ROUTER_PLUGIN_ID: &str = "mutsuki.bot.router.event";
pub const BOT_EVENT_ROUTER_RUNNER_ID: &str = "mutsuki.bot.router.event.ingest";

pub fn bot_event_router_manifest(plugin_generation: u64) -> PluginManifest {
    PluginBuilder::new(BOT_EVENT_ROUTER_PLUGIN_ID)
        .runner_descriptor(router_descriptor(plugin_generation))
        .build()
        .manifest
}

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
        parent: &Task,
        event: &BotEvent,
        registry_generation: u64,
    ) -> Vec<Task> {
        let mut tasks = Vec::new();
        for subscription in &self.subscriptions {
            if matches_subscription(event, subscription) {
                self.next_sequence += 1;
                tasks.push(build_dispatch_task(
                    parent,
                    event,
                    subscription,
                    self.next_sequence,
                    registry_generation,
                ));
            }
        }
        tasks
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

    fn run_batch(
        &mut self,
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            let event = task
                .payload
                .decode_shared::<BotEvent>()
                .map_err(|error| failure("mutsuki.bot.router.event.decode", error))?;
            let mut dispatch_tasks =
                self.router
                    .route(task, event.as_ref(), ctx.registry_generation);
            for child in &mut dispatch_tasks {
                child.trace_id = task.trace_id.clone();
                child.correlation_id = task.correlation_id.clone();
            }
            tracing::info!(
                account_id = %event.bot.account_id,
                event_id = %event.event_id,
                task_id = %task.task_id,
                runner_id = BOT_EVENT_ROUTER_RUNNER_ID,
                correlation_id = task.correlation_id.as_deref().unwrap_or(""),
                dispatched_tasks = dispatch_tasks.len(),
                "Bot event routed"
            );
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.tasks = dispatch_tasks;
            Ok(result)
        })
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
        invocation_mode: InvocationMode::SyncExclusive,
        concurrency: RunnerConcurrency::Exclusive,
        input_schema: json!({
            "type": "object",
            "required": ["event_id", "platform", "kind", "target"]
        }),
        output_schema: json!({
            "tasks": [BOT_EVENT_HANDLE_PROTOCOL_ID]
        }),
        batch: RunnerBatchCapability {
            mode: RunnerMode::NativeBatch,
            preferred_batch_size: 32,
            max_batch_entries: 128,
            max_entry_concurrency: 32,
            max_inflight_batches: 1,
            side_effect: RunnerSideEffect::None,
            ..Default::default()
        },
        payload: RunnerPayloadCapability::default(),
        resources: RunnerResourceCapability {
            requires_resource_plan: false,
            ..Default::default()
        },
        ordering: RunnerOrderingCapability {
            default: OrderingRequirement::PreserveSubmitOrder,
            supports_sequence: true,
            supports_same_resource_order: true,
        },
        control: RunnerControlCapability::default(),
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

fn failure(route: impl Into<String>, error: impl std::fmt::Display) -> RuntimeError {
    let mut runtime_error = RuntimeError::new(
        ERR_RUNTIME_HOST_FAILED,
        BOT_EVENT_ROUTER_PLUGIN_ID,
        route.into(),
    );
    runtime_error
        .evidence
        .insert("message".into(), ScalarValue::String(error.to_string()));
    runtime_error
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_bot_protocol::{BotAccountRef, BotEventKind, BotPlatform, BotTarget};
    use mutsuki_runtime_contracts::{
        BatchEntry, BatchPayload, DispatchLane, OrderingRequirement, WorkResourcePlan,
    };

    #[test]
    fn router_batches_events_and_isolates_decode_failure() {
        let mut runner = BotEventRouterRunner::new(
            1,
            vec![BotEventSubscription::new(
                "all-events",
                BOT_EVENT_HANDLE_PROTOCOL_ID,
            )],
        );
        let valid_a = event_task("task-a", "event-a");
        let invalid = Task::new("task-invalid", BOT_EVENT_INGEST_PROTOCOL_ID, json!({}));
        let valid_b = event_task("task-b", "event-b");

        let completion = runner
            .run_batch(test_context(7, 3), batch(vec![valid_a, invalid, valid_b]))
            .unwrap();

        assert_eq!(completion.results.len(), 3);
        assert!(completion.results[1].result.is_none());
        assert!(completion.results[1].error.is_some());
        for index in [0, 2] {
            let result = completion.results[index].result.as_ref().unwrap();
            assert_eq!(result.tasks.len(), 1);
            assert_eq!(result.tasks[0].registry_generation, 7);
            assert_eq!(result.tasks[0].protocol_id, BOT_EVENT_HANDLE_PROTOCOL_ID);
        }
    }

    fn event_task(task_id: &str, event_id: &str) -> Task {
        Task::new(
            task_id,
            BOT_EVENT_INGEST_PROTOCOL_ID,
            mutsuki_runtime_contracts::TaskPayload::from_local(BotEvent {
                event_id: event_id.into(),
                platform: BotPlatform::QqBot,
                bot: BotAccountRef {
                    account_id: "main".into(),
                    platform: BotPlatform::QqBot,
                },
                kind: BotEventKind::BotConnected,
                time_ms: 1,
                target: BotTarget::User {
                    user_id: "user".into(),
                },
                actor: None,
                message: None,
                raw: None,
                ext: Default::default(),
            }),
        )
    }

    fn batch(tasks: Vec<Task>) -> WorkBatch {
        WorkBatch {
            batch_id: "batch:router".into(),
            tick_id: "tick:router".into(),
            batch_key: BOT_EVENT_ROUTER_RUNNER_ID.into(),
            entries: entries(&tasks),
            payload: BatchPayload::from_tasks(&tasks),
            resource_plan: WorkResourcePlan::empty(),
            task_leases: Vec::new(),
        }
    }

    fn entries(tasks: &[Task]) -> Vec<BatchEntry> {
        tasks
            .iter()
            .enumerate()
            .map(|(index, task)| BatchEntry {
                entry_id: format!("entry-{index}"),
                task_id: task.task_id.clone(),
                trace_id: None,
                parent_id: None,
                payload_index: index,
                resource_requirement_indices: Vec::new(),
                cancel_index: None,
                deadline_tick: None,
                priority: 0,
                lane: DispatchLane::Normal,
                ordering: OrderingRequirement::PreserveSubmitOrder,
            })
            .collect()
    }

    fn test_context(registry_generation: u64, entry_count: usize) -> RunnerContext {
        RunnerContext::new(
            registry_generation,
            1,
            "executor:router",
            Vec::<String>::new(),
            "batch:router",
        )
        .with_batch("batch:router", entry_count)
    }
}
