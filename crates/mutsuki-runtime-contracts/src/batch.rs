use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    BatchId, BatchKey, EntryId, PayloadIndex, RefId, ResourceRef, RuntimeError, ScalarValue, Task,
    TaskId, TaskLease, TickId, TraceId,
};
use crate::{RunnerResult, VersionExpectation};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchLane {
    Control,
    Interactive,
    Normal,
    Background,
    Bulk,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OrderingRequirement {
    None,
    PreserveSubmitOrder,
    SameResourceOrder { ref_id: RefId },
    StrictSequence { sequence_id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceAccessMode {
    Read,
    Write,
    ExclusiveWrite,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayloadLayout {
    Row,
    Columnar,
    BinaryPacked,
    ResourceBacked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRequirement {
    pub ref_id: RefId,
    pub mode: ResourceAccessMode,
    pub expected_version: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BatchEntry {
    pub entry_id: EntryId,
    pub task_id: TaskId,
    pub trace_id: Option<TraceId>,
    pub parent_id: Option<EntryId>,
    pub payload_index: PayloadIndex,
    pub resource_requirement_indices: Vec<usize>,
    pub cancel_index: Option<usize>,
    pub deadline_tick: Option<u64>,
    pub priority: i64,
    pub lane: DispatchLane,
    pub ordering: OrderingRequirement,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnarPayload {
    pub columns: Vec<ColumnPayload>,
    pub row_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnPayload {
    pub name: String,
    pub values: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PackedBuffer {
    pub encoding: String,
    pub bytes: Vec<u8>,
    pub row_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceSlice {
    pub resource: ResourceRef,
    pub offset: u64,
    pub length: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceSliceSet {
    pub slices: Vec<ResourceSlice>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "layout", rename_all = "snake_case")]
pub enum BatchPayload {
    Row { entries: Vec<Value> },
    Columnar { payload: ColumnarPayload },
    BinaryPacked { buffer: PackedBuffer },
    ResourceBacked { slices: ResourceSliceSet },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskBatch {
    pub batch_id: BatchId,
    pub tick_id: Option<TickId>,
    pub tasks: Vec<Task>,
    pub payload_layout: PayloadLayout,
    pub resource_plan: Option<WorkResourcePlan>,
}

impl TaskBatch {
    pub fn one(batch_id: impl Into<BatchId>, task: Task) -> Self {
        Self {
            batch_id: batch_id.into(),
            tick_id: None,
            tasks: vec![task],
            payload_layout: PayloadLayout::Row,
            resource_plan: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceReadView {
    pub ref_id: RefId,
    pub requirement_indices: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceWriteLock {
    pub ref_id: RefId,
    pub requirement_indices: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeferredResourceOp {
    pub entry_id: EntryId,
    pub ref_id: RefId,
    pub operation: String,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkResourcePlan {
    pub read_views: Vec<ResourceReadView>,
    pub write_locks: Vec<ResourceWriteLock>,
    pub version_checks: Vec<VersionExpectation>,
    pub deferred_writes: Vec<DeferredResourceOp>,
    pub conflict_entries: Vec<EntryId>,
}

impl WorkResourcePlan {
    pub fn empty() -> Self {
        Self {
            read_views: Vec::new(),
            write_locks: Vec::new(),
            version_checks: Vec::new(),
            deferred_writes: Vec::new(),
            conflict_entries: Vec::new(),
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntryCompletion {
    pub entry_id: EntryId,
    pub task_id: TaskId,
    pub result: Option<RunnerResult>,
    pub error: Option<RuntimeError>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompletionBatch {
    pub batch_id: BatchId,
    pub tick_id: TickId,
    pub results: Vec<EntryCompletion>,
    pub metadata: Vec<(String, ScalarValue)>,
}

impl CompletionBatch {
    pub fn from_results(batch: &WorkBatch, results: Vec<EntryCompletion>) -> Self {
        Self {
            batch_id: batch.batch_id.clone(),
            tick_id: batch.tick_id.clone(),
            results,
            metadata: Vec::new(),
        }
    }
}

impl WorkBatch {
    pub fn row_payload_tasks(&self) -> Vec<Task> {
        match &self.payload {
            BatchPayload::Row { entries } => entries
                .iter()
                .filter_map(|value| serde_json::from_value(value.clone()).ok())
                .collect(),
            _ => Vec::new(),
        }
    }
}
