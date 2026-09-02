use std::sync::Arc;

use mutsuki_bot_delivery::{
    ActiveDeliveryService, BOT_SCHEDULED_DELIVERY_PROTOCOL_ID, DeliveryError, ReplyDeliveryService,
    ScheduledAgentDeliveryBridge, ScheduledDeliveryRequest,
};
use mutsuki_bot_protocol::{
    BOT_ACTIVE_DELIVERY_PROTOCOL_ID, BOT_MESSAGE_SEND_PROTOCOL_ID, BOT_REPLY_DELIVERY_PROTOCOL_ID,
    BotActiveDeliveryCommand, BotNodeInvocation, BotNodeResult, BotReplyDeliveryCommand,
    BotReplyDeliveryRequest,
};
use mutsuki_runtime_contracts::{
    ExecutionClass, InvocationMode, RunnerBatchCapability, RunnerConcurrency,
    RunnerControlCapability, RunnerMode, RunnerResult, RunnerSideEffect, Task, TimeoutGranularity,
};
use mutsuki_runtime_core::{Runner, RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{
    AsyncRunnerContext, RunnerDescriptorBuilder, RuntimeClientRef, TaskAwaitRunnerAdapter,
};

pub const BOT_SCHEDULED_DELIVERY_PLUGIN_ID: &str = "mutsuki.plugin.bot.delivery.scheduled";
pub const BOT_SCHEDULED_DELIVERY_RUNNER_ID: &str = "mutsuki.bot.delivery.scheduled.runner";
pub const BOT_DELIVERY_PLUGIN_ID: &str = "mutsuki.plugin.bot.delivery";
pub const BOT_DELIVERY_RUNNER_ID: &str = "mutsuki.bot.delivery.runner";
pub const BOT_REPLY_DELIVERY_PLUGIN_ID: &str = "mutsuki.plugin.bot.delivery.reply";
pub const BOT_REPLY_DELIVERY_RUNNER_ID: &str = "mutsuki.bot.delivery.reply.runner";

#[must_use]
pub fn scheduled_delivery_runner(
    client: RuntimeClientRef,
    bridge: ScheduledAgentDeliveryBridge,
) -> Box<dyn Runner> {
    let factory = Box::new(move |_ctx, task: Task| {
        let bridge = bridge.clone();
        Box::pin(async move { scheduled_delivery_result(&bridge, &task).await })
            as std::pin::Pin<
                Box<dyn std::future::Future<Output = RuntimeResult<RunnerResult>> + Send>,
            >
    });
    Box::new(
        TaskAwaitRunnerAdapter::new(scheduled_delivery_descriptor(), client, factory)
            .with_self_call_policy(false),
    )
}

async fn scheduled_delivery_result(
    bridge: &ScheduledAgentDeliveryBridge,
    task: &Task,
) -> RuntimeResult<RunnerResult> {
    let payload = task.payload.to_value();
    let request: ScheduledDeliveryRequest =
        if let Ok(invocation) = serde_json::from_value::<BotNodeInvocation>(payload.clone()) {
            serde_json::from_value(invocation.input.payload.value)
                .map_err(|error| runtime_error(task, error))?
        } else {
            serde_json::from_value(payload).map_err(|error| runtime_error(task, error))?
        };
    let receipt = bridge
        .deliver(request.result, request.now_unix_ms)
        .await
        .map_err(|error| runtime_error(task, error))?;
    let mut completed = RunnerResult::completed(task.task_id.clone());
    completed.output =
        Some(serde_json::to_value(receipt).map_err(|error| runtime_error(task, error))?);
    Ok(completed)
}

#[must_use]
pub fn scheduled_delivery_descriptor() -> mutsuki_runtime_contracts::RunnerDescriptor {
    RunnerDescriptorBuilder::new(
        BOT_SCHEDULED_DELIVERY_RUNNER_ID,
        BOT_SCHEDULED_DELIVERY_PLUGIN_ID,
    )
    .accepted_protocol(BOT_SCHEDULED_DELIVERY_PROTOCOL_ID)
    .execution_class(ExecutionClass::Blocking)
    .build()
}

#[must_use]
pub fn delivery_runner(
    client: RuntimeClientRef,
    service: ActiveDeliveryService,
) -> Box<dyn Runner> {
    let factory = Box::new(move |_ctx, task: Task| {
        let service = service.clone();
        Box::pin(async move { delivery_result(&service, &task).await })
            as std::pin::Pin<
                Box<dyn std::future::Future<Output = RuntimeResult<RunnerResult>> + Send>,
            >
    });
    Box::new(
        TaskAwaitRunnerAdapter::new(delivery_descriptor(), client, factory)
            .with_self_call_policy(false),
    )
}

#[must_use]
pub fn delivery_descriptor() -> mutsuki_runtime_contracts::RunnerDescriptor {
    RunnerDescriptorBuilder::new(BOT_DELIVERY_RUNNER_ID, BOT_DELIVERY_PLUGIN_ID)
        .accepted_protocol(BOT_ACTIVE_DELIVERY_PROTOCOL_ID)
        .execution_class(ExecutionClass::Blocking)
        .build()
}

#[must_use]
pub fn reply_delivery_runner(
    client: RuntimeClientRef,
    repository: Arc<dyn mutsuki_bot_delivery::ReplyDeliveryRepository>,
) -> Box<dyn Runner> {
    reply_delivery_runner_for(
        client,
        repository,
        BOT_REPLY_DELIVERY_PLUGIN_ID,
        BOT_REPLY_DELIVERY_RUNNER_ID,
    )
}

#[must_use]
pub fn reply_delivery_runner_for(
    client: RuntimeClientRef,
    repository: Arc<dyn mutsuki_bot_delivery::ReplyDeliveryRepository>,
    plugin_id: &str,
    runner_id: &str,
) -> Box<dyn Runner> {
    let service = ReplyDeliveryService::new(repository);
    let descriptor = reply_delivery_descriptor(plugin_id, runner_id);
    let factory = Box::new(move |ctx, task: Task| {
        let service = service.clone();
        Box::pin(async move { reply_delivery_result(&service, &ctx, &task).await })
            as std::pin::Pin<
                Box<dyn std::future::Future<Output = RuntimeResult<RunnerResult>> + Send>,
            >
    });
    Box::new(TaskAwaitRunnerAdapter::new(descriptor, client, factory).with_self_call_policy(false))
}

#[must_use]
pub fn reply_delivery_descriptor(
    plugin_id: &str,
    runner_id: &str,
) -> mutsuki_runtime_contracts::RunnerDescriptor {
    RunnerDescriptorBuilder::new(runner_id, plugin_id)
        .accepted_protocol(BOT_REPLY_DELIVERY_PROTOCOL_ID)
        .requires_protocol(BOT_MESSAGE_SEND_PROTOCOL_ID)
        .execution_class(ExecutionClass::Orchestration)
        .invocation_mode(InvocationMode::AsyncReentrant)
        .concurrency(RunnerConcurrency::Reentrant {
            max_inflight_batches: 128,
            max_inflight_entries: 128,
        })
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::NativeBatch,
            preferred_batch_size: 1,
            max_batch_entries: 1,
            max_entry_concurrency: 1,
            max_inflight_batches: 128,
            side_effect: RunnerSideEffect::External,
            ..RunnerBatchCapability::default()
        })
        .control_capability(RunnerControlCapability {
            entry_cancel: true,
            batch_cancel: true,
            timeout_granularity: TimeoutGranularity::Entry,
        })
        .build()
}

