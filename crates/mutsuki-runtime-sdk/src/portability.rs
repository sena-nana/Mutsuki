use mutsuki_runtime_contracts::{PortableTask, TaskCheckpoint};

use crate::RuntimeResult;

/// Optional side contract for plugin-owned task state.
///
/// Implementing this trait does not change `Runner::run_batch` and is not required for local
/// execution. A host or replay tool may query a separately registered implementation only when
/// the matching portability descriptor opts into checkpoint recovery.
pub trait Checkpointable: Send {
    fn checkpoint(&mut self, task: &PortableTask, sequence: u64) -> RuntimeResult<TaskCheckpoint>;

    fn restore(&mut self, checkpoint: &TaskCheckpoint) -> RuntimeResult<PortableTask>;
}
