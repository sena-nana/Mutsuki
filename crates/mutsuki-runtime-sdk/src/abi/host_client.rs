use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, ExportPlan, ReadPlan, TaskBatch, TaskHandle, TaskOutcome, WritePlan,
};
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use mutsuki_runtime_wire::{
    CancelTaskRequest, CollectReadPlanRequest, CommandBatchRequest, CommandPlanRequest,
    CommitWritePlanRequest, ExportPlanRequest, OpenStreamPlanRequest, SnapshotReadPlanRequest,
    SubmitTaskBatchRequest, TaskOutcomeRequest, WireRequest, decode_jsonl_response,
    encode_jsonl_request,
};

use crate::{ResourcePlanGateway, TaskSubmitter};

use super::error::{abi_failure, wire_failure};
use super::types::AbiHostV1;

#[derive(Clone, Copy)]
pub struct AbiHostClient {
    host: AbiHostV1,
}

impl AbiHostClient {
    pub fn new(host: AbiHostV1) -> Self {
        Self { host }
    }

    fn request<R: WireRequest>(&self, value: &R) -> RuntimeResult<R::Response> {
        let callback = self.host.request.ok_or_else(|| {
            abi_failure(
                "abi.host_callback_missing",
                "host request callback is unavailable",
            )
        })?;
        let request_id = 1;
        let bytes =
            encode_jsonl_request(request_id, value, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS)
                .map_err(wire_failure)?;
        let response = unsafe { callback(self.host.context, bytes.as_ptr(), bytes.len()) };
        let response_bytes = unsafe { response.payload.as_slice() }.to_vec();
        if let Some(release) = self.host.release {
            unsafe { release(response.payload) };
        }
        if response.status != 0 {
            return Err(abi_failure(
                "abi.host_callback_failed",
                String::from_utf8_lossy(&response_bytes),
            ));
        }
        decode_jsonl_response::<R>(
            &response_bytes,
            request_id,
            mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
        )
        .map_err(RuntimeFailure::new)
    }
}

impl TaskSubmitter for AbiHostClient {
    fn submit_batch(&self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        self.request(&SubmitTaskBatchRequest { batch })
    }

    fn cancel_task(&self, handle: &TaskHandle) -> RuntimeResult<()> {
        self.request(&CancelTaskRequest {
            handle: handle.clone(),
        })
    }

    fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        self.request(&TaskOutcomeRequest {
            handle: handle.clone(),
        })
    }
}

impl ResourcePlanGateway for AbiHostClient {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        self.request(&CollectReadPlanRequest {
            provider_id: None,
            plan: plan.clone(),
        })
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<mutsuki_runtime_contracts::SnapshotDescriptor> {
        self.request(&SnapshotReadPlanRequest {
            provider_id: None,
            plan: plan.clone(),
            kind_id: kind_id.into(),
            schema: schema.into(),
        })
    }

    fn open_stream_plan(
        &self,
        plan: &ReadPlan,
    ) -> RuntimeResult<mutsuki_runtime_contracts::StreamPlan> {
        self.request(&OpenStreamPlanRequest {
            provider_id: None,
            plan: plan.clone(),
        })
    }

    fn execute_export_plan(
        &self,
        plan: &ExportPlan,
    ) -> RuntimeResult<mutsuki_runtime_contracts::PlanReceipt> {
        self.request(&ExportPlanRequest {
            provider_id: None,
            plan: plan.clone(),
        })
    }

    fn commit_write_plan(
        &self,
        plan: &WritePlan,
        bytes: Vec<u8>,
    ) -> RuntimeResult<mutsuki_runtime_contracts::PlanReceipt> {
        self.request(&CommitWritePlanRequest {
            provider_id: None,
            plan: plan.clone(),
            bytes,
        })
    }

    fn execute_command_plan(
        &self,
        plan: &CommandPlan,
    ) -> RuntimeResult<mutsuki_runtime_contracts::PlanReceipt> {
        self.request(&CommandPlanRequest {
            provider_id: None,
            plan: plan.clone(),
        })
    }

    fn execute_command_batch(
        &self,
        batch: &CommandBatch,
    ) -> RuntimeResult<Vec<mutsuki_runtime_contracts::PlanReceipt>> {
        self.request(&CommandBatchRequest {
            provider_id: None,
            batch: batch.clone(),
        })
    }

    fn execute_saga_plan(
        &self,
        saga: &SagaPlan,
    ) -> RuntimeResult<Vec<mutsuki_runtime_contracts::PlanReceipt>> {
        self.request(&mutsuki_runtime_wire::SagaPlanRequest {
            provider_id: None,
            saga: saga.clone(),
        })
    }
}
