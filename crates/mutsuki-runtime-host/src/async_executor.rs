use std::collections::BTreeMap;
use std::fmt;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_channel::oneshot;
use futures_util::FutureExt;
use mutsuki_runtime_contracts::{
    AsyncInvocation, AsyncInvocationHandle, CompletionBatch, ERR_CAPABILITY_EXHAUSTED,
    RuntimeError, ScalarValue,
};
use mutsuki_runtime_core::{AsyncCompletionFuture, RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::BoxRuntimeFuture;
use serde::Serialize;
use tokio::runtime::{Builder, Runtime};
use tokio::task::AbortHandle;

use crate::HostRuntimeReply;
use crate::error::host_failure;

pub enum AsyncExecutorEvent {
    Started(AsyncInvocation),
    Completed {
        invocation: AsyncInvocation,
        result: RuntimeResult<CompletionBatch>,
    },
    TimedOut(AsyncInvocation),
    Panicked(AsyncInvocation),
    ResourceCompleted {
        invocation: AsyncInvocation,
        reply: oneshot::Sender<RuntimeResult<HostRuntimeReply>>,
        result: Box<RuntimeResult<HostRuntimeReply>>,
    },
    ResourceTimedOut {
        invocation: AsyncInvocation,
        reply: oneshot::Sender<RuntimeResult<HostRuntimeReply>>,
    },
    ResourcePanicked {
        invocation: AsyncInvocation,
        reply: oneshot::Sender<RuntimeResult<HostRuntimeReply>>,
    },
}

pub type AsyncEventSink = Arc<dyn Fn(AsyncExecutorEvent) + Send + Sync + 'static>;

enum ResourceFutureOutcome {
    Completed(Box<RuntimeResult<HostRuntimeReply>>),
    TimedOut,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct AsyncExecutorSnapshot {
    pub executor_id: String,
    pub configured_threads: usize,
    pub running_invocations: usize,
    pub running_entries: usize,
    pub inflight_bytes: usize,
    pub max_inflight_invocations: usize,
    pub max_inflight_entries: usize,
    pub max_inflight_bytes: usize,
}

pub trait AsyncExecutor: Send + Sync + fmt::Debug {
    fn spawn(
        &self,
        invocation: AsyncInvocation,
        future: AsyncCompletionFuture,
        events: AsyncEventSink,
    ) -> RuntimeResult<AsyncInvocationHandle>;

    fn cancel(&self, handle: &AsyncInvocationHandle) -> RuntimeResult<bool>;

    fn spawn_resource(
        &self,
        invocation: AsyncInvocation,
        future: BoxRuntimeFuture<HostRuntimeReply>,
        reply: oneshot::Sender<RuntimeResult<HostRuntimeReply>>,
        events: AsyncEventSink,
    ) -> RuntimeResult<AsyncInvocationHandle>;

    fn cancel_all(&self) -> RuntimeResult<usize>;

    fn snapshot(&self) -> AsyncExecutorSnapshot;
}

#[derive(Debug)]
struct AsyncExecutorState {
    running_invocations: AtomicUsize,
    running_entries: AtomicUsize,
    inflight_bytes: AtomicUsize,
    handles: Mutex<BTreeMap<String, AbortHandle>>,
}

impl Default for AsyncExecutorState {
    fn default() -> Self {
        Self {
            running_invocations: AtomicUsize::new(0),
            running_entries: AtomicUsize::new(0),
            inflight_bytes: AtomicUsize::new(0),
            handles: Mutex::new(BTreeMap::new()),
        }
    }
}

pub struct TokioAsyncExecutor {
    executor_id: String,
    configured_threads: usize,
    max_inflight_invocations: usize,
    max_inflight_entries: usize,
    max_inflight_bytes: usize,
    runtime: Arc<Runtime>,
    state: Arc<AsyncExecutorState>,
}

impl fmt::Debug for TokioAsyncExecutor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokioAsyncExecutor")
            .field("executor_id", &self.executor_id)
            .field("configured_threads", &self.configured_threads)
            .field("max_inflight_invocations", &self.max_inflight_invocations)
            .field("max_inflight_entries", &self.max_inflight_entries)
            .field("max_inflight_bytes", &self.max_inflight_bytes)
            .finish_non_exhaustive()
    }
}

