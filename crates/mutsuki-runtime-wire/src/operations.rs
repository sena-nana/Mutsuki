use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, CompletionBatch, ExportPlan, PlanReceipt, ReadPlan, ResourceRef, RunnerContext,
    SnapshotDescriptor, StreamPlan, TaskBatch, TaskHandle, TaskOutcome, WorkBatch, WritePlan,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InitializeRequest {
    pub hello: ProtocolHello,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
}

impl WireRequest for InitializeRequest {
    const OPCODE: Opcode = Opcode::PluginInitialize;
    type Response = ProtocolHelloAck;
}
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

#[derive(Clone, Debug, PartialEq)]
pub enum AnyWireRequest {
    Initialize(InitializeRequest),
    RunBatch(Box<RunBatchRequest>),
    CancelRunner(CancelRunnerRequest),
    DisposeRunner(DisposeRunnerRequest),
    SubmitTaskBatch(SubmitTaskBatchRequest),
    CancelTask(CancelTaskRequest),
    TaskOutcome(TaskOutcomeRequest),
    CollectReadPlan(CollectReadPlanRequest),
    SnapshotReadPlan(SnapshotReadPlanRequest),
    OpenStreamPlan(OpenStreamPlanRequest),
    ExportPlan(ExportPlanRequest),
    CommitWritePlan(Box<CommitWritePlanRequest>),
    CommandPlan(CommandPlanRequest),
    CommandBatch(CommandBatchRequest),
    SagaPlan(SagaPlanRequest),
    CreateBlob(CreateBlobRequest),
    CreateCowState(CreateCowStateRequest),
    CreateCapability(CreateCapabilityRequest),
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecodedWireRequest {
    pub request_id: u64,
    pub request: AnyWireRequest,
}

impl AnyWireRequest {
    pub const fn opcode(&self) -> Opcode {
        match self {
            Self::Initialize(_) => Opcode::PluginInitialize,
            Self::RunBatch(_) => Opcode::RunnerRunBatch,
            Self::CancelRunner(_) => Opcode::RunnerCancel,
            Self::DisposeRunner(_) => Opcode::RunnerDispose,
            Self::SubmitTaskBatch(_) => Opcode::TaskSubmitBatch,
            Self::CancelTask(_) => Opcode::TaskCancel,
            Self::TaskOutcome(_) => Opcode::TaskOutcome,
            Self::CollectReadPlan(_) => Opcode::ResourceReadCollect,
            Self::SnapshotReadPlan(_) => Opcode::ResourceReadSnapshot,
            Self::OpenStreamPlan(_) => Opcode::ResourceStreamOpen,
            Self::ExportPlan(_) => Opcode::ResourceExport,
            Self::CommitWritePlan(_) => Opcode::ResourceWriteCommit,
            Self::CommandPlan(_) => Opcode::ResourceCommand,
            Self::CommandBatch(_) => Opcode::ResourceCommandBatch,
            Self::SagaPlan(_) => Opcode::ResourceSaga,
            Self::CreateBlob(_) => Opcode::ResourceCreateBlob,
            Self::CreateCowState(_) => Opcode::ResourceCreateCowState,
            Self::CreateCapability(_) => Opcode::ResourceCreateCapability,
        }
    }

