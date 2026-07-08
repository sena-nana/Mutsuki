use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{EntryId, RefId, VersionExpectation};

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
    pub parallel_groups: Vec<Vec<EntryId>>,
    pub serial_groups: Vec<Vec<EntryId>>,
    pub parallelism_limit: usize,
    pub version_checks: Vec<VersionExpectation>,
    pub deferred_writes: Vec<DeferredResourceOp>,
    pub conflict_entries: Vec<EntryId>,
}

impl WorkResourcePlan {
    pub fn empty() -> Self {
        Self {
            read_views: Vec::new(),
            write_locks: Vec::new(),
            parallel_groups: Vec::new(),
            serial_groups: Vec::new(),
            parallelism_limit: 1,
            version_checks: Vec::new(),
            deferred_writes: Vec::new(),
            conflict_entries: Vec::new(),
        }
    }
}
