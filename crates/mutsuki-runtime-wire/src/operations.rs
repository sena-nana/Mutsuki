use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, CompletionBatch, ExportPlan, PlanReceipt, ReadPlan, ResourceRef, RunnerContext,
    SnapshotDescriptor, StreamPlan, TaskBatch, TaskHandle, TaskOutcome, WorkBatch, WritePlan,
};
use serde::{Deserialize, Serialize};

use crate::{Opcode, ProtocolHello, ProtocolHelloAck, WireCodecError, WireLimits, WireRequest};

macro_rules! wire_request {
    ($name:ident, $opcode:ident, $response:ty, { $($field:ident : $type:ty),* $(,)? }) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        pub struct $name { $(pub $field: $type),* }
        impl WireRequest for $name {
            const OPCODE: Opcode = Opcode::$opcode;
            type Response = $response;
        }
    };
}

wire_request!(InitializeRequest, PluginInitialize, ProtocolHelloAck, { hello: ProtocolHello });
wire_request!(RunBatchRequest, RunnerRunBatch, CompletionBatch, {
    runner_id: String,
    ctx: RunnerContext,
    batch: WorkBatch,
});
wire_request!(CancelRunnerRequest, RunnerCancel, (), {
    runner_id: String,
    invocation_id: String,
});
wire_request!(DisposeRunnerRequest, RunnerDispose, (), { runner_id: String });
wire_request!(SubmitTaskBatchRequest, TaskSubmitBatch, Vec<TaskHandle>, { batch: TaskBatch });
wire_request!(CancelTaskRequest, TaskCancel, (), { handle: TaskHandle });
wire_request!(TaskOutcomeRequest, TaskOutcome, Option<TaskOutcome>, { handle: TaskHandle });
wire_request!(CollectReadPlanRequest, ResourceReadCollect, Vec<u8>, {
    provider_id: Option<String>,
    plan: ReadPlan,
});
wire_request!(SnapshotReadPlanRequest, ResourceReadSnapshot, SnapshotDescriptor, {
    provider_id: Option<String>,
    plan: ReadPlan,
    kind_id: String,
    schema: String,
});
wire_request!(OpenStreamPlanRequest, ResourceStreamOpen, StreamPlan, {
    provider_id: Option<String>,
    plan: ReadPlan,
});
wire_request!(ExportPlanRequest, ResourceExport, PlanReceipt, {
    provider_id: Option<String>,
    plan: ExportPlan,
});
wire_request!(CommandPlanRequest, ResourceCommand, PlanReceipt, {
    provider_id: Option<String>,
    plan: CommandPlan,
});
wire_request!(CommandBatchRequest, ResourceCommandBatch, Vec<PlanReceipt>, {
    provider_id: Option<String>,
    batch: CommandBatch,
});
wire_request!(SagaPlanRequest, ResourceSaga, Vec<PlanReceipt>, {
    provider_id: Option<String>,
    saga: SagaPlan,
});
wire_request!(CreateCapabilityRequest, ResourceCreateCapability, ResourceRef, {
    provider_id: Option<String>,
    kind_id: String,
    schema: String,
});

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommitWritePlanRequest {
    pub provider_id: Option<String>,
    pub plan: WritePlan,
    pub bytes: Vec<u8>,
}

impl WireRequest for CommitWritePlanRequest {
    const OPCODE: Opcode = Opcode::ResourceWriteCommit;
    type Response = PlanReceipt;

    fn validate(&self, limits: WireLimits) -> Result<(), WireCodecError> {
        validate_inline_resource_bytes(self.bytes.len(), limits)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateBlobRequest {
    pub provider_id: Option<String>,
    pub schema: String,
    pub bytes: Vec<u8>,
}

impl WireRequest for CreateBlobRequest {
    const OPCODE: Opcode = Opcode::ResourceCreateBlob;
    type Response = ResourceRef;

    fn validate(&self, limits: WireLimits) -> Result<(), WireCodecError> {
        validate_inline_resource_bytes(self.bytes.len(), limits)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateCowStateRequest {
    pub provider_id: Option<String>,
    pub kind_id: String,
    pub schema: String,
    pub bytes: Vec<u8>,
}

impl WireRequest for CreateCowStateRequest {
    const OPCODE: Opcode = Opcode::ResourceCreateCowState;
    type Response = ResourceRef;

    fn validate(&self, limits: WireLimits) -> Result<(), WireCodecError> {
        validate_inline_resource_bytes(self.bytes.len(), limits)
    }
}

fn validate_inline_resource_bytes(length: usize, limits: WireLimits) -> Result<(), WireCodecError> {
    if length > limits.max_inline_resource_bytes {
        return Err(WireCodecError::InlineResourceOversized {
            actual: length,
            limit: limits.max_inline_resource_bytes,
        });
    }
    Ok(())
}