async fn delivery_result(
    service: &ActiveDeliveryService,
    task: &Task,
) -> RuntimeResult<RunnerResult> {
    let command: BotActiveDeliveryCommand = serde_json::from_value(task.payload.to_value())
        .map_err(|error| runtime_error(task, error))?;
    let output = match command {
        BotActiveDeliveryCommand::Submit {
            request,
            now_unix_ms,
        } => serde_json::to_value(
            service
                .submit(&request, now_unix_ms)
                .await
                .map_err(|error| runtime_error(task, error))?,
        ),
        BotActiveDeliveryCommand::ResumeDue { now_unix_ms } => serde_json::to_value(
            service
                .resume_due(now_unix_ms)
                .await
                .map_err(|error| runtime_error(task, error))?,
        ),
        BotActiveDeliveryCommand::Inspect { delivery_id } => serde_json::to_value(
            service
                .inspect(&delivery_id)
                .await
                .map_err(|error| runtime_error(task, error))?,
        ),
        BotActiveDeliveryCommand::Preview { delivery_id } => serde_json::to_value(
            service
                .preview(&delivery_id)
                .await
                .map_err(|error| runtime_error(task, error))?,
        ),
        BotActiveDeliveryCommand::Retry {
            delivery_id,
            now_unix_ms,
        } => serde_json::to_value(
            service
                .retry(&delivery_id, now_unix_ms)
                .await
                .map_err(|error| runtime_error(task, error))?,
        ),
        BotActiveDeliveryCommand::Cancel { delivery_id } => serde_json::to_value(
            service
                .cancel(&delivery_id)
                .await
                .map_err(|error| runtime_error(task, error))?,
        ),
    }
    .map_err(|error| runtime_error(task, error))?;
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.output = Some(output);
    Ok(result)
}

