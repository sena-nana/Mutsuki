use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use mutsuki_bot_protocol::{
    BOT_EVENT_INGEST_PROTOCOL_ID, BOT_PERMISSION_CHECK_PROTOCOL_ID,
    BOT_RATE_LIMIT_CHECK_PROTOCOL_ID, BotConversationKind, BotCustomPredicateRequest,
    BotCustomPredicateResult, BotEvent, BotFilterExpr, BotHandlerDescriptor,
    BotHandlerExecutionResult, BotHandlerHookRequest, BotHandlerOutcome, BotPermissionCheckRequest,
    BotPermissionCheckResult, BotPropagationPolicy, BotRateLimitCheckRequest,
    BotRateLimitCheckResult, BotTarget, MessageSegment,
};
use mutsuki_runtime_contracts::{
    ExecutionClass, InvocationMode, PluginManifest, RunnerBatchCapability, RunnerConcurrency,
    RunnerControlCapability, RunnerMode, RunnerResult, RunnerSideEffect, Task, TaskOutcome,
    TimeoutGranularity,
};
use mutsuki_runtime_core::Runner;
use mutsuki_runtime_sdk::{
    AsyncRunnerContext, BoxedTaskAwaitRunner, PluginBuilder, ProtocolDescriptorBuilder,
    RunnerDescriptorBuilder, RuntimeClientRef, RuntimeFailure, RuntimeResult,
    TaskAwaitRunnerAdapter,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

pub const BOT_HANDLER_PIPELINE_PLUGIN_ID: &str = "mutsuki.bot.router.handler";
pub const BOT_HANDLER_PIPELINE_RUNNER_ID: &str = "mutsuki.bot.router.handler.pipeline";

#[must_use]
pub fn handler_pipeline_manifest() -> PluginManifest {
    handler_pipeline_manifest_for(
        BOT_HANDLER_PIPELINE_PLUGIN_ID,
        BOT_HANDLER_PIPELINE_RUNNER_ID,
    )
}

#[must_use]
pub fn handler_pipeline_manifest_for(plugin_id: &str, runner_id: &str) -> PluginManifest {
    PluginBuilder::new(plugin_id)
        .runner_descriptor(handler_pipeline_descriptor_for(plugin_id, runner_id))
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_EVENT_INGEST_PROTOCOL_ID).build(),
            runner_id,
            "bot-handler-pipeline",
        )
        .build()
        .manifest
}

pub fn handler_pipeline_runner(
    client: RuntimeClientRef,
    handlers: Vec<BotHandlerDescriptor>,
) -> Box<dyn Runner> {
    handler_pipeline_runner_for(
        client,
        handlers,
        BOT_HANDLER_PIPELINE_PLUGIN_ID,
        BOT_HANDLER_PIPELINE_RUNNER_ID,
    )
}

pub fn handler_pipeline_runner_for(
    client: RuntimeClientRef,
    mut handlers: Vec<BotHandlerDescriptor>,
    plugin_id: &str,
    runner_id: &str,
) -> Box<dyn Runner> {
    handlers.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| left.handler_id.cmp(&right.handler_id))
    });
    let ledger = Arc::new(HandlerExecutionLedger::new(65_536));
    let factory: BoxedTaskAwaitRunner = Box::new(move |ctx: AsyncRunnerContext, task: Task| {
        let handlers = handlers.clone();
        let ledger = ledger.clone();
        Box::pin(run_pipeline(ctx, task, handlers, ledger))
    });
    Box::new(
        TaskAwaitRunnerAdapter::new(
            handler_pipeline_descriptor_for(plugin_id, runner_id),
            client,
            factory,
        )
        .with_self_call_policy(false),
    )
}

