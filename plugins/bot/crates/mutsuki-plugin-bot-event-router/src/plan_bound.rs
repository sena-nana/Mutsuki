//! Task-await runners whose callable protocols are `LoadPlan` facts.
//!
//! A task-await adapter may only call the protocols its descriptor declares with
//! `requires_protocol`. For the Flow node runner those are the bindings of the active node catalog,
//! and for the outbound router the endpoints of the active adapters; both are published by the
//! Flow `LoadPlan` observer. `ServiceHost` builds runners before it activates the plan, so an adapter
//! built at construction would declare the previous plan (an empty one on first boot) and every
//! call would fail as undeclared. The adapter is therefore built on first use and rebuilt, once
//! no invocation is in flight, whenever the registry has activated a newer plan.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mutsuki_bot_flow::BotFlowRegistry;
use mutsuki_runtime_contracts::{CompletionBatch, RunnerDescriptor, RunnerResult, Task, WorkBatch};
use mutsuki_runtime_sdk::{
    AsyncRunnerContext, BoxedTaskAwaitRunner, Runner, RunnerContext, RuntimeClientRef,
    RuntimeResult, TaskAwaitRunnerAdapter,
};

pub(crate) type PlanBoundCall = Arc<
    dyn Fn(
            AsyncRunnerContext,
            Task,
        ) -> Pin<Box<dyn Future<Output = RuntimeResult<RunnerResult>> + Send>>
        + Send
        + Sync,
>;

pub(crate) struct PlanBoundRunner {
    /// What the Host sees; equal to the manifest declaration.
    descriptor: RunnerDescriptor,
    client: RuntimeClientRef,
    registry: Arc<BotFlowRegistry>,
    /// The descriptor, including `requires`, for the plan the registry currently publishes.
    descriptor_for_plan: fn(&BotFlowRegistry) -> RunnerDescriptor,
    call: PlanBoundCall,
    inner: Option<(u64, TaskAwaitRunnerAdapter)>,
    inflight: Arc<AtomicUsize>,
}

/// Counts one invocation for as long as its future lives.
struct InflightGuard(Arc<AtomicUsize>);

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl PlanBoundRunner {
    pub(crate) fn new(
        descriptor: RunnerDescriptor,
        client: RuntimeClientRef,
        registry: Arc<BotFlowRegistry>,
        descriptor_for_plan: fn(&BotFlowRegistry) -> RunnerDescriptor,
        call: PlanBoundCall,
    ) -> Self {
        Self {
            descriptor,
            client,
            registry,
            descriptor_for_plan,
            call,
            inner: None,
            inflight: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn adapter(&mut self) -> &mut TaskAwaitRunnerAdapter {
        let generation = self.registry.plan_generation();
        let rebuild = match &self.inner {
            None => true,
            Some((built_for, _)) => {
                *built_for != generation && self.inflight.load(Ordering::SeqCst) == 0
            }
        };
        if rebuild {
            let call = self.call.clone();
            let inflight = self.inflight.clone();
            let factory: BoxedTaskAwaitRunner = Box::new(move |ctx, task| {
                inflight.fetch_add(1, Ordering::SeqCst);
                let guard = InflightGuard(inflight.clone());
                let invocation = call(ctx, task);
                Box::pin(async move {
                    let _guard = guard;
                    invocation.await
                })
            });
            let adapter = TaskAwaitRunnerAdapter::new(
                (self.descriptor_for_plan)(&self.registry),
                self.client.clone(),
                factory,
            )
            .with_self_call_policy(false);
            self.inner = Some((generation, adapter));
        }
        &mut self.inner.as_mut().expect("plan-bound adapter is built").1
    }
}

impl Runner for PlanBoundRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        self.adapter().run_batch(ctx, batch)
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        match &mut self.inner {
            Some((_, adapter)) => adapter.cancel(invocation_id),
            None => Ok(()),
        }
    }
}
