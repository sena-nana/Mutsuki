use mutsuki_runtime_contracts::{
    CommandBatch, CommandPlan, ExportPlan, PlanReceipt, ReadPlan, ResourceRef, RuntimeEvent,
    SagaPlan, SnapshotDescriptor, StreamPlan, Task, TaskOutcome, TraceSpan, WritePlan,
};
use mutsuki_runtime_core::{ReloadDecision, RunnerLoopReport};
use mutsuki_runtime_sdk::HostTaskSnapshot;

use crate::PreparedRuntimeReload;

pub enum HostRuntimeCommand {
    SubmitTask(Box<Task>),
    TickOnce,
    RunUntilIdle {
        max_ticks: usize,
    },
    CancelTask(String),
    TaskSnapshots,
    TaskOutcome(String),
    EventsAfter(u64),
    TraceSpansAfter(usize),
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
    Reload {
        prepared: PreparedRuntimeReload,
        drain_timeout: std::time::Duration,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum HostRuntimeReply {
    TaskSubmitted(String),
    Tick(RunnerLoopReport),
    Idle(RunnerLoopReport),
    TaskCancelled(String),
    TaskSnapshots(Vec<HostTaskSnapshot>),
    TaskOutcome(Option<TaskOutcome>),
    Events(Vec<RuntimeEvent>),
    TraceSpans {
        next_index: usize,
        spans: Vec<TraceSpan>,
    },
    ResourceCreated(ResourceRef),
    ResourceBytes(Vec<u8>),
    Snapshot(SnapshotDescriptor),
    StreamPlan(StreamPlan),
    PlanReceipt(PlanReceipt),
    PlanReceipts(Vec<PlanReceipt>),
    Reloaded(ReloadDecision),
}