fn handler_pipeline_descriptor_for(
    plugin_id: &str,
    runner_id: &str,
) -> mutsuki_runtime_contracts::RunnerDescriptor {
    RunnerDescriptorBuilder::new(runner_id, plugin_id)
        .accepted_protocol(BOT_EVENT_INGEST_PROTOCOL_ID)
        .execution_class(ExecutionClass::Orchestration)
        .invocation_mode(InvocationMode::AsyncReentrant)
        .concurrency(RunnerConcurrency::Reentrant {
            max_inflight_batches: 16,
            max_inflight_entries: 128,
        })
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::NativeBatch,
            preferred_batch_size: 32,
            max_batch_entries: 128,
            max_entry_concurrency: 128,
            max_inflight_batches: 16,
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

async fn run_pipeline(
    ctx: AsyncRunnerContext,
    task: Task,
    handlers: Vec<BotHandlerDescriptor>,
    ledger: Arc<HandlerExecutionLedger>,
) -> RuntimeResult<RunnerResult> {
    let event = task
        .payload
        .decode_shared::<BotEvent>()
        .map_err(|error| failure(&task, "event.decode", error))?;
    let mut invoked = 0_u64;
    let mut handler_errors = Vec::new();
    let mut handler_denials = Vec::new();
    for handler in handlers {
        if !handler_matches(&handler, event.as_ref()) {
            continue;
        }
        match filter_allowed(&ctx, &handler, event.as_ref()).await {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => {
                record_handler_failure(&mut handler_errors, &ctx, &handler, event.as_ref(), &error)
                    .await;
                continue;
            }
        }
        match permission_check(&ctx, event.as_ref(), &handler, &task).await {
            Ok(Some(check)) if !check.allowed => {
                handler_denials.push(serde_json::json!({
                    "handler_id": handler.handler_id,
                    "kind": "permission",
                    "code": check.denial_code,
                    "reason": check.audit_reason,
                }));
                continue;
            }
            Ok(_) => {}
            Err(error) => {
                record_handler_failure(&mut handler_errors, &ctx, &handler, event.as_ref(), &error)
                    .await;
                continue;
            }
        }
        match rate_limit_check(&ctx, event.as_ref(), &handler, &task).await {
            Ok(Some(check)) if !check.allowed => {
                handler_denials.push(serde_json::json!({
                    "handler_id": handler.handler_id,
                    "kind": "rate_limit",
                    "code": check.denial_code,
                    "reason": check.audit_reason,
                    "retry_after_ms": check.retry_after_ms,
                }));
                continue;
            }
            Ok(_) => {}
            Err(error) => {
                record_handler_failure(&mut handler_errors, &ctx, &handler, event.as_ref(), &error)
                    .await;
                continue;
            }
        }
        let mut claim = match ledger.claim(&handler, &event.bot.account_id, &event.event_id) {
            HandlerClaim::Acquired(claim) => claim,
            HandlerClaim::Duplicate => {
                handler_denials.push(serde_json::json!({
                    "handler_id": handler.handler_id,
                    "kind": "duplicate",
                    "code": "bot.handler.duplicate",
                }));
                continue;
            }
            HandlerClaim::ConcurrencyLimited => {
                handler_denials.push(serde_json::json!({
                    "handler_id": handler.handler_id,
                    "kind": "concurrency",
                    "code": "bot.handler.concurrency_limited",
                }));
                continue;
            }
        };
        if let Err(error) = invoke_hooks(&ctx, &handler, event.as_ref(), "before", None).await {
            record_handler_failure(&mut handler_errors, &ctx, &handler, event.as_ref(), &error)
                .await;
            continue;
        }
        let payload = match serde_json::to_value(event.as_ref()) {
            Ok(payload) => payload,
            Err(error) => {
                let error = failure(&task, "handler.encode", error);
                record_handler_failure(&mut handler_errors, &ctx, &handler, event.as_ref(), &error)
                    .await;
                continue;
            }
        };
        let outcome = match ctx
            .call_targeted_raw(
                handler.binding_id.clone(),
                handler.handler_protocol_id.clone(),
                handler
                    .runner_hint
                    .clone()
                    .unwrap_or_else(|| handler.handler_id.clone()),
                payload,
            )
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                // A runtime/transport failure is local to this handler. The parent
                // event must still be offered to lower-priority handlers.
                record_handler_failure(&mut handler_errors, &ctx, &handler, event.as_ref(), &error)
                    .await;
                continue;
            }
        };
        invoked += 1;
        let explicit = match decode_optional::<BotHandlerExecutionResult>(&outcome, &task) {
            Ok(result) => result.map(|result| result.outcome),
            Err(error) => {
                record_handler_failure(&mut handler_errors, &ctx, &handler, event.as_ref(), &error)
                    .await;
                continue;
            }
        };
        claim.commit();
        if let Err(error) = invoke_hooks(&ctx, &handler, event.as_ref(), "after", None).await {
            record_handler_failure(&mut handler_errors, &ctx, &handler, event.as_ref(), &error)
                .await;
        }
        if matches!(
            explicit,
            Some(BotHandlerOutcome::Stop | BotHandlerOutcome::Consume)
        ) || matches!(
            handler.propagation,
            BotPropagationPolicy::StopOnSuccess | BotPropagationPolicy::ConsumeOnSuccess
        ) {
            break;
        }
    }
    let mut result = RunnerResult::completed(task.task_id);
    result.output = Some(serde_json::json!({
        "invoked_handlers": invoked,
        "handler_errors": handler_errors,
        "handler_denials": handler_denials,
    }));
    Ok(result)
}

