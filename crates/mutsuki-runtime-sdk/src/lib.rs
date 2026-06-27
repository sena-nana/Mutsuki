use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use mutsuki_runtime_contracts::{
    CancelPolicy, ResourceAccess, ResourceLifetime, ResourceRef, ResourceSealState,
    RunnerDescriptor, RunnerResult, RunnerStatus, Task, TaskAwait, TaskHandle, TaskOutcome,
    TaskStepContinuation,
};
use mutsuki_runtime_core::{CoreRuntime, Runner, RunnerContext, RuntimeResult};
use serde_json::Value;

pub trait RuntimeClient {
    fn submit_task(&self, task: Task) -> RuntimeResult<TaskHandle>;
    fn task_outcome(&self, task_id: &str) -> RuntimeResult<Option<TaskOutcome>>;
    fn register_waker(&self, _task_id: &str, _waker: &Waker) {}
}

pub type RuntimeClientRef = Rc<dyn RuntimeClient>;

impl RuntimeClient for Rc<RefCell<CoreRuntime>> {
    fn submit_task(&self, task: Task) -> RuntimeResult<TaskHandle> {
        self.borrow_mut().submit_task_handle(task)
    }

    fn task_outcome(&self, task_id: &str) -> RuntimeResult<Option<TaskOutcome>> {
        self.borrow().task_outcome(task_id)
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
    next_call: Rc<Cell<u64>>,
    pending: Rc<RefCell<Option<PendingAwait>>>,
    allow_self_call: bool,
}

impl AsyncRunnerContext {
    pub fn task_id(&self) -> &str {
        &self.parent_task_id
    }

    pub fn call(&self, protocol_id: impl Into<String>, payload: Value) -> CallFuture {
        self.call_with_runner_hint(protocol_id, payload, None)
    }

    pub fn call_targeted(
        &self,
        binding_id: impl Into<String>,
        protocol_id: impl Into<String>,
        runner_hint: impl Into<String>,
        payload: Value,
    ) -> CallFuture {
        self.call_with_runner_hint(
            protocol_id,
            payload,
            Some((binding_id.into(), runner_hint.into())),
        )
    }

    fn call_with_runner_hint(
        &self,
        protocol_id: impl Into<String>,
        payload: Value,
        target: Option<(String, String)>,
    ) -> CallFuture {
        let call_index = self.next_call.get() + 1;
        self.next_call.set(call_index);
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
            cancel_policy: CancelPolicy::Cascade,
            trace_id: self.trace_id.clone(),
            correlation_id: self.correlation_id.clone(),
        };
        CallFuture {
            client: self.client.clone(),
            parent_task_id: self.parent_task_id.clone(),
            pending: self.pending.clone(),
            state: CallState::Init { task, handle },
            self_call_blocked,
        }
    }
}

pub struct CallFuture {
    client: RuntimeClientRef,
    parent_task_id: String,
    pending: Rc<RefCell<Option<PendingAwait>>>,
    state: CallState,
    self_call_blocked: bool,
}

enum CallState {
    Init { task: Task, handle: TaskHandle },
    Submitted { handle: TaskHandle },
    Done,
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
                let pending =
                    PendingAwait::new(self.parent_task_id.clone(), handle.clone(), Some(task));
                *self.pending.borrow_mut() = Some(pending);
                self.state = CallState::Submitted { handle };
                Poll::Pending
            }
            CallState::Submitted { handle } => match self.client.task_outcome(&handle.task_id) {
                Ok(Some(outcome)) => {
                    self.state = CallState::Done;
                    Poll::Ready(Ok(outcome))
                }
                Ok(None) => {
                    *self.pending.borrow_mut() = Some(PendingAwait::new(
                        self.parent_task_id.clone(),
                        handle.clone(),
                        None,
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
            CallState::Done => panic!("CallFuture polled after completion"),
        }
    }
}

struct PendingAwait {
    task: Option<Task>,
    task_await: TaskAwait,
}

impl PendingAwait {
    fn new(parent_task_id: String, child: TaskHandle, task: Option<Task>) -> Self {
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
                cancel_policy: CancelPolicy::Cascade,
            },
        }
    }
}

pub type BoxedAsyncRunner = Box<
    dyn FnMut(
        AsyncRunnerContext,
        Task,
    ) -> Pin<Box<dyn Future<Output = RuntimeResult<RunnerResult>>>>,
>;

pub struct AsyncRunnerAdapter {
    descriptor: RunnerDescriptor,
    client: RuntimeClientRef,
    factory: BoxedAsyncRunner,
    invocations: HashMap<String, AsyncInvocation>,
    allow_self_call: bool,
}

struct AsyncInvocation {
    future: Pin<Box<dyn Future<Output = RuntimeResult<RunnerResult>>>>,
    pending: Rc<RefCell<Option<PendingAwait>>>,
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
            allow_self_call: true,
        }
    }

    pub fn with_self_call_policy(mut self, allow_self_call: bool) -> Self {
        self.allow_self_call = allow_self_call;
        self
    }
}