impl TokioAsyncExecutor {
    pub fn new(
        configured_threads: usize,
        max_inflight_invocations: usize,
        max_inflight_entries: usize,
        max_inflight_bytes: usize,
    ) -> RuntimeResult<Self> {
        if configured_threads == 0
            || max_inflight_invocations == 0
            || max_inflight_entries == 0
            || max_inflight_bytes == 0
        {
            return Err(host_failure(
                "host.async_executor.config",
                "async executor limits must be greater than zero",
            ));
        }
        let runtime = Builder::new_multi_thread()
            .worker_threads(configured_threads)
            .thread_name("mutsuki-async-io")
            .enable_time()
            .build()
            .map_err(|error| host_failure("host.async_executor.start", error.to_string()))?;
        Ok(Self {
            executor_id: "async_io".into(),
            configured_threads,
            max_inflight_invocations,
            max_inflight_entries,
            max_inflight_bytes,
            runtime: Arc::new(runtime),
            state: Arc::new(AsyncExecutorState::default()),
        })
    }

    fn reserve(&self, invocation: &AsyncInvocation) -> RuntimeResult<AsyncReservation> {
        reserve_counter(
            &self.state.running_invocations,
            1,
            self.max_inflight_invocations,
            "invocations",
        )?;
        if let Err(error) = reserve_counter(
            &self.state.running_entries,
            invocation.entry_count,
            self.max_inflight_entries,
            "entries",
        ) {
            self.state
                .running_invocations
                .fetch_sub(1, Ordering::AcqRel);
            return Err(error);
        }
        if let Err(error) = reserve_counter(
            &self.state.inflight_bytes,
            invocation.payload_bytes,
            self.max_inflight_bytes,
            "bytes",
        ) {
            self.state
                .running_entries
                .fetch_sub(invocation.entry_count, Ordering::AcqRel);
            self.state
                .running_invocations
                .fetch_sub(1, Ordering::AcqRel);
            return Err(error);
        }
        Ok(AsyncReservation {
            state: self.state.clone(),
            entry_count: invocation.entry_count,
            payload_bytes: invocation.payload_bytes,
        })
    }
}

impl AsyncExecutor for TokioAsyncExecutor {
    fn spawn(
        &self,
        invocation: AsyncInvocation,
        future: AsyncCompletionFuture,
        events: AsyncEventSink,
    ) -> RuntimeResult<AsyncInvocationHandle> {
        let reservation = self.reserve(&invocation)?;
        let state = self.state.clone();
        let invocation_for_task = invocation.clone();
        let (start_tx, start_rx) = tokio::sync::oneshot::channel::<()>();
        let task = self.runtime.spawn(async move {
            let _ = start_rx.await;
            let _reservation = reservation;
            events(AsyncExecutorEvent::Started(invocation_for_task.clone()));
            let deadline = invocation_for_task
                .deadline_after_ms
                .map(Duration::from_millis);
            let outcome = AssertUnwindSafe(async {
                match deadline {
                    Some(deadline) => match tokio::time::timeout(deadline, future).await {
                        Ok(result) => AsyncExecutorEvent::Completed {
                            invocation: invocation_for_task.clone(),
                            result,
                        },
                        Err(_) => AsyncExecutorEvent::TimedOut(invocation_for_task.clone()),
                    },
                    None => AsyncExecutorEvent::Completed {
                        invocation: invocation_for_task.clone(),
                        result: future.await,
                    },
                }
            })
            .catch_unwind()
            .await
            .unwrap_or_else(|_| AsyncExecutorEvent::Panicked(invocation_for_task.clone()));
            state
                .handles
                .lock()
                .expect("async executor handle lock poisoned")
                .remove(&invocation_for_task.invocation_id);
            events(outcome);
        });
        self.state
            .handles
            .lock()
            .expect("async executor handle lock poisoned")
            .insert(invocation.invocation_id.clone(), task.abort_handle());
        let _ = start_tx.send(());
        Ok(AsyncInvocationHandle {
            invocation_id: invocation.invocation_id,
            cancel_token: invocation.cancel_token,
        })
    }

