use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

extern crate self as mutsuki_runtime_sdk;

use mutsuki_runtime_contracts::{
    CancelPolicy, ResourceAccess, ResourceId, ResourceLifetime, ResourceRef, ResourceSealState,
    ResourceSemantic, RunnerDescriptor, RunnerResult, RunnerStatus, Task, TaskAwait, TaskHandle,
    TaskOutcome, TaskStepContinuation,
};
use mutsuki_runtime_core::{CoreRuntime, Runner, RunnerContext};
use serde::Serialize;
use serde_json::Value;

mod backend;
mod descriptor;
mod host;
mod plugin;
mod resource;

pub use backend::{ResourcePlanGateway, ResourceProviderGateway};
pub use descriptor::{
    HandlerBindingBuilder, ProtocolDescriptorBuilder, ProtocolSpec, ResourceKindSpec,
    ResourceTypeDescriptorBuilder, RunnerDescriptorBuilder,
};
pub use host::{
    CapabilityBroker, ConfigProvider, EventBridge, HostContext, HostRuntime, HostService,
    HostServiceRegistry, HostTaskFailureSummary, HostTaskSnapshot, ManualShutdownController,
    NoopEventBridge, RecordingEventBridge, ShutdownController, StaticCapabilityBroker,
    StaticConfigProvider, TaskSubmitter, TaskSubmitterRuntimeClient,
};
pub use mutsuki_runtime_core::{ReloadDecision, RuntimeFailure, RuntimeResult};
pub use mutsuki_runtime_sdk_macros::{ResourceKind, SdkProtocol, mutsuki_runner};
pub use plugin::{
    BuiltinPluginLoader, LoadedPlugin, Plugin, PluginBuilder, PluginLoader,
    RuntimeBootstrapperResourceProvider, RuntimeBootstrapperService,
};
pub use resource::{ResourceClient, ResourceKind, TypedResourceHandle};

pub mod contracts {
    pub use mutsuki_runtime_contracts::*;
}

pub trait SdkProtocol {
    const PROTOCOL_ID: &'static str;
}

pub trait RuntimeClient: Send + Sync {
    fn submit_task(&self, task: Task) -> RuntimeResult<TaskHandle>;
    fn task_outcome(&self, task_id: &str) -> RuntimeResult<Option<TaskOutcome>>;
    fn register_waker(&self, _task_id: &str, _waker: &Waker) {}
}

pub type RuntimeClientRef = Arc<dyn RuntimeClient>;

impl RuntimeClient for Arc<Mutex<CoreRuntime>> {
    fn submit_task(&self, task: Task) -> RuntimeResult<TaskHandle> {
        self.lock()
            .expect("runtime mutex poisoned")
            .submit_task_handle(task)
    }

    fn task_outcome(&self, task_id: &str) -> RuntimeResult<Option<TaskOutcome>> {
        self.lock()
            .expect("runtime mutex poisoned")
            .task_outcome(task_id)
    }
}

pub struct TaskHandleFuture {
    client: RuntimeClientRef,
    handle: TaskHandle,
}

impl TaskHandleFuture {
    pub fn new(client: RuntimeClientRef, handle: TaskHandle) -> Self {
        Self { client, handle }
    }

    pub fn handle(&self) -> &TaskHandle {
        &self.handle
    }
}

impl Future for TaskHandleFuture {
    type Output = RuntimeResult<TaskOutcome>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.client.task_outcome(&self.handle.task_id) {
            Ok(Some(outcome)) => Poll::Ready(Ok(outcome)),
            Ok(None) => {
                self.client.register_waker(&self.handle.task_id, cx.waker());
                Poll::Pending
            }
            Err(error) => Poll::Ready(Err(error)),
        }
    }
}

#[derive(Clone)]
pub struct AsyncRunnerContext {
    client: RuntimeClientRef,
    parent_task_id: String,
    current_runner_id: String,
    trace_id: Option<String>,
    correlation_id: Option<String>,
    next_call: Arc<AtomicU64>,
    pending: Arc<Mutex<Option<PendingAwait>>>,
    allow_self_call: bool,
}

impl AsyncRunnerContext {
    pub fn task_id(&self) -> &str {
        &self.parent_task_id
    }

    pub fn call<P>(&self, input: impl Serialize) -> CallFuture
    where
        P: SdkProtocol,
    {
        self.call_with_cancel_policy::<P>(input, CancelPolicy::Cascade)
    }