async fn record_handler_failure(
    handler_errors: &mut Vec<Value>,
    ctx: &AsyncRunnerContext,
    handler: &BotHandlerDescriptor,
    event: &BotEvent,
    error: &RuntimeFailure,
) {
    let code = error.error().code.clone();
    record_hook_failure(
        handler_errors,
        ctx,
        handler,
        event,
        "error",
        Some(code.clone()),
    )
    .await;
    handler_errors.push(serde_json::json!({
        "handler_id": handler.handler_id,
        "code": code,
    }));
}

async fn record_hook_failure(
    handler_errors: &mut Vec<Value>,
    ctx: &AsyncRunnerContext,
    handler: &BotHandlerDescriptor,
    event: &BotEvent,
    phase: &str,
    error_code: Option<String>,
) {
    if let Err(error) = invoke_hooks(ctx, handler, event, phase, error_code).await {
        handler_errors.push(serde_json::json!({
            "handler_id": handler.handler_id,
            "phase": format!("{phase}_hook"),
            "code": error.error().code,
        }));
    }
}

struct HandlerExecutionLedger {
    capacity: usize,
    state: Mutex<HandlerExecutionState>,
}

#[derive(Default)]
struct HandlerExecutionState {
    completed: BTreeSet<String>,
    completed_order: VecDeque<String>,
    inflight: BTreeSet<String>,
    inflight_by_handler: BTreeMap<String, u32>,
}

enum HandlerClaim {
    Acquired(HandlerExecutionClaim),
    Duplicate,
    ConcurrencyLimited,
}

struct HandlerExecutionClaim {
    ledger: Arc<HandlerExecutionLedger>,
    key: String,
    handler_id: String,
    committed: bool,
}

impl HandlerExecutionLedger {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::new(HandlerExecutionState::default()),
        }
    }

    fn claim(
        self: &Arc<Self>,
        handler: &BotHandlerDescriptor,
        account_id: &str,
        event_id: &str,
    ) -> HandlerClaim {
        let key = format!(
            "{}:{}:{}:{}",
            handler.handler_id, handler.generation, account_id, event_id
        );
        let mut state = self.state.lock().expect("handler ledger mutex poisoned");
        if state.completed.contains(&key) || state.inflight.contains(&key) {
            return HandlerClaim::Duplicate;
        }
        let inflight = state
            .inflight_by_handler
            .get(&handler.handler_id)
            .copied()
            .unwrap_or(0);
        if handler
            .max_concurrency
            .is_some_and(|limit| inflight >= limit.max(1))
        {
            return HandlerClaim::ConcurrencyLimited;
        }
        state.inflight.insert(key.clone());
        *state
            .inflight_by_handler
            .entry(handler.handler_id.clone())
            .or_default() += 1;
        HandlerClaim::Acquired(HandlerExecutionClaim {
            ledger: self.clone(),
            key,
            handler_id: handler.handler_id.clone(),
            committed: false,
        })
    }
}

impl HandlerExecutionClaim {
    fn commit(&mut self) {
        let mut state = self
            .ledger
            .state
            .lock()
            .expect("handler ledger mutex poisoned");
        release_inflight(&mut state, &self.key, &self.handler_id);
        state.completed.insert(self.key.clone());
        state.completed_order.push_back(self.key.clone());
        while state.completed_order.len() > self.ledger.capacity {
            if let Some(expired) = state.completed_order.pop_front() {
                state.completed.remove(&expired);
            }
        }
        self.committed = true;
    }
}

impl Drop for HandlerExecutionClaim {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let mut state = self
            .ledger
            .state
            .lock()
            .expect("handler ledger mutex poisoned");
        release_inflight(&mut state, &self.key, &self.handler_id);
    }
}

fn release_inflight(state: &mut HandlerExecutionState, key: &str, handler_id: &str) {
    state.inflight.remove(key);
    if let Some(inflight) = state.inflight_by_handler.get_mut(handler_id) {
        *inflight = inflight.saturating_sub(1);
        if *inflight == 0 {
            state.inflight_by_handler.remove(handler_id);
        }
    }
}