    fn cancel(&self, handle: &AsyncInvocationHandle) -> RuntimeResult<bool> {
        let abort = self
            .state
            .handles
            .lock()
            .expect("async executor handle lock poisoned")
            .remove(&handle.invocation_id);
        if let Some(abort) = abort {
            abort.abort();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn spawn_resource(
        &self,
        invocation: AsyncInvocation,
        future: BoxRuntimeFuture<HostRuntimeReply>,
        reply: oneshot::Sender<RuntimeResult<HostRuntimeReply>>,
        events: AsyncEventSink,
    ) -> RuntimeResult<AsyncInvocationHandle> {
        let reservation = match self.reserve(&invocation) {
            Ok(reservation) => reservation,
            Err(failure) => {
                let returned = RuntimeFailure::new(failure.error().clone());
                let _ = reply.send(Err(failure));
                return Err(returned);
            }
        };
        let state = self.state.clone();
        let invocation_for_task = invocation.clone();
        let (start_tx, start_rx) = tokio::sync::oneshot::channel::<()>();
        let task = self.runtime.spawn(async move {
            let _ = start_rx.await;
            let _reservation = reservation;
            let deadline = invocation_for_task
                .deadline_after_ms
                .map(Duration::from_millis);
            let outcome = AssertUnwindSafe(async {
                match deadline {
                    Some(deadline) => match tokio::time::timeout(deadline, future).await {
                        Ok(result) => ResourceFutureOutcome::Completed(Box::new(result)),
                        Err(_) => ResourceFutureOutcome::TimedOut,
                    },
                    None => ResourceFutureOutcome::Completed(Box::new(future.await)),
                }
            })
            .catch_unwind()
            .await;
            state
                .handles
                .lock()
                .expect("async executor handle lock poisoned")
                .remove(&invocation_for_task.invocation_id);
            match outcome {
                Ok(ResourceFutureOutcome::Completed(result)) => {
                    events(AsyncExecutorEvent::ResourceCompleted {
                        invocation: invocation_for_task,
                        reply,
                        result,
                    })
                }
                Ok(ResourceFutureOutcome::TimedOut) => {
                    events(AsyncExecutorEvent::ResourceTimedOut {
                        invocation: invocation_for_task,
                        reply,
                    })
                }
                Err(_) => events(AsyncExecutorEvent::ResourcePanicked {
                    invocation: invocation_for_task,
                    reply,
                }),
            }
        });
        self.state
            .handles
            .lock()
            .expect("async executor handle lock poisoned")
            .insert(invocation.invocation_id.clone(), task.abort_handle());
        let _ = start_tx.send(());
        Ok(AsyncInvocationHandle {
            invocation_id: invocation.invocation_id,
            cancel_token: invocation.cancel_token,
        })
    }

    fn cancel_all(&self) -> RuntimeResult<usize> {
        let handles = std::mem::take(
            &mut *self
                .state
                .handles
                .lock()
                .expect("async executor handle lock poisoned"),
        );
        let count = handles.len();
        for handle in handles.into_values() {
            handle.abort();
        }
        Ok(count)
    }

    fn snapshot(&self) -> AsyncExecutorSnapshot {
        AsyncExecutorSnapshot {
            executor_id: self.executor_id.clone(),
            configured_threads: self.configured_threads,
            running_invocations: self.state.running_invocations.load(Ordering::Acquire),
            running_entries: self.state.running_entries.load(Ordering::Acquire),
            inflight_bytes: self.state.inflight_bytes.load(Ordering::Acquire),
            max_inflight_invocations: self.max_inflight_invocations,
            max_inflight_entries: self.max_inflight_entries,
            max_inflight_bytes: self.max_inflight_bytes,
        }
    }
}

struct AsyncReservation {
    state: Arc<AsyncExecutorState>,
    entry_count: usize,
    payload_bytes: usize,
}

impl Drop for AsyncReservation {
    fn drop(&mut self) {
        self.state
            .running_invocations
            .fetch_sub(1, Ordering::AcqRel);
        self.state
            .running_entries
            .fetch_sub(self.entry_count, Ordering::AcqRel);
        self.state
            .inflight_bytes
            .fetch_sub(self.payload_bytes, Ordering::AcqRel);
    }
}

fn reserve_counter(
    counter: &AtomicUsize,
    amount: usize,
    limit: usize,
    dimension: &str,
) -> RuntimeResult<()> {
    let mut current = counter.load(Ordering::Acquire);
    loop {
        let Some(next) = current.checked_add(amount) else {
            return Err(capacity_error(dimension, amount, current, limit));
        };
        if next > limit {
            return Err(capacity_error(dimension, amount, current, limit));
        }
        match counter.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Ok(()),
            Err(observed) => current = observed,
        }
    }
}

fn capacity_error(
    dimension: &str,
    requested: usize,
    current: usize,
    limit: usize,
) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        ERR_CAPABILITY_EXHAUSTED,
        "runtime.host.async_executor",
        format!("host.async_executor.capacity.{dimension}"),
    );
    error
        .evidence
        .insert("requested".into(), ScalarValue::Int(requested as i64));
    error
        .evidence
        .insert("current".into(), ScalarValue::Int(current as i64));
    error
        .evidence
        .insert("limit".into(), ScalarValue::Int(limit as i64));
    RuntimeFailure::new(error)
}