    pub fn call_with_cancel_policy<P>(
        &self,
        input: impl Serialize,
        cancel_policy: CancelPolicy,
    ) -> CallFuture
    where
        P: SdkProtocol,
    {
        match serde_json::to_value(input) {
            Ok(payload) => self.call_raw_with_cancel_policy(P::PROTOCOL_ID, payload, cancel_policy),
            Err(error) => CallFuture::failed(
                self.client.clone(),
                self.parent_task_id.clone(),
                self.pending.clone(),
                serialize_error(error),
            ),
        }
    }

    pub fn call_raw(&self, protocol_id: impl Into<String>, payload: Value) -> CallFuture {
        self.call_raw_with_cancel_policy(protocol_id, payload, CancelPolicy::Cascade)
    }

    pub fn call_raw_with_cancel_policy(
        &self,
        protocol_id: impl Into<String>,
        payload: Value,
        cancel_policy: CancelPolicy,
    ) -> CallFuture {
        self.call_with_runner_hint(protocol_id, payload, None, cancel_policy)
    }

    pub fn call_targeted<P>(
        &self,
        binding_id: impl Into<String>,
        runner_hint: impl Into<String>,
        input: impl Serialize,
    ) -> CallFuture
    where
        P: SdkProtocol,
    {
        self.call_targeted_with_cancel_policy::<P>(
            binding_id,
            runner_hint,
            input,
            CancelPolicy::Cascade,
        )
    }

    pub fn call_targeted_with_cancel_policy<P>(
        &self,
        binding_id: impl Into<String>,
        runner_hint: impl Into<String>,
        input: impl Serialize,
        cancel_policy: CancelPolicy,
    ) -> CallFuture
    where
        P: SdkProtocol,
    {
        match serde_json::to_value(input) {
            Ok(payload) => self.call_targeted_raw_with_cancel_policy(
                binding_id,
                P::PROTOCOL_ID,
                runner_hint,
                payload,
                cancel_policy,
            ),
            Err(error) => CallFuture::failed(
                self.client.clone(),
                self.parent_task_id.clone(),
                self.pending.clone(),
                serialize_error(error),
            ),
        }
    }

    pub fn call_targeted_raw(
        &self,
        binding_id: impl Into<String>,
        protocol_id: impl Into<String>,
        runner_hint: impl Into<String>,
        payload: Value,
    ) -> CallFuture {
        self.call_targeted_raw_with_cancel_policy(
            binding_id,
            protocol_id,
            runner_hint,
            payload,
            CancelPolicy::Cascade,
        )
    }

    pub fn call_targeted_raw_with_cancel_policy(
        &self,
        binding_id: impl Into<String>,
        protocol_id: impl Into<String>,
        runner_hint: impl Into<String>,
        payload: Value,
        cancel_policy: CancelPolicy,
    ) -> CallFuture {
        self.call_with_runner_hint(
            protocol_id,
            payload,
            Some((binding_id.into(), runner_hint.into())),
            cancel_policy,
        )
    }

    fn call_with_runner_hint(
        &self,
        protocol_id: impl Into<String>,
        payload: Value,
        target: Option<(String, String)>,
        cancel_policy: CancelPolicy,
    ) -> CallFuture {
        let call_index = self.next_call.fetch_add(1, Ordering::Relaxed) + 1;
        let protocol_id = protocol_id.into();
        let task_id = format!("{}:call:{call_index}", self.parent_task_id);
        let mut task = Task::new(task_id.clone(), protocol_id.clone(), payload);
        task.trace_id = self.trace_id.clone();
        task.correlation_id = self.correlation_id.clone();
        let (target_binding_id, runner_hint) = match target {
            Some((binding_id, runner_hint)) => (Some(binding_id), Some(runner_hint)),
            None => (None, None),
        };
        task.target_binding_id = target_binding_id.clone();
        task.runner_hint = runner_hint.clone();
        let self_call_blocked = runner_hint.as_deref() == Some(self.current_runner_id.as_str())
            && !self.allow_self_call;
        let handle = TaskHandle {
            task_id,
            protocol_id,
            target_binding_id,
            cancel_policy: cancel_policy.clone(),
            trace_id: self.trace_id.clone(),
            correlation_id: self.correlation_id.clone(),
        };
        CallFuture {
            client: self.client.clone(),
            parent_task_id: self.parent_task_id.clone(),
            pending: self.pending.clone(),
            state: CallState::Init {
                task: Box::new(task),
                handle,
            },
            self_call_blocked,
        }
    }
}

