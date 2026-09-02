use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use mutsuki_runtime_contracts::{
    CompletionBatch, EntryCompletion, RunnerContext, RunnerDescriptor, RunnerResult, RunnerStatus,
    Task, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerManagementHandle, RuntimeResult};

type NativeEntryHandler = Box<dyn FnMut(RunnerContext, Task) -> RuntimeResult<RunnerResult> + Send>;
type BorrowedNativeEntryHandler =
    Box<dyn FnMut(&RunnerContext, &Task) -> RuntimeResult<RunnerResult> + Send>;
type CancellableNativeEntryHandler =
    Box<dyn FnMut(RunnerContext, Task, CancellationProbe) -> RuntimeResult<RunnerResult> + Send>;
type BorrowedCancellableNativeEntryHandler =
    Box<dyn FnMut(&RunnerContext, &Task, &CancellationProbe) -> RuntimeResult<RunnerResult> + Send>;

enum NativeHandler {
    Owned(NativeEntryHandler),
    Borrowed(BorrowedNativeEntryHandler),
    CancellableOwned(CancellableNativeEntryHandler),
    CancellableBorrowed(BorrowedCancellableNativeEntryHandler),
}

/// Host-local cooperative cancellation state for one in-process invocation.
///
/// This probe is deliberately not part of `RunnerContext` or any wire contract.
/// Native domain code should query it immediately before irreversible side
/// effects and periodically while performing long-running work.
#[derive(Clone, Debug)]
pub struct CancellationProbe {
    requested: Arc<AtomicBool>,
}

impl CancellationProbe {
    pub fn is_cancelled(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }
}

#[derive(Debug, Default)]
struct NativeCancellationRegistry {
    invocations: Mutex<BTreeMap<String, Arc<AtomicBool>>>,
}

impl NativeCancellationRegistry {
    fn probe(&self, invocation_id: &str) -> CancellationProbe {
        let mut invocations = self
            .invocations
            .lock()
            .expect("native cancellation registry lock poisoned");
        invocations.retain(|known_id, _| known_id == invocation_id);
        let requested = invocations
            .entry(invocation_id.to_string())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .clone();
        CancellationProbe { requested }
    }

    fn request(&self, invocation_id: &str) {
        self.invocations
            .lock()
            .expect("native cancellation registry lock poisoned")
            .entry(invocation_id.to_string())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .store(true, Ordering::Release);
    }

    fn finish(&self, invocation_id: &str) {
        self.invocations
            .lock()
            .expect("native cancellation registry lock poisoned")
            .remove(invocation_id);
    }

    fn clear(&self) {
        self.invocations
            .lock()
            .expect("native cancellation registry lock poisoned")
            .clear();
    }
}

impl RunnerManagementHandle for NativeCancellationRegistry {
    fn cancel(&self, invocation_id: &str) -> RuntimeResult<()> {
        self.request(invocation_id);
        Ok(())
    }

    fn cancels_in_process(&self) -> bool {
        true
    }

    fn dispose(&self) -> RuntimeResult<()> {
        self.clear();
        Ok(())
    }
}

pub struct NativeRunner {
    descriptor: RunnerDescriptor,
    handler: NativeHandler,
    cancellations: Arc<NativeCancellationRegistry>,
}

impl NativeRunner {
    pub fn new(
        descriptor: RunnerDescriptor,
        handler: impl FnMut(RunnerContext, Task) -> RuntimeResult<RunnerResult> + Send + 'static,
    ) -> Self {
        Self::with_handler(descriptor, NativeHandler::Owned(Box::new(handler)))
    }

    /// Creates a builtin runner whose entry handler borrows typed local tasks.
    ///
    /// This is the allocation-free in-process path. Wire-backed payloads are
    /// still decoded to an owned temporary by `BatchPayload::task_at`.
    pub fn new_borrowed(
        descriptor: RunnerDescriptor,
        handler: impl FnMut(&RunnerContext, &Task) -> RuntimeResult<RunnerResult> + Send + 'static,
    ) -> Self {
        Self::with_handler(descriptor, NativeHandler::Borrowed(Box::new(handler)))
    }

