use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    CompletionBatch, EntryCompletion, ExecutionClass, RunnerContext, RunnerDescriptor,
    RunnerPurity, RunnerResult, WorkBatch,
};

use crate::RuntimeResult;

pub trait Runner: Send {
    fn descriptor(&self) -> &RunnerDescriptor;

    fn run_batch(&mut self, ctx: RunnerContext, batch: WorkBatch)
    -> RuntimeResult<CompletionBatch>;

    fn cancel(&mut self, _invocation_id: &str) -> RuntimeResult<()> {
        Ok(())
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunnerLoopReport {
    pub claimed_tasks: usize,
    pub completed_tasks: usize,
}

pub struct CoreKernelRunner {
    descriptor: RunnerDescriptor,
}

impl CoreKernelRunner {
    pub fn new(plugin_generation: u64) -> Self {
        Self {
            descriptor: RunnerDescriptor {
                runner_id: "core.kernel".into(),
                plugin_id: "core".into(),
                plugin_generation,
                accepted_protocol_ids: vec!["core.commit".into(), "core.event.append".into()],
                purity: RunnerPurity::Committer,
                execution_class: ExecutionClass::Control,
                input_schema: serde_json::json!({}),
                output_schema: serde_json::json!({}),
                batch: Default::default(),
                payload: Default::default(),
                resources: Default::default(),
                ordering: Default::default(),
                control: Default::default(),
                metadata: BTreeMap::new(),
                contract_surfaces: vec!["runner:core.kernel".into()],
            },
        }
    }
}

impl Runner for CoreKernelRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    /// Marker runner used to authorize core.* tasks. Actual kernel task
    /// handling is performed by CoreRuntime before runner dispatch.
    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        let results = batch
            .entries
            .iter()
            .map(|entry| EntryCompletion {
                entry_id: entry.entry_id.clone(),
                task_id: entry.task_id.clone(),
                result: Some(RunnerResult::completed(entry.task_id.clone())),
                error: None,
            })
            .collect();
        Ok(CompletionBatch::from_results(&batch, results))
    }
}