pub struct CallFuture {
    client: RuntimeClientRef,
    parent_task_id: String,
    pending: Arc<Mutex<Option<PendingAwait>>>,
    state: CallState,
    self_call_blocked: bool,
}

enum CallState {
    Init { task: Box<Task>, handle: TaskHandle },
    Submitted { handle: TaskHandle },
    Failed(Option<RuntimeFailure>),
    Done,
}

impl CallFuture {
    fn failed(
        client: RuntimeClientRef,
        parent_task_id: String,
        pending: Arc<Mutex<Option<PendingAwait>>>,
        error: RuntimeFailure,
    ) -> Self {
        Self {
            client,
            parent_task_id,
            pending,
            state: CallState::Failed(Some(error)),
            self_call_blocked: false,
        }
    }
}

impl Future for CallFuture {
    type Output = RuntimeResult<TaskOutcome>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.self_call_blocked {
            return Poll::Ready(Err(mutsuki_runtime_core::RuntimeFailure::new(
                mutsuki_runtime_contracts::RuntimeError::new(
                    "task.self_call_blocked",
                    "runtime.sdk",
                    format!("task.await.{}", self.parent_task_id),
                ),
            )));
        }

        let state = std::mem::replace(&mut self.state, CallState::Done);
        match state {
            CallState::Init { task, handle } => {
                let pending = PendingAwait::new(
                    self.parent_task_id.clone(),
                    handle.clone(),
                    Some(*task),
                    handle.cancel_policy.clone(),
                );
                *self.pending.lock().expect("pending await mutex poisoned") = Some(pending);
                self.state = CallState::Submitted { handle };
                Poll::Pending
            }
            CallState::Submitted { handle } => match self.client.task_outcome(&handle.task_id) {
                Ok(Some(outcome)) => {
                    self.state = CallState::Done;
                    Poll::Ready(Ok(outcome))
                }
                Ok(None) => {
                    *self.pending.lock().expect("pending await mutex poisoned") =
                        Some(PendingAwait::new(
                            self.parent_task_id.clone(),
                            handle.clone(),
                            None,
                            handle.cancel_policy.clone(),
                        ));
                    self.client.register_waker(&handle.task_id, cx.waker());
                    self.state = CallState::Submitted { handle };
                    Poll::Pending
                }
                Err(error) => {
                    self.state = CallState::Done;
                    Poll::Ready(Err(error))
                }
            },
            CallState::Failed(mut error) => {
                self.state = CallState::Done;
                Poll::Ready(Err(error.take().expect("failed future contains error")))
            }
            CallState::Done => panic!("CallFuture polled after completion"),
        }
    }
}

fn serialize_error(error: serde_json::Error) -> RuntimeFailure {
    RuntimeFailure::new(mutsuki_runtime_contracts::RuntimeError::new(
        "sdk.serialize_failed",
        "runtime.sdk",
        error.to_string(),
    ))
}

struct PendingAwait {
    task: Option<Task>,
    task_await: TaskAwait,
}

impl PendingAwait {
    fn new(
        parent_task_id: String,
        child: TaskHandle,
        task: Option<Task>,
        cancel_policy: CancelPolicy,
    ) -> Self {
        Self {
            task,
            task_await: TaskAwait {
                parent_task_id: parent_task_id.clone(),
                child,
                continuation: TaskStepContinuation {
                    continuation: continuation_ref(&parent_task_id),
                    wake: None,
                    reason: Some("sdk.await".into()),
                },
                cancel_policy,
            },
        }
    }
}

pub type BoxedAsyncRunner = Box<
    dyn FnMut(
            AsyncRunnerContext,
            Task,
        ) -> Pin<Box<dyn Future<Output = RuntimeResult<RunnerResult>> + Send>>
        + Send,
>;

pub struct AsyncRunnerAdapter {
    descriptor: RunnerDescriptor,
    client: RuntimeClientRef,
    factory: BoxedAsyncRunner,
    invocations: HashMap<String, AsyncInvocation>,
    invocation_tasks: HashMap<String, String>,
    allow_self_call: bool,
}

struct AsyncInvocation {
    future: Pin<Box<dyn Future<Output = RuntimeResult<RunnerResult>> + Send>>,
    pending: Arc<Mutex<Option<PendingAwait>>>,
}

impl AsyncRunnerAdapter {
    pub fn new(
        descriptor: RunnerDescriptor,
        client: RuntimeClientRef,
        factory: BoxedAsyncRunner,
    ) -> Self {
        Self {
            descriptor,
            client,
            factory,
            invocations: HashMap::new(),
            invocation_tasks: HashMap::new(),
            allow_self_call: true,
        }
    }

