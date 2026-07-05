use serde::{Deserialize, Serialize};

use crate::{BatchId, EntryId, RunnerResult, RuntimeError, ScalarValue, TaskId, TickId, WorkBatch};

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

    pub fn from_error(batch: &WorkBatch, error: RuntimeError) -> Self {
        Self::from_results(
            batch,
            batch
                .entries
                .iter()
                .map(|entry| EntryCompletion {
                    entry_id: entry.entry_id.clone(),
                    task_id: entry.task_id.clone(),
                    result: None,
                    error: Some(error.clone()),
                })
                .collect(),
        )
    }
}