async fn invoke_hooks(
    ctx: &AsyncRunnerContext,
    handler: &BotHandlerDescriptor,
    event: &BotEvent,
    phase: &str,
    error_code: Option<String>,
) -> RuntimeResult<()> {
    let protocols = match phase {
        "before" => &handler.before_hook_protocol_ids,
        "after" => &handler.after_hook_protocol_ids,
        "error" => &handler.error_hook_protocol_ids,
        _ => return Ok(()),
    };
    for protocol_id in protocols {
        let outcome = ctx
            .call_raw(
                protocol_id.clone(),
                serde_json::to_value(BotHandlerHookRequest {
                    handler_id: handler.handler_id.clone(),
                    phase: phase.into(),
                    event: event.clone(),
                    error_code: error_code.clone(),
                })
                .map_err(|error| failure_raw(&handler.handler_id, "hook.encode", error))?,
            )
            .await?;
        decode_optional::<serde_json::Value>(
            &outcome,
            &Task::new(&handler.handler_id, protocol_id, serde_json::Value::Null),
        )?;
    }
    Ok(())
}

async fn permission_check(
    ctx: &AsyncRunnerContext,
    event: &BotEvent,
    handler: &BotHandlerDescriptor,
    task: &Task,
) -> RuntimeResult<Option<BotPermissionCheckResult>> {
    if handler.permissions.is_empty() {
        return Ok(None);
    }
    let outcome = ctx
        .call_raw(
            BOT_PERMISSION_CHECK_PROTOCOL_ID,
            serde_json::to_value(BotPermissionCheckRequest {
                handler: handler.clone(),
                event: event.clone(),
            })
            .map_err(|error| failure(task, "permission.encode", error))?,
        )
        .await?;
    Ok(Some(decode_required::<BotPermissionCheckResult>(
        &outcome, task,
    )?))
}

async fn rate_limit_check(
    ctx: &AsyncRunnerContext,
    event: &BotEvent,
    handler: &BotHandlerDescriptor,
    task: &Task,
) -> RuntimeResult<Option<BotRateLimitCheckResult>> {
    let Some(policy) = handler.rate_limit.clone() else {
        return Ok(None);
    };
    let outcome = ctx
        .call_raw(
            BOT_RATE_LIMIT_CHECK_PROTOCOL_ID,
            serde_json::to_value(BotRateLimitCheckRequest {
                handler_id: handler.handler_id.clone(),
                policy,
                event: event.clone(),
            })
            .map_err(|error| failure(task, "rate_limit.encode", error))?,
        )
        .await?;
    Ok(Some(decode_required::<BotRateLimitCheckResult>(
        &outcome, task,
    )?))
}

fn decode_required<T: DeserializeOwned>(outcome: &TaskOutcome, task: &Task) -> RuntimeResult<T> {
    decode_optional(outcome, task)?.ok_or_else(|| failure(task, "child.output", "missing output"))
}

fn decode_optional<T: DeserializeOwned>(
    outcome: &TaskOutcome,
    task: &Task,
) -> RuntimeResult<Option<T>> {
    match outcome {
        TaskOutcome::Completed { output, .. } => output
            .clone()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| failure(task, "child.output", error)),
        TaskOutcome::Failed { error, .. } => Err(RuntimeFailure::new(error.clone())),
        TaskOutcome::Cancelled { .. } => Err(failure(task, "child.cancelled", "child cancelled")),
        TaskOutcome::Expired { .. } => Err(failure(task, "child.expired", "child expired")),
        TaskOutcome::DeadLetter { .. } => {
            Err(failure(task, "child.dead_letter", "child dead-lettered"))
        }
    }
}