    pub fn with_self_call_policy(mut self, allow_self_call: bool) -> Self {
        self.allow_self_call = allow_self_call;
        self
    }

    fn track_invocation(&mut self, task_id: &str, invocation_id: &str) {
        if invocation_id.is_empty() {
            return;
        }
        self.invocation_tasks
            .insert(invocation_id.to_owned(), task_id.to_owned());
    }

    fn remove_invocation_by_task(&mut self, task_id: &str) -> Option<AsyncInvocation> {
        let invocation = self.invocations.remove(task_id)?;
        self.invocation_tasks
            .retain(|_, known_task_id| known_task_id != task_id);
        Some(invocation)
    }
}

impl Runner for AsyncRunnerAdapter {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn step(&mut self, _ctx: RunnerContext, tasks: Vec<Task>) -> RuntimeResult<Vec<RunnerResult>> {
        let mut results = Vec::new();
        let invocation_id = _ctx.invocation_id.clone();
        for task in tasks {
            let task_id = task.task_id.clone();
            if !self.invocations.contains_key(&task_id) {
                let pending = Arc::new(Mutex::new(None));
                let async_ctx = AsyncRunnerContext {
                    client: self.client.clone(),
                    parent_task_id: task.task_id.clone(),
                    current_runner_id: self.descriptor.runner_id.clone(),
                    trace_id: task.trace_id.clone(),
                    correlation_id: task.correlation_id.clone(),
                    next_call: Arc::new(AtomicU64::new(0)),
                    pending: pending.clone(),
                    allow_self_call: self.allow_self_call,
                };
                let future = (self.factory)(async_ctx, task);
                self.invocations
                    .insert(task_id.clone(), AsyncInvocation { future, pending });
            }
            self.track_invocation(&task_id, &invocation_id);
            let invocation = self
                .invocations
                .get_mut(&task_id)
                .expect("invocation inserted before poll");
            *invocation
                .pending
                .lock()
                .expect("pending await mutex poisoned") = None;
            let waker = noop_waker();
            let mut cx = Context::from_waker(&waker);
            match invocation.future.as_mut().poll(&mut cx) {
                Poll::Ready(result) => {
                    self.remove_invocation_by_task(&task_id);
                    results.push(result?);
                }
                Poll::Pending => {
                    if let Some(pending) = invocation
                        .pending
                        .lock()
                        .expect("pending await mutex poisoned")
                        .take()
                    {
                        results.push(RunnerResult {
                            task_id,
                            deltas: Vec::new(),
                            events: Vec::new(),
                            tasks: pending.task.into_iter().collect(),
                            effects: Vec::new(),
                            values: Vec::new(),
                            resources: Vec::new(),
                            task_await: Some(pending.task_await),
                            status: RunnerStatus::Waiting,
                        });
                    } else {
                        results.push(RunnerResult {
                            task_id,
                            deltas: Vec::new(),
                            events: Vec::new(),
                            tasks: Vec::new(),
                            effects: Vec::new(),
                            values: Vec::new(),
                            resources: Vec::new(),
                            task_await: None,
                            status: RunnerStatus::Continue,
                        });
                    }
                }
            }
        }
        Ok(results)
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        if let Some(task_id) = self.invocation_tasks.get(invocation_id).cloned() {
            self.remove_invocation_by_task(&task_id);
        }
        Ok(())
    }
}

fn continuation_ref(parent_task_id: &str) -> ResourceRef {
    let ref_id = format!("continuation:{parent_task_id}");
    ResourceRef {
        resource_id: ResourceId {
            kind_id: "continuation".into(),
            slot_id: ref_id.clone(),
            generation: 1,
            version: 1,
        },
        ref_id,
        semantic: ResourceSemantic::FrozenValue,
        provider_id: "mutsuki.sdk".into(),
        resource_kind: "continuation".into(),
        schema: "mutsuki.continuation.v1".into(),
        version: 1,
        generation: 1,
        access: ResourceAccess::Inline,
        size_hint: None,
        content_hash: None,
        lifetime: ResourceLifetime::BorrowedUntilTaskEnd,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}

fn noop_waker() -> Waker {
    unsafe fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }
    unsafe fn wake(_: *const ()) {}
    unsafe fn wake_by_ref(_: *const ()) {}
    unsafe fn drop(_: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop);
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
}

#[cfg(test)]
mod tests;
