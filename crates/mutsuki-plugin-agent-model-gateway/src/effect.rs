use std::collections::BTreeMap;

use mutsuki_agent_protocol::*;
use mutsuki_agent_sdk::{
    AgentModelHttpEffectProtocol, AgentModelPollProtocol, effectful_runner, orchestration_runner,
    result_event, runtime_failure,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::contracts::{
    CancelPolicy, CompletionBatch, ResourceAccess, ResourceId, ResourceLifetime, ResourceRef,
    ResourceSealState, ResourceSemantic, RunnerBatchCapability, RunnerDescriptor, RunnerMode,
    RunnerResult, RunnerSideEffect, RunnerStatus, Task, TaskAwait, TaskHandle,
    TaskStepContinuation, WakeCondition, WorkBatch,
};
use mutsuki_runtime_sdk::map_work_batch_entries;

use crate::ModelGateway;
use crate::plugin::{PLUGIN_ID, append_callback};

pub const EFFECT_RUNNER_ID: &str = "effect.mutsuki.agent.model.http";
pub const POLL_RUNNER_ID: &str = "mutsuki.agent.model.poll";

pub struct HttpEffectRunner {
    descriptor: RunnerDescriptor,
    gateway: ModelGateway,
    handle: Option<tokio::runtime::Handle>,
    pending: BTreeMap<String, PendingHttp>,
    invocations: BTreeMap<String, Vec<String>>,
}

struct PendingHttp {
    request: AgentModelHttpEffectRequest,
    join: tokio::task::JoinHandle<AgentResult<EffectValue>>,
    polls: u64,
}

enum EffectValue {
    Generated(AgentModelGenerateResult),
    Streamed(AgentModelStreamResult),
}

impl HttpEffectRunner {
    pub fn blocking(gateway: ModelGateway) -> Self {
        Self::new(gateway, None)
    }

    pub fn cancellable(gateway: ModelGateway, handle: tokio::runtime::Handle) -> Self {
        Self::new(gateway, Some(handle))
    }

    fn new(gateway: ModelGateway, handle: Option<tokio::runtime::Handle>) -> Self {
        Self {
            descriptor: effectful_runner(EFFECT_RUNNER_ID, PLUGIN_ID)
                .accepts::<AgentModelHttpEffectProtocol>()
                .batch_capability(RunnerBatchCapability {
                    mode: RunnerMode::NativeBatch,
                    max_batch_entries: 16,
                    max_inflight_batches: 1,
                    side_effect: RunnerSideEffect::External,
                    ..Default::default()
                })
                .build(),
            gateway,
            handle,
            pending: BTreeMap::new(),
            invocations: BTreeMap::new(),
        }
    }

    fn run_one(
        &mut self,
        ctx: &RunnerContext,
        task: &Task,
    ) -> Result<RunnerResult, mutsuki_runtime_sdk::contracts::RuntimeError> {
        let request: AgentModelHttpEffectRequest = serde_json::from_value(task.payload.clone())
            .map_err(|error| agent_error(task, AgentError::invalid_input(error.to_string())))?;
        if ctx.cancel_requested {
            self.abort_task(&task.task_id);
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.status = RunnerStatus::Cancelled;
            return Ok(result);
        }
        let Some(handle) = self.handle.clone() else {
            return finish_effect(task, request.clone(), run_blocking(&self.gateway, request)?);
        };

        self.invocations
            .entry(ctx.invocation_id.clone())
            .or_default()
            .push(task.task_id.clone());
        if !self.pending.contains_key(&task.task_id) {
            let gateway = self.gateway.clone();
            let async_request = request.clone();
            let join = handle.spawn(async move { run_async(gateway, async_request).await });
            self.pending.insert(
                task.task_id.clone(),
                PendingHttp {
                    request,
                    join,
                    polls: 0,
                },
            );
        }
        if !self.pending[&task.task_id].join.is_finished() {
            let pending = self
                .pending
                .get_mut(&task.task_id)
                .expect("pending request");
            pending.polls = pending.polls.saturating_add(1);
            return Ok(waiting_result(task, ctx, pending.polls));
        }

        let pending = self
            .pending
            .remove(&task.task_id)
            .expect("finished request");
        self.remove_task_mapping(&task.task_id);
        let value = handle
            .block_on(pending.join)
            .map_err(|error| {
                agent_error(
                    task,
                    AgentError::provider_unavailable(format!("model task join failed: {error}")),
                )
            })?
            .map_err(|error| agent_error(task, error))?;
        finish_effect(task, pending.request, value)
    }

    fn abort_task(&mut self, task_id: &str) {
        if let Some(pending) = self.pending.remove(task_id) {
            pending.join.abort();
        }
        self.remove_task_mapping(task_id);
    }

    fn remove_task_mapping(&mut self, task_id: &str) {
        self.invocations.retain(|_, tasks| {
            tasks.retain(|known| known != task_id);
            !tasks.is_empty()
        });
    }
}

impl Runner for HttpEffectRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| self.run_one(&ctx, task))
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        if let Some(tasks) = self.invocations.remove(invocation_id) {
            for task_id in tasks {
                self.abort_task(&task_id);
            }
        }
        Ok(())
    }
}

impl Drop for HttpEffectRunner {
    fn drop(&mut self) {
        for (_, pending) in std::mem::take(&mut self.pending) {
            pending.join.abort();
        }
    }
}

pub struct ModelPollRunner {
    descriptor: RunnerDescriptor,
}