pub fn handler_matches(handler: &BotHandlerDescriptor, event: &BotEvent) -> bool {
    (handler.event_kinds.is_empty() || handler.event_kinds.contains(&event.kind))
        && (handler.conversation_kinds.is_empty()
            || handler
                .conversation_kinds
                .contains(&conversation_kind(&event.target)))
        && handler
            .filter
            .as_ref()
            .is_none_or(|filter| static_filter_match(filter, event) != StaticFilterMatch::NoMatch)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StaticFilterMatch {
    Match,
    NoMatch,
    Unknown,
}

fn static_filter_match(filter: &BotFilterExpr, event: &BotEvent) -> StaticFilterMatch {
    match filter {
        BotFilterExpr::All { filters } => {
            let mut unknown = false;
            for filter in filters {
                match static_filter_match(filter, event) {
                    StaticFilterMatch::NoMatch => return StaticFilterMatch::NoMatch,
                    StaticFilterMatch::Unknown => unknown = true,
                    StaticFilterMatch::Match => {}
                }
            }
            if unknown {
                StaticFilterMatch::Unknown
            } else {
                StaticFilterMatch::Match
            }
        }
        BotFilterExpr::Any { filters } => {
            let mut unknown = false;
            for filter in filters {
                match static_filter_match(filter, event) {
                    StaticFilterMatch::Match => return StaticFilterMatch::Match,
                    StaticFilterMatch::Unknown => unknown = true,
                    StaticFilterMatch::NoMatch => {}
                }
            }
            if unknown {
                StaticFilterMatch::Unknown
            } else {
                StaticFilterMatch::NoMatch
            }
        }
        BotFilterExpr::Not { filter } => match static_filter_match(filter, event) {
            StaticFilterMatch::Match => StaticFilterMatch::NoMatch,
            StaticFilterMatch::NoMatch => StaticFilterMatch::Match,
            StaticFilterMatch::Unknown => StaticFilterMatch::Unknown,
        },
        BotFilterExpr::CustomPredicate { .. } => StaticFilterMatch::Unknown,
        other => {
            if filter_matches(other, event) {
                StaticFilterMatch::Match
            } else {
                StaticFilterMatch::NoMatch
            }
        }
    }
}

fn filter_matches(filter: &BotFilterExpr, event: &BotEvent) -> bool {
    match filter {
        BotFilterExpr::All { filters } => {
            filters.iter().all(|filter| filter_matches(filter, event))
        }
        BotFilterExpr::Any { filters } => {
            filters.iter().any(|filter| filter_matches(filter, event))
        }
        BotFilterExpr::Not { filter } => !filter_matches(filter, event),
        BotFilterExpr::ConversationKind { kind } => conversation_kind(&event.target) == *kind,
        BotFilterExpr::EventKind { kind } => event.kind == *kind,
        BotFilterExpr::MustMentionBot => {
            event.message.as_ref().is_some_and(|message| {
                message.segments.iter().any(|segment| {
                    matches!(
                        segment,
                        MessageSegment::MentionUser { user_id }
                            if user_id == &event.bot.account_id
                    )
                })
            }) || event
                .ext
                .get("bot.mentioned_bot")
                .or_else(|| event.ext.get("qqbot.mentioned_bot"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        }
        BotFilterExpr::IsReply => event
            .message
            .as_ref()
            .is_some_and(|message| message.reply_to.is_some()),
        BotFilterExpr::ActorRole { role } => event
            .ext
            .get("bot.actor_role")
            .or_else(|| event.ext.get("qqbot.actor_role"))
            .and_then(Value::as_str)
            .is_some_and(|value| value == format!("{role:?}").to_ascii_lowercase()),
        BotFilterExpr::Account { account_id } => event.bot.account_id == *account_id,
        BotFilterExpr::ActorAllowlist { actor_ids } => event
            .actor
            .as_ref()
            .is_some_and(|actor| actor_ids.contains(&actor.user_id)),
        BotFilterExpr::ActorDenylist { actor_ids } => event
            .actor
            .as_ref()
            .is_none_or(|actor| !actor_ids.contains(&actor.user_id)),
        BotFilterExpr::MessageSegmentType { segment_type } => {
            event.message.as_ref().is_some_and(|message| {
                message
                    .segments
                    .iter()
                    .any(|segment| segment_name(segment) == segment_type)
            })
        }
        // Custom predicates are evaluated asynchronously by `filter_expression_allowed`.
        BotFilterExpr::CustomPredicate { .. } => false,
    }
}

fn filter_allowed<'a>(
    ctx: &'a AsyncRunnerContext,
    handler: &'a BotHandlerDescriptor,
    event: &'a BotEvent,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = RuntimeResult<bool>> + Send + 'a>> {
    match handler.filter.as_ref() {
        Some(filter) => filter_expression_allowed(ctx, handler, event, filter),
        None => Box::pin(async { Ok(true) }),
    }
}

fn filter_expression_allowed<'a>(
    ctx: &'a AsyncRunnerContext,
    handler: &'a BotHandlerDescriptor,
    event: &'a BotEvent,
    filter: &'a BotFilterExpr,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = RuntimeResult<bool>> + Send + 'a>> {
    Box::pin(async move {
        match filter {
            BotFilterExpr::CustomPredicate { service_id } => {
                let outcome = ctx
                    .call_raw(
                        service_id,
                        serde_json::to_value(BotCustomPredicateRequest {
                            event: event.clone(),
                            handler_id: handler.handler_id.clone(),
                        })
                        .map_err(|error| {
                            failure_raw(&handler.handler_id, "predicate.encode", error)
                        })?,
                    )
                    .await?;
                Ok(decode_required::<BotCustomPredicateResult>(
                    &outcome,
                    &Task::new(
                        &handler.handler_id,
                        handler.handler_protocol_id.clone(),
                        serde_json::Value::Null,
                    ),
                )?
                .matched)
            }
            BotFilterExpr::All { filters } => {
                for filter in filters {
                    if !filter_expression_allowed(ctx, handler, event, filter).await? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            BotFilterExpr::Any { filters } => {
                for filter in filters {
                    if filter_expression_allowed(ctx, handler, event, filter).await? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            BotFilterExpr::Not { filter } => {
                Ok(!filter_expression_allowed(ctx, handler, event, filter).await?)
            }
            other => Ok(filter_matches(other, event)),
        }
    })
}

pub fn conversation_kind(target: &BotTarget) -> BotConversationKind {
    match target {
        BotTarget::User { .. } => BotConversationKind::Private,
        BotTarget::Group { .. } => BotConversationKind::Group,
        BotTarget::GuildChannel { .. } => BotConversationKind::Channel,
        BotTarget::Conversation { .. } | BotTarget::PlatformSpecific { .. } => {
            BotConversationKind::Private
        }
    }
}

fn segment_name(segment: &MessageSegment) -> &'static str {
    match segment {
        MessageSegment::Text { .. } => "text",
        MessageSegment::MentionUser { .. } => "mention_user",
        MessageSegment::MentionAll => "mention_all",
        MessageSegment::Image { .. } => "image",
        MessageSegment::File { .. } => "file",
        MessageSegment::Audio { .. } => "audio",
        MessageSegment::Video { .. } => "video",
        MessageSegment::Reply { .. } => "reply",
        MessageSegment::Quote { .. } => "quote",
        MessageSegment::PlatformSpecific { .. } => "platform_specific",
    }
}

fn failure(task: &Task, route: &str, error: impl std::fmt::Display) -> RuntimeFailure {
    failure_raw(&task.task_id, route, error)
}

fn failure_raw(id: &str, route: &str, error: impl std::fmt::Display) -> RuntimeFailure {
    RuntimeFailure::new(mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        BOT_HANDLER_PIPELINE_PLUGIN_ID,
        format!("{id}.{route}:{error}"),
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use mutsuki_bot_protocol::{
        BotAccountRef, BotEventKind, BotPlatform, BotPropagationPolicy, BotUser,
    };
    use mutsuki_runtime_contracts::{
        BatchEntry, BatchPayload, DispatchLane, OrderingRequirement, TaskBatch, TaskHandle,
        WorkBatch, WorkResourcePlan,
    };
    use mutsuki_runtime_core::RunnerContext;
    use mutsuki_runtime_sdk::RuntimeClient;

    use super::*;

    #[derive(Default)]
    struct OutcomeClient {
        outcomes: Mutex<BTreeMap<String, TaskOutcome>>,
        errors: Mutex<BTreeMap<String, mutsuki_runtime_contracts::RuntimeError>>,
    }

    impl OutcomeClient {
        fn complete(&self, task_id: String, outcome: BotHandlerOutcome) {
            self.outcomes.lock().unwrap().insert(
                task_id.clone(),
                TaskOutcome::Completed {
                    task_id,
                    output: Some(
                        serde_json::to_value(BotHandlerExecutionResult { outcome }).unwrap(),
                    ),
                    output_ref: None,
                },
            );
        }

        fn fail(&self, task_id: String, code: &str) {
            self.outcomes.lock().unwrap().insert(
                task_id.clone(),
                TaskOutcome::Failed {
                    task_id,
                    error: mutsuki_runtime_contracts::RuntimeError::new(code, "test", "handler"),
                },
            );
        }

        fn fail_runtime(&self, task_id: String, code: &str) {
            self.errors.lock().unwrap().insert(
                task_id,
                mutsuki_runtime_contracts::RuntimeError::new(code, "test", "handler"),
            );
        }
    }

    impl RuntimeClient for OutcomeClient {
        fn submit_batch(&self, _batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
            Ok(Vec::new())
        }

        fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
            if let Some(error) = self.errors.lock().unwrap().get(&handle.task_id).cloned() {
                return Err(RuntimeFailure::new(error));
            }
            Ok(self.outcomes.lock().unwrap().get(&handle.task_id).cloned())
        }
    }

    #[test]
    fn higher_priority_stop_prevents_lower_priority_dispatch() {
        let client = Arc::new(OutcomeClient::default());
        let mut runner = handler_pipeline_runner(
            client.clone(),
            vec![
                handler("low", 1, BotPropagationPolicy::Continue),
                handler("high", 10, BotPropagationPolicy::StopOnSuccess),
            ],
        );
        let task = event_task();
        let ctx = context();
        let first = single_result(runner.run_batch(ctx.clone(), batch(&task)).unwrap());
        assert_eq!(first.tasks.len(), 1);
        assert_eq!(
            first.tasks[0].target_binding_id.as_deref(),
            Some("high-binding")
        );

        client.complete(first.tasks[0].task_id.clone(), BotHandlerOutcome::Continue);
        let completed = single_result(runner.run_batch(ctx, batch(&task)).unwrap());
        assert_eq!(
            completed.output,
            Some(serde_json::json!({
                "invoked_handlers": 1,
                "handler_errors": [],
                "handler_denials": [],
            }))
        );
        assert!(completed.tasks.is_empty());
    }

    #[test]
    fn composed_filters_match_conversation_mention_and_allowlist() {
        let mut descriptor = handler("filtered", 0, BotPropagationPolicy::Continue);
        descriptor.filter = Some(BotFilterExpr::All {
            filters: vec![
                BotFilterExpr::ConversationKind {
                    kind: BotConversationKind::Group,
                },
                BotFilterExpr::MustMentionBot,
                BotFilterExpr::ActorAllowlist {
                    actor_ids: vec!["actor".into()],
                },
            ],
        });
        let event: BotEvent = event_task().payload.decode().unwrap();
        assert!(handler_matches(&descriptor, &event));
    }

    #[test]
    fn failed_handler_is_recorded_and_does_not_block_unrelated_handler() {
        let client = Arc::new(OutcomeClient::default());
        let mut runner = handler_pipeline_runner(
            client.clone(),
            vec![
                handler("failed", 10, BotPropagationPolicy::Continue),
                handler("healthy", 1, BotPropagationPolicy::Continue),
            ],
        );
        let task = event_task();
        let ctx = context();
        let failed = single_result(runner.run_batch(ctx.clone(), batch(&task)).unwrap());
        client.fail(failed.tasks[0].task_id.clone(), "handler.crashed");

        let healthy = single_result(runner.run_batch(ctx.clone(), batch(&task)).unwrap());
        assert_eq!(
            healthy.tasks[0].target_binding_id.as_deref(),
            Some("healthy-binding")
        );
        client.complete(
            healthy.tasks[0].task_id.clone(),
            BotHandlerOutcome::Continue,
        );

        let completed = single_result(runner.run_batch(ctx, batch(&task)).unwrap());
        assert_eq!(completed.output.as_ref().unwrap()["invoked_handlers"], 2);
        assert_eq!(
            completed.output.as_ref().unwrap()["handler_errors"][0]["code"],
            "handler.crashed"
        );
    }

    #[test]
    fn runtime_failure_is_recorded_and_does_not_block_unrelated_handler() {
        let client = Arc::new(OutcomeClient::default());
        let mut runner = handler_pipeline_runner(
            client.clone(),
            vec![
                handler("failed", 10, BotPropagationPolicy::Continue),
                handler("healthy", 1, BotPropagationPolicy::Continue),
            ],
        );
        let task = event_task();
        let ctx = context();
        let failed = single_result(runner.run_batch(ctx.clone(), batch(&task)).unwrap());
        client.fail_runtime(failed.tasks[0].task_id.clone(), "handler.timeout");

        let healthy = single_result(runner.run_batch(ctx.clone(), batch(&task)).unwrap());
        assert_eq!(
            healthy.tasks[0].target_binding_id.as_deref(),
            Some("healthy-binding")
        );
        client.complete(
            healthy.tasks[0].task_id.clone(),
            BotHandlerOutcome::Continue,
        );

        let completed = single_result(runner.run_batch(ctx, batch(&task)).unwrap());
        assert_eq!(completed.output.as_ref().unwrap()["invoked_handlers"], 1);
        assert_eq!(
            completed.output.as_ref().unwrap()["handler_errors"][0]["code"],
            "handler.timeout"
        );
    }

    #[test]
    fn custom_predicate_inside_composite_filter_is_not_prefiltered() {
        let mut descriptor = handler("custom", 0, BotPropagationPolicy::Continue);
        descriptor.filter = Some(BotFilterExpr::Not {
            filter: Box::new(BotFilterExpr::CustomPredicate {
                service_id: "test.predicate".into(),
            }),
        });
        let event: BotEvent = event_task().payload.decode().unwrap();
        assert!(handler_matches(&descriptor, &event));
    }

    #[test]
    fn completed_handler_event_is_suppressed_across_duplicate_ingest_tasks() {
        let client = Arc::new(OutcomeClient::default());
        let mut runner = handler_pipeline_runner(
            client.clone(),
            vec![handler("once", 1, BotPropagationPolicy::Continue)],
        );
        let task = event_task();
        let ctx = context();
        let dispatched = single_result(runner.run_batch(ctx.clone(), batch(&task)).unwrap());
        client.complete(
            dispatched.tasks[0].task_id.clone(),
            BotHandlerOutcome::Continue,
        );
        let completed = single_result(runner.run_batch(ctx.clone(), batch(&task)).unwrap());
        assert_eq!(completed.output.as_ref().unwrap()["invoked_handlers"], 1);

        let mut duplicate = event_task();
        duplicate.task_id = "pipeline-event-duplicate".into();
        let suppressed = single_result(runner.run_batch(ctx, batch(&duplicate)).unwrap());

        assert!(suppressed.tasks.is_empty());
        assert_eq!(suppressed.output.as_ref().unwrap()["invoked_handlers"], 0);
        assert_eq!(
            suppressed.output.as_ref().unwrap()["handler_denials"][0]["code"],
            "bot.handler.duplicate"
        );
    }

    fn handler(id: &str, priority: i32, propagation: BotPropagationPolicy) -> BotHandlerDescriptor {
        BotHandlerDescriptor {
            handler_id: id.into(),
            binding_id: format!("{id}-binding"),
            generation: 1,
            handler_protocol_id: "mutsuki.bot.test/handle@1".into(),
            runner_hint: None,
            event_kinds: vec![BotEventKind::MessageCreated],
            conversation_kinds: vec![BotConversationKind::Group],
            filter: None,
            permissions: Vec::new(),
            priority,
            propagation,
            rate_limit: None,
            timeout_ms: None,
            side_effects: Vec::new(),
            max_concurrency: None,
            before_hook_protocol_ids: Vec::new(),
            after_hook_protocol_ids: Vec::new(),
            error_hook_protocol_ids: Vec::new(),
        }
    }

    fn event_task() -> Task {
        let mut ext = BTreeMap::new();
        ext.insert("qqbot.mentioned_bot".into(), Value::Bool(true));
        Task::new(
            "pipeline-event",
            BOT_EVENT_INGEST_PROTOCOL_ID,
            mutsuki_runtime_contracts::TaskPayload::from_local(BotEvent {
                event_id: "event".into(),
                platform: BotPlatform::QqBot,
                bot: BotAccountRef {
                    account_id: "bot".into(),
                    platform: BotPlatform::QqBot,
                },
                kind: BotEventKind::MessageCreated,
                time_ms: 1,
                target: BotTarget::Group {
                    group_id: "group".into(),
                },
                actor: Some(BotUser {
                    user_id: "actor".into(),
                    display_name: None,
                    avatar_url: None,
                }),
                message: None,
                raw: None,
                ext,
            }),
        )
    }

    fn context() -> RunnerContext {
        RunnerContext::new(
            1,
            1,
            "pipeline-test",
            Vec::<String>::new(),
            "pipeline-invocation",
        )
        .with_batch("pipeline-batch", 1)
    }

    fn batch(task: &Task) -> WorkBatch {
        WorkBatch {
            batch_id: "pipeline-batch".into(),
            tick_id: "pipeline-tick".into(),
            batch_key: BOT_HANDLER_PIPELINE_RUNNER_ID.into(),
            entries: vec![BatchEntry {
                entry_id: "entry".into(),
                task_id: task.task_id.clone(),
                trace_id: None,
                parent_id: None,
                payload_index: 0,
                resource_requirement_indices: Vec::new(),
                cancel_index: None,
                deadline_tick: None,
                priority: 0,
                lane: DispatchLane::Normal,
                ordering: OrderingRequirement::None,
            }],
            payload: BatchPayload::from_tasks(std::slice::from_ref(task)),
            resource_plan: WorkResourcePlan::empty(),
            task_leases: Vec::new(),
        }
    }

    fn single_result(completion: mutsuki_runtime_contracts::CompletionBatch) -> RunnerResult {
        completion
            .results
            .into_iter()
            .next()
            .unwrap()
            .result
            .unwrap()
    }
}
