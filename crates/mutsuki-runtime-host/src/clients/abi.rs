use std::io::{BufRead, Write};

use mutsuki_runtime_contracts::{
    CommandBatch, CommandPlan, ExportPlan, PlanReceipt, ReadPlan, SagaPlan, SnapshotDescriptor,
    StreamPlan, TaskBatch, TaskHandle, TaskOutcome, WritePlan,
};
use mutsuki_runtime_core::RuntimeResult;
use mutsuki_runtime_sdk::{ResourcePlanGateway, TaskSubmitter};
use serde_json::json;

use crate::jsonl::JsonlBridge;

pub struct AbiTaskClient<R, W> {
    bridge: JsonlBridge<R, W>,
}

impl<R, W> AbiTaskClient<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            bridge: JsonlBridge::new(reader, writer),
        }
    }

    pub fn into_inner(self) -> (R, W) {
        self.bridge.into_inner()
    }
}

impl<R, W> TaskSubmitter for AbiTaskClient<R, W>
where
    R: BufRead + Send,
    W: Write + Send,
{
    fn submit_batch(&self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        self.bridge
            .request_as("task.submit_batch", json!({ "batch": batch }))
    }

    fn cancel_task(&self, handle: &TaskHandle) -> RuntimeResult<()> {
        self.bridge
            .request("task.cancel", json!({ "handle": handle }))?;
        Ok(())
    }

    fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        self.bridge
            .request_as("task.outcome", json!({ "handle": handle }))
    }
}

pub struct AbiResourceClient<R, W> {
    bridge: JsonlBridge<R, W>,
}

impl<R, W> AbiResourceClient<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            bridge: JsonlBridge::new(reader, writer),
        }
    }

    pub fn into_inner(self) -> (R, W) {
        self.bridge.into_inner()
    }
}

impl<R, W> ResourcePlanGateway for AbiResourceClient<R, W>
where
    R: BufRead + Send,
    W: Write + Send,
{
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        self.bridge
            .request_as("resource.read.collect", json!({ "plan": plan }))
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        self.bridge.request_as(
            "resource.read.snapshot",
            json!({ "plan": plan, "kind_id": kind_id, "schema": schema }),
        )
    }

    fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        self.bridge
            .request_as("resource.stream.open", json!({ "plan": plan }))
    }

    fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        self.bridge
            .request_as("resource.export", json!({ "plan": plan }))
    }

    fn commit_write_plan(&self, plan: &WritePlan, bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        self.bridge.request_as(
            "resource.write.commit",
            json!({ "plan": plan, "bytes": bytes }),
        )
    }

    fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        self.bridge
            .request_as("resource.command", json!({ "plan": plan }))
    }

    fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        self.bridge
            .request_as("resource.command_batch", json!({ "batch": batch }))
    }

    fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        self.bridge
            .request_as("resource.saga", json!({ "saga": saga }))
    }
}