    pub fn new_cancellable(
        descriptor: RunnerDescriptor,
        handler: impl FnMut(RunnerContext, Task, CancellationProbe) -> RuntimeResult<RunnerResult>
        + Send
        + 'static,
    ) -> Self {
        Self::with_handler(
            descriptor,
            NativeHandler::CancellableOwned(Box::new(handler)),
        )
    }

    pub fn new_borrowed_cancellable(
        descriptor: RunnerDescriptor,
        handler: impl FnMut(&RunnerContext, &Task, &CancellationProbe) -> RuntimeResult<RunnerResult>
        + Send
        + 'static,
    ) -> Self {
        Self::with_handler(
            descriptor,
            NativeHandler::CancellableBorrowed(Box::new(handler)),
        )
    }

    fn with_handler(descriptor: RunnerDescriptor, handler: NativeHandler) -> Self {
        Self {
            descriptor,
            handler,
            cancellations: Arc::new(NativeCancellationRegistry::default()),
        }
    }
}

struct NativeInvocationGuard(Arc<NativeCancellationRegistry>, String);

impl Drop for NativeInvocationGuard {
    fn drop(&mut self) {
        self.0.finish(&self.1);
    }
}

impl Runner for NativeRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        let probe = self.cancellations.probe(&ctx.invocation_id);
        let _invocation_guard =
            NativeInvocationGuard(self.cancellations.clone(), ctx.invocation_id.clone());
        let mut results = Vec::with_capacity(batch.entries.len());
        for entry in &batch.entries {
            if probe.is_cancelled() {
                results.push(cancelled_entry(entry));
                continue;
            }
            let task = match batch.payload_task(entry.payload_index) {
                Ok(task) if task.task_id == entry.task_id => task,
                Ok(_) => {
                    results.push(EntryCompletion {
                        entry_id: entry.entry_id.clone(),
                        task_id: entry.task_id.clone(),
                        result: None,
                        error: Some(mutsuki_runtime_contracts::RuntimeError::new(
                            mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT,
                            "native_runner",
                            format!("batch.entry.{}.payload_task_id", entry.entry_id),
                        )),
                    });
                    continue;
                }
                Err(error) => {
                    results.push(EntryCompletion {
                        entry_id: entry.entry_id.clone(),
                        task_id: entry.task_id.clone(),
                        result: None,
                        error: Some(error),
                    });
                    continue;
                }
            };
            if probe.is_cancelled() {
                results.push(cancelled_entry(entry));
                continue;
            }
            let result = match &mut self.handler {
                NativeHandler::Owned(handler) => handler(ctx.clone(), task.into_owned()),
                NativeHandler::Borrowed(handler) => handler(&ctx, task.as_ref()),
                NativeHandler::CancellableOwned(handler) => {
                    handler(ctx.clone(), task.into_owned(), probe.clone())
                }
                NativeHandler::CancellableBorrowed(handler) => handler(&ctx, task.as_ref(), &probe),
            };
            match result {
                Ok(result) => results.push(EntryCompletion {
                    entry_id: entry.entry_id.clone(),
                    task_id: entry.task_id.clone(),
                    result: Some(result),
                    error: None,
                }),
                Err(failure) => results.push(EntryCompletion {
                    entry_id: entry.entry_id.clone(),
                    task_id: entry.task_id.clone(),
                    result: None,
                    error: Some(failure.error().clone()),
                }),
            }
        }
        Ok(CompletionBatch::from_results(&batch, results))
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.cancellations.request(invocation_id);
        Ok(())
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        self.cancellations.clear();
        Ok(())
    }

    fn management_handle(&self) -> Option<Arc<dyn RunnerManagementHandle>> {
        Some(self.cancellations.clone())
    }
}

fn cancelled_entry(entry: &mutsuki_runtime_contracts::BatchEntry) -> EntryCompletion {
    let mut result = RunnerResult::completed(entry.task_id.clone());
    result.status = RunnerStatus::Cancelled;
    EntryCompletion {
        entry_id: entry.entry_id.clone(),
        task_id: entry.task_id.clone(),
        result: Some(result),
        error: None,
    }
}