impl Runner for AsyncRunnerAdapter {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn step(&mut self, _ctx: RunnerContext, tasks: Vec<Task>) -> RuntimeResult<Vec<RunnerResult>> {
        let mut results = Vec::new();
        for task in tasks {
            let task_id = task.task_id.clone();
            if !self.invocations.contains_key(&task_id) {
                let pending = Rc::new(RefCell::new(None));
                let async_ctx = AsyncRunnerContext {
                    client: self.client.clone(),
                    parent_task_id: task.task_id.clone(),
                    current_runner_id: self.descriptor.runner_id.clone(),
                    trace_id: task.trace_id.clone(),
                    correlation_id: task.correlation_id.clone(),
                    next_call: Rc::new(Cell::new(0)),
                    pending: pending.clone(),
                    allow_self_call: self.allow_self_call,
                };
                let future = (self.factory)(async_ctx, task);
                self.invocations
                    .insert(task_id.clone(), AsyncInvocation { future, pending });
            }
            let invocation = self
                .invocations
                .get_mut(&task_id)
                .expect("invocation inserted before poll");
            *invocation.pending.borrow_mut() = None;
            let waker = noop_waker();
            let mut cx = Context::from_waker(&waker);
            match invocation.future.as_mut().poll(&mut cx) {
                Poll::Ready(result) => {
                    self.invocations.remove(&task_id);
                    results.push(result?);
                }
                Poll::Pending => {
                    if let Some(pending) = invocation.pending.borrow_mut().take() {
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
        self.invocations.remove(invocation_id);
        Ok(())
    }
}

fn continuation_ref(parent_task_id: &str) -> ResourceRef {
    ResourceRef {
        ref_id: format!("continuation:{parent_task_id}"),
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
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use mutsuki_runtime_contracts::{RunnerPurity, RuntimeError};
    use serde_json::json;

    struct ManualClient {
        outcomes: RefCell<HashMap<String, TaskOutcome>>,
        submitted: RefCell<Vec<Task>>,
    }

    impl RuntimeClient for ManualClient {
        fn submit_task(&self, task: Task) -> RuntimeResult<TaskHandle> {
            let handle = TaskHandle {
                task_id: task.task_id.clone(),
                protocol_id: task.protocol_id.clone(),
                target_binding_id: task.target_binding_id.clone(),
                cancel_policy: CancelPolicy::Cascade,
                trace_id: task.trace_id.clone(),
                correlation_id: task.correlation_id.clone(),
            };
            self.submitted.borrow_mut().push(task);
            Ok(handle)
        }

        fn task_outcome(&self, task_id: &str) -> RuntimeResult<Option<TaskOutcome>> {
            Ok(self.outcomes.borrow().get(task_id).cloned())
        }
    }

    #[test]
    fn task_handle_future_polls_until_outcome() {
        let client = Rc::new(ManualClient {
            outcomes: RefCell::new(HashMap::new()),
            submitted: RefCell::new(Vec::new()),
        });
        let handle = TaskHandle {
            task_id: "task-1".into(),
            protocol_id: "child.work".into(),
            target_binding_id: None,
            cancel_policy: CancelPolicy::Cascade,
            trace_id: None,
            correlation_id: None,
        };
        let mut future = Box::pin(TaskHandleFuture::new(client.clone(), handle));
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());
        client.outcomes.borrow_mut().insert(
            "task-1".into(),
            TaskOutcome::Completed {
                task_id: "task-1".into(),
                output_ref: Some("value:1".into()),
            },
        );

        assert!(matches!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(_))));
    }

    #[test]
    fn async_runner_adapter_suspends_and_resumes_call() {
        let client = Rc::new(ManualClient {
            outcomes: RefCell::new(HashMap::new()),
            submitted: RefCell::new(Vec::new()),
        });
        let descriptor = RunnerDescriptor {
            runner_id: "async.runner".into(),
            plugin_id: "plugin-a".into(),
            plugin_generation: 1,
            accepted_protocol_ids: vec!["parent.work".into()],
            purity: RunnerPurity::Pure,
            input_schema: json!({}),
            output_schema: json!({}),
            metadata: BTreeMap::new(),
            contract_surfaces: vec!["runner:async.runner".into()],
        };
        let mut adapter = AsyncRunnerAdapter::new(
            descriptor,
            client.clone(),
            Box::new(|ctx, task| {
                Box::pin(async move {
                    let outcome = ctx
                        .call("child.work", json!({"from": task.task_id}))
                        .await?;
                    match outcome {
                        TaskOutcome::Completed { .. } => Ok(RunnerResult::completed(task.task_id)),
                        TaskOutcome::Failed { error, .. } => {
                            Err(mutsuki_runtime_core::RuntimeFailure::new(error))
                        }
                        _ => Err(mutsuki_runtime_core::RuntimeFailure::new(
                            RuntimeError::new(
                                "task.await_unexpected_outcome",
                                "runtime.sdk",
                                "sdk.test",
                            ),
                        )),
                    }
                })
            }),
        );

        let first = adapter
            .step(
                RunnerContext {
                    registry_generation: 1,
                    current_step: 1,
                    executor_id: "executor:test".into(),
                    task_lease_id: Some("lease:test".into()),
                },
                vec![Task::new("parent-1", "parent.work", json!({}))],
            )
            .unwrap();

        assert_eq!(first[0].status, RunnerStatus::Waiting);
        assert_eq!(first[0].tasks[0].task_id, "parent-1:call:1");
        client.outcomes.borrow_mut().insert(
            "parent-1:call:1".into(),
            TaskOutcome::Completed {
                task_id: "parent-1:call:1".into(),
                output_ref: None,
            },
        );

        let second = adapter
            .step(
                RunnerContext {
                    registry_generation: 1,
                    current_step: 2,
                    executor_id: "executor:test".into(),
                    task_lease_id: Some("lease:test-2".into()),
                },
                vec![Task::new("parent-1", "parent.work", json!({}))],
            )
            .unwrap();

        assert_eq!(second[0].status, RunnerStatus::Completed);
    }

    #[test]
    fn async_runner_adapter_rejects_self_call_when_policy_disallows_it() {
        let client = Rc::new(ManualClient {
            outcomes: RefCell::new(HashMap::new()),
            submitted: RefCell::new(Vec::new()),
        });
        let descriptor = RunnerDescriptor {
            runner_id: "async.runner".into(),
            plugin_id: "plugin-a".into(),
            plugin_generation: 1,
            accepted_protocol_ids: vec!["parent.work".into()],
            purity: RunnerPurity::Pure,
            input_schema: json!({}),
            output_schema: json!({}),
            metadata: BTreeMap::new(),
            contract_surfaces: vec!["runner:async.runner".into()],
        };
        let mut adapter = AsyncRunnerAdapter::new(
            descriptor,
            client,
            Box::new(|ctx, task| {
                Box::pin(async move {
                    let task_id = task.task_id.clone();
                    ctx.call_targeted(
                        "binding:self",
                        "parent.work",
                        "async.runner",
                        json!({"from": task_id}),
                    )
                    .await?;
                    Ok(RunnerResult::completed(task.task_id))
                })
            }),
        )
        .with_self_call_policy(false);

        let error = adapter
            .step(
                RunnerContext {
                    registry_generation: 1,
                    current_step: 1,
                    executor_id: "executor:test".into(),
                    task_lease_id: Some("lease:test".into()),
                },
                vec![Task::new("parent-1", "parent.work", json!({}))],
            )
            .unwrap_err();

        assert_eq!(error.error().code, "task.self_call_blocked");
    }

    #[test]
    fn async_runner_adapter_emits_targeted_child_task_descriptor() {
        let client = Rc::new(ManualClient {
            outcomes: RefCell::new(HashMap::new()),
            submitted: RefCell::new(Vec::new()),
        });
        let descriptor = RunnerDescriptor {
            runner_id: "async.runner".into(),
            plugin_id: "plugin-a".into(),
            plugin_generation: 1,
            accepted_protocol_ids: vec!["parent.work".into()],
            purity: RunnerPurity::Pure,
            input_schema: json!({}),
            output_schema: json!({}),
            metadata: BTreeMap::new(),
            contract_surfaces: vec!["runner:async.runner".into()],
        };
        let mut adapter = AsyncRunnerAdapter::new(
            descriptor,
            client,
            Box::new(|ctx, task| {
                Box::pin(async move {
                    ctx.call_targeted(
                        "binding:child",
                        "child.work",
                        "child.runner",
                        json!({"from": task.task_id}),
                    )
                    .await?;
                    Ok(RunnerResult::completed(task.task_id))
                })
            }),
        );

        let first = adapter
            .step(
                RunnerContext {
                    registry_generation: 1,
                    current_step: 1,
                    executor_id: "executor:test".into(),
                    task_lease_id: Some("lease:test".into()),
                },
                vec![Task::new("parent-1", "parent.work", json!({}))],
            )
            .unwrap();

        assert_eq!(first[0].status, RunnerStatus::Waiting);
        assert_eq!(
            first[0].tasks[0].target_binding_id.as_deref(),
            Some("binding:child")
        );
        assert_eq!(
            first[0].tasks[0].runner_hint.as_deref(),
            Some("child.runner")
        );
        assert_eq!(
            first[0]
                .task_await
                .as_ref()
                .unwrap()
                .child
                .target_binding_id
                .as_deref(),
            Some("binding:child")
        );
    }
}