    pub fn validate(&self, limits: WireLimits) -> Result<(), WireCodecError> {
        match self {
            Self::Initialize(request) => request.validate(limits),
            Self::RunBatch(request) => request.validate(limits),
            Self::CancelRunner(request) => request.validate(limits),
            Self::DisposeRunner(request) => request.validate(limits),
            Self::SubmitTaskBatch(request) => request.validate(limits),
            Self::CancelTask(request) => request.validate(limits),
            Self::TaskOutcome(request) => request.validate(limits),
            Self::CollectReadPlan(request) => request.validate(limits),
            Self::SnapshotReadPlan(request) => request.validate(limits),
            Self::OpenStreamPlan(request) => request.validate(limits),
            Self::ExportPlan(request) => request.validate(limits),
            Self::CommitWritePlan(request) => request.validate(limits),
            Self::CommandPlan(request) => request.validate(limits),
            Self::CommandBatch(request) => request.validate(limits),
            Self::SagaPlan(request) => request.validate(limits),
            Self::CreateBlob(request) => request.validate(limits),
            Self::CreateCowState(request) => request.validate(limits),
            Self::CreateCapability(request) => request.validate(limits),
        }
    }
}

macro_rules! decode_any_wire_request {
    ($opcode:expr, $decode:ident, $input:expr) => {
        match $opcode {
            Opcode::PluginInitialize => {
                $crate::AnyWireRequest::Initialize($decode::<$crate::InitializeRequest>($input)?)
            }
            Opcode::RunnerRunBatch => $crate::AnyWireRequest::RunBatch(Box::new($decode::<
                $crate::RunBatchRequest,
            >($input)?)),
            Opcode::RunnerCancel => $crate::AnyWireRequest::CancelRunner($decode::<
                $crate::CancelRunnerRequest,
            >($input)?),
            Opcode::RunnerDispose => $crate::AnyWireRequest::DisposeRunner($decode::<
                $crate::DisposeRunnerRequest,
            >($input)?),
            Opcode::TaskSubmitBatch => $crate::AnyWireRequest::SubmitTaskBatch($decode::<
                $crate::SubmitTaskBatchRequest,
            >($input)?),
            Opcode::TaskCancel => {
                $crate::AnyWireRequest::CancelTask($decode::<$crate::CancelTaskRequest>($input)?)
            }
            Opcode::TaskOutcome => {
                $crate::AnyWireRequest::TaskOutcome($decode::<$crate::TaskOutcomeRequest>($input)?)
            }
            Opcode::ResourceReadCollect => {
                $crate::AnyWireRequest::CollectReadPlan($decode::<$crate::CollectReadPlanRequest>(
                    $input,
                )?)
            }
            Opcode::ResourceReadSnapshot => {
                $crate::AnyWireRequest::SnapshotReadPlan(
                    $decode::<$crate::SnapshotReadPlanRequest>($input)?,
                )
            }
            Opcode::ResourceStreamOpen => $crate::AnyWireRequest::OpenStreamPlan($decode::<
                $crate::OpenStreamPlanRequest,
            >($input)?),
            Opcode::ResourceExport => {
                $crate::AnyWireRequest::ExportPlan($decode::<$crate::ExportPlanRequest>($input)?)
            }
            Opcode::ResourceWriteCommit => {
                $crate::AnyWireRequest::CommitWritePlan(Box::new($decode::<
                    $crate::CommitWritePlanRequest,
                >($input)?))
            }
            Opcode::ResourceCommand => {
                $crate::AnyWireRequest::CommandPlan($decode::<$crate::CommandPlanRequest>($input)?)
            }
            Opcode::ResourceCommandBatch => $crate::AnyWireRequest::CommandBatch($decode::<
                $crate::CommandBatchRequest,
            >($input)?),
            Opcode::ResourceSaga => {
                $crate::AnyWireRequest::SagaPlan($decode::<$crate::SagaPlanRequest>($input)?)
            }
            Opcode::ResourceCreateBlob => {
                $crate::AnyWireRequest::CreateBlob($decode::<$crate::CreateBlobRequest>($input)?)
            }
            Opcode::ResourceCreateCowState => {
                $crate::AnyWireRequest::CreateCowState($decode::<$crate::CreateCowStateRequest>(
                    $input,
                )?)
            }
            Opcode::ResourceCreateCapability => $crate::AnyWireRequest::CreateCapability(
                $decode::<$crate::CreateCapabilityRequest>($input)?,
            ),
        }
    };
}

pub(crate) use decode_any_wire_request;
