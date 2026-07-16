use std::io::{BufRead, Write};

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, ExportPlan, PlanReceipt, ReadPlan, SnapshotDescriptor, StreamPlan, TaskBatch,
    TaskHandle, TaskOutcome, WritePlan,
};
use mutsuki_runtime_core::RuntimeResult;
use mutsuki_runtime_sdk::{ResourcePlanGateway, TaskSubmitter};
use mutsuki_runtime_wire::{
    CancelTaskRequest, CollectReadPlanRequest, CommandBatchRequest, CommandPlanRequest,
    CommitWritePlanRequest, ExportPlanRequest, OpenStreamPlanRequest, SagaPlanRequest,
    SnapshotReadPlanRequest, SubmitTaskBatchRequest, TaskOutcomeRequest,
};

use crate::JsonlTransport;

pub struct AbiTaskClient<R, W> {
    transport: JsonlTransport<R, W>,
}

impl<R, W> AbiTaskClient<R, W>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            transport: JsonlTransport::new(reader, writer),
        }
    }

    pub fn into_inner(self) -> (R, W) {
        self.transport.into_inner()
    }
}

impl<R, W> TaskSubmitter for AbiTaskClient<R, W>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    fn submit_batch(&self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        self.transport.request(&SubmitTaskBatchRequest { batch })
    }

    fn cancel_task(&self, handle: &TaskHandle) -> RuntimeResult<()> {
        self.transport.request(&CancelTaskRequest {
            handle: handle.clone(),
        })
    }

    fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        self.transport.request(&TaskOutcomeRequest {
            handle: handle.clone(),
        })
    }
}

pub struct AbiResourceClient<R, W> {
    transport: JsonlTransport<R, W>,
}

impl<R, W> AbiResourceClient<R, W>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            transport: JsonlTransport::new(reader, writer),
        }
    }

    pub fn into_inner(self) -> (R, W) {
        self.transport.into_inner()
    }
}

impl<R, W> ResourcePlanGateway for AbiResourceClient<R, W>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        self.transport.request(&CollectReadPlanRequest {
            provider_id: None,
            plan: plan.clone(),
        })
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        self.transport.request(&SnapshotReadPlanRequest {
            provider_id: None,
            plan: plan.clone(),
            kind_id: kind_id.into(),
            schema: schema.into(),
        })
    }

    fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        self.transport.request(&OpenStreamPlanRequest {
            provider_id: None,
            plan: plan.clone(),
        })
    }

    fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        self.transport.request(&ExportPlanRequest {
            provider_id: None,
            plan: plan.clone(),
        })
    }

    fn commit_write_plan(&self, plan: &WritePlan, bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        self.transport.request(&CommitWritePlanRequest {
            provider_id: None,
            plan: plan.clone(),
            bytes,
        })
    }

    fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        self.transport.request(&CommandPlanRequest {
            provider_id: None,
            plan: plan.clone(),
        })
    }

    fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        self.transport.request(&CommandBatchRequest {
            provider_id: None,
            batch: batch.clone(),
        })
    }

    fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        self.transport.request(&SagaPlanRequest {
            provider_id: None,
            saga: saga.clone(),
        })
    }
}