impl Default for ModelPollRunner {
    fn default() -> Self {
        Self {
            descriptor: orchestration_runner(POLL_RUNNER_ID, PLUGIN_ID)
                .accepts::<AgentModelPollProtocol>()
                .batch_capability(RunnerBatchCapability {
                    mode: RunnerMode::NativeBatch,
                    max_batch_entries: 64,
                    ..Default::default()
                })
                .build(),
        }
    }
}

impl Runner for ModelPollRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| Ok(RunnerResult::completed(&task.task_id)))
    }
}

async fn run_async(
    gateway: ModelGateway,
    request: AgentModelHttpEffectRequest,
) -> AgentResult<EffectValue> {
    match request {
        AgentModelHttpEffectRequest::Generate(request) => gateway
            .generate_effect_async(request)
            .await
            .map(EffectValue::Generated),
        AgentModelHttpEffectRequest::Stream(request) => gateway
            .stream_effect_async(request)
            .await
            .map(EffectValue::Streamed),
    }
}

fn run_blocking(
    gateway: &ModelGateway,
    request: AgentModelHttpEffectRequest,
) -> Result<EffectValue, mutsuki_runtime_sdk::contracts::RuntimeError> {
    match request {
        AgentModelHttpEffectRequest::Generate(request) => gateway
            .generate_effect(request)
            .map(EffectValue::Generated)
            .map_err(|error| standalone_error(error)),
        AgentModelHttpEffectRequest::Stream(request) => gateway
            .stream_effect(request)
            .map(EffectValue::Streamed)
            .map_err(|error| standalone_error(error)),
    }
}

fn finish_effect(
    task: &Task,
    request: AgentModelHttpEffectRequest,
    value: EffectValue,
) -> Result<RunnerResult, mutsuki_runtime_sdk::contracts::RuntimeError> {
    match (request, value) {
        (AgentModelHttpEffectRequest::Generate(request), EffectValue::Generated(generated)) => {
            let mut result = result_event(
                task.task_id.clone(),
                "mutsuki.agent.model.generated",
                generated.clone(),
            )
            .map_err(|failure| failure.error().clone())?;
            append_callback(
                task,
                &mut result,
                request.result_protocol_id,
                request.result_context,
                request.session_id,
                generated,
            )
            .map_err(|failure| failure.error().clone())?;
            Ok(result)
        }
        (AgentModelHttpEffectRequest::Stream(_), EffectValue::Streamed(streamed)) => result_event(
            task.task_id.clone(),
            "mutsuki.agent.model.stream_opened",
            streamed,
        )
        .map_err(|failure| failure.error().clone()),
        _ => Err(standalone_error(AgentError::new(
            "agent.model.effect_mismatch",
            "model effect result does not match its request",
        ))),
    }
}

fn waiting_result(task: &Task, ctx: &RunnerContext, poll: u64) -> RunnerResult {
    let ready_at_step = ctx.current_step.saturating_add(1);
    let poll_task_id = format!("{}:poll:{poll}", task.task_id);
    let mut poll_task = Task::new(
        poll_task_id.clone(),
        AGENT_MODEL_POLL_PROTOCOL,
        serde_json::json!({"parent_task_id": task.task_id}),
    );
    poll_task.ready_at_step = Some(ready_at_step);
    poll_task.registry_generation = ctx.registry_generation;
    poll_task.trace_id = task.trace_id.clone();
    poll_task.correlation_id = task.correlation_id.clone();
    let child = TaskHandle {
        task_id: poll_task_id,
        protocol_id: AGENT_MODEL_POLL_PROTOCOL.into(),
        target_binding_id: None,
        cancel_policy: CancelPolicy::Cascade,
        trace_id: task.trace_id.clone(),
        correlation_id: task.correlation_id.clone(),
    };
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.tasks.push(poll_task);
    result.task_await = Some(TaskAwait {
        parent_task_id: task.task_id.clone(),
        child,
        continuation: TaskStepContinuation {
            continuation: continuation_ref(&task.task_id),
            wake: Some(WakeCondition::Timer { ready_at_step }),
            reason: Some("agent.model.http.poll".into()),
        },
        cancel_policy: CancelPolicy::Cascade,
    });
    result.status = RunnerStatus::Waiting;
    result
}

fn continuation_ref(task_id: &str) -> ResourceRef {
    let ref_id = format!("agent-model-continuation:{task_id}");
    ResourceRef {
        ref_id: ref_id.clone(),
        resource_id: ResourceId {
            kind_id: "mutsuki.agent.model.continuation".into(),
            slot_id: task_id.into(),
            generation: 1,
            version: 1,
        },
        semantic: ResourceSemantic::CowVersionedState,
        provider_id: PLUGIN_ID.into(),
        resource_kind: "mutsuki.agent.model.continuation".into(),
        schema: "mutsuki.agent.model.continuation.v1".into(),
        version: 1,
        generation: 1,
        access: ResourceAccess::ProviderRpc {
            provider_id: PLUGIN_ID.into(),
            method: "poll".into(),
        },
        size_hint: None,
        content_hash: None,
        lifetime: ResourceLifetime::BorrowedUntilTaskEnd,
        lease: None,
        seal_state: ResourceSealState::Writable,
    }
}

fn agent_error(task: &Task, error: AgentError) -> mutsuki_runtime_sdk::contracts::RuntimeError {
    runtime_failure(PLUGIN_ID, &task.task_id, error)
        .error()
        .clone()
}

fn standalone_error(error: AgentError) -> mutsuki_runtime_sdk::contracts::RuntimeError {
    runtime_failure(PLUGIN_ID, "model.effect", error)
        .error()
        .clone()
}