async fn reply_delivery_result(
    service: &ReplyDeliveryService,
    ctx: &AsyncRunnerContext,
    task: &Task,
) -> RuntimeResult<RunnerResult> {
    let (command, node_invocation) =
        match serde_json::from_value::<BotNodeInvocation>(task.payload.to_value()) {
            Ok(invocation) => {
                let request = serde_json::from_value::<BotReplyDeliveryRequest>(
                    invocation.input.payload.value.clone(),
                )
                .map_err(|error| runtime_error(task, error))?;
                (
                    BotReplyDeliveryCommand::Submit {
                        request: Box::new(request),
                        now_unix_ms: unix_ms(),
                    },
                    true,
                )
            }
            Err(_) => (
                serde_json::from_value(task.payload.to_value())
                    .map_err(|error| runtime_error(task, error))?,
                false,
            ),
        };
    let output = match command {
        BotReplyDeliveryCommand::Reserve { request } => serde_json::to_value(
            service
                .reserve(&request)
                .await
                .map_err(|error| runtime_delivery_error(task, error))?,
        ),
        BotReplyDeliveryCommand::Submit {
            request,
            now_unix_ms,
        } => serde_json::to_value(
            service
                .submit(ctx, &request, now_unix_ms)
                .await
                .map_err(|error| runtime_delivery_error(task, error))?,
        ),
        BotReplyDeliveryCommand::ResumeDue { now_unix_ms } => serde_json::to_value(
            service
                .resume_due(ctx, now_unix_ms)
                .await
                .map_err(|error| runtime_delivery_error(task, error))?,
        ),
        BotReplyDeliveryCommand::Inspect { reply_id } => serde_json::to_value(
            service
                .inspect(&reply_id)
                .await
                .map_err(|error| runtime_delivery_error(task, error))?,
        ),
        BotReplyDeliveryCommand::RetryPart {
            delivery_id,
            now_unix_ms,
        } => serde_json::to_value(
            service
                .retry_part(ctx, &delivery_id, now_unix_ms)
                .await
                .map_err(|error| runtime_delivery_error(task, error))?,
        ),
        BotReplyDeliveryCommand::CancelPart { delivery_id } => serde_json::to_value(
            service
                .cancel_part(&delivery_id)
                .await
                .map_err(|error| runtime_delivery_error(task, error))?,
        ),
    }
    .map_err(|error| runtime_error(task, error))?;
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.output = Some(if node_invocation {
        serde_json::to_value(BotNodeResult {
            outputs: Vec::new(),
            metadata: std::collections::BTreeMap::from([("receipt".into(), output)]),
        })
        .map_err(|error| runtime_error(task, error))?
    } else {
        output
    });
    Ok(result)
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

fn runtime_error(task: &Task, error: impl std::fmt::Display) -> RuntimeFailure {
    RuntimeFailure::new(mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        BOT_DELIVERY_PLUGIN_ID,
        format!("{}.{}", task.task_id, error),
    ))
}

#[allow(clippy::needless_pass_by_value)]
fn runtime_delivery_error(task: &Task, error: DeliveryError) -> RuntimeFailure {
    RuntimeFailure::new(mutsuki_runtime_contracts::RuntimeError::new(
        error.code(),
        BOT_REPLY_DELIVERY_PLUGIN_ID,
        format!("{}.{}", task.task_id, error),
    ))
}
