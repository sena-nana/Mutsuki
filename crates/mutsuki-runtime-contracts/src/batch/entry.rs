use serde::{Deserialize, Serialize};

use crate::{EntryId, PayloadIndex, RefId, TaskId, TraceId};

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
