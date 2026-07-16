use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, ExportPlan, ReadPlan, TaskBatch, TaskHandle, TaskOutcome, WritePlan,
};
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use mutsuki_runtime_wire::{
    CancelTaskRequest, CollectReadPlanRequest, CommandBatchRequest, CommandPlanRequest,
    CommitWritePlanRequest, ExportPlanRequest, OpenStreamPlanRequest, SnapshotReadPlanRequest,
    SubmitTaskBatchRequest, TaskOutcomeRequest, WireRequest, decode_binary_response,
    encode_binary_request,
};

use crate::{ResourcePlanGateway, TaskSubmitter};

use super::error::{abi_failure, wire_failure};
use super::types::{AbiHostV2, consume_call_result};

#[derive(Clone, Copy)]
pub struct AbiHostClientV2 {
    host: AbiHostV2,
}

impl AbiHostClientV2 {
    pub fn new(host: AbiHostV2) -> Self {
        Self { host }
    }

    fn request<R: WireRequest>(&self, value: &R) -> RuntimeResult<R::Response> {
        let callback = self.host.request.ok_or_else(|| {
            abi_failure(
                "abi.v2.host_callback_missing",
                "host request callback is unavailable",
            )
        })?;
        let request_id = 1;
        let bytes =
            encode_binary_request(request_id, value, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS)
                .map_err(wire_failure)?;
        let response = unsafe { callback(self.host.context, bytes.as_ptr(), bytes.len()) };
        let (ok, response_bytes) =
            consume_call_result(response, self.host.release, "abi.v2.host_callback_contract")?;
        if !ok {
            return Err(abi_failure(
                "abi.v2.host_callback_failed",
                String::from_utf8_lossy(&response_bytes),
            ));
        }
        decode_binary_response::<R>(
            &response_bytes,
            request_id,
            mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
        )
        .map_err(RuntimeFailure::new)
    }
}

impl TaskSubmitter for AbiHostClientV2 {
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

impl ResourcePlanGateway for AbiHostClientV2 {
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
