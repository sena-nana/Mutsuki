mod completion;
mod entry;
mod payload;
mod resource_plan;

use serde::{Deserialize, Serialize};

use crate::{BatchId, BatchKey, Task, TaskLease, TickId};

pub use completion::{CompletionBatch, EntryCompletion};
pub use entry::{
    BatchEntry, DispatchLane, OrderingRequirement, ResourceAccessMode, ResourceRequirement,
};
pub use payload::{
    BatchPayload, BinaryPackedPayload, ColumnPayload, ColumnarPayload, PayloadLayout,
    ResourceBackedPayload, ResourceSlice, RowPayload,
};
pub use resource_plan::{
    DeferredResourceOp, ResourceReadView, ResourceWriteLock, WorkResourcePlan,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskBatch {
    pub batch_id: BatchId,
    pub tick_id: Option<TickId>,
    pub tasks: Vec<Task>,
    pub resource_plan: Option<WorkResourcePlan>,
}

impl TaskBatch {
    pub fn one(batch_id: impl Into<BatchId>, task: Task) -> Self {
        Self {
            batch_id: batch_id.into(),
            tick_id: None,
            tasks: vec![task],
            resource_plan: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkSet {
    pub tick_id: TickId,
    pub batch_key: BatchKey,
    pub entries: Vec<BatchEntry>,
    pub resource_requirements: Vec<ResourceRequirement>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkBatch {
    pub batch_id: BatchId,
    pub tick_id: TickId,
    pub batch_key: BatchKey,
    pub entries: Vec<BatchEntry>,
    pub payload: BatchPayload,
    pub resource_plan: WorkResourcePlan,
    pub task_leases: Vec<TaskLease>,
}

impl WorkBatch {
    // Keep the same structured RuntimeError contract as BatchPayload::try_row_tasks.
    #[allow(clippy::result_large_err)]
    pub fn row_payload_tasks(&self) -> Result<Vec<Task>, crate::RuntimeError> {
        self.payload.try_row_tasks()
    }
}
