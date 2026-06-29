use mutsuki_runtime_contracts::{
    CommandBatch, CommandPlan, ExportPlan, PlanReceipt, ReadPlan, ResourceRef, SagaPlan,
    SnapshotDescriptor, StreamPlan, Task, WritePlan,
};
use mutsuki_runtime_core::RunnerLoopReport;

pub enum HostRuntimeCommand {
    SubmitTask(Box<Task>),
    TickOnce,
    RunUntilIdle {
        max_ticks: usize,
    },
    CancelTask(String),
    CreateBlobResource {
        schema: String,
        bytes: Vec<u8>,
    },
    CreateCowStateResource {
        kind_id: String,
        schema: String,
        bytes: Vec<u8>,
    },
    CreateCapabilityResource {
        kind_id: String,
        schema: String,
    },
    CollectReadPlan(Box<ReadPlan>),
    SnapshotReadPlan {
        plan: Box<ReadPlan>,
        kind_id: String,
        schema: String,
    },
    OpenStreamPlan(Box<ReadPlan>),
    ExecuteExportPlan(Box<ExportPlan>),
    CommitWritePlan {
        plan: Box<WritePlan>,
        bytes: Vec<u8>,
    },
    ExecuteCommandPlan(Box<CommandPlan>),
    ExecuteCommandBatch(Box<CommandBatch>),
    ExecuteSagaPlan(Box<SagaPlan>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum HostRuntimeReply {
    TaskSubmitted(String),
    Tick(RunnerLoopReport),
    Idle(RunnerLoopReport),
    TaskCancelled(String),
    ResourceCreated(ResourceRef),
    ResourceBytes(Vec<u8>),
    Snapshot(SnapshotDescriptor),
    StreamPlan(StreamPlan),
    PlanReceipt(PlanReceipt),
    PlanReceipts(Vec<PlanReceipt>),
}
