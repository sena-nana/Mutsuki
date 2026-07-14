use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, ExportPlan, PlanReceipt, ReadPlan, ResourceRef, RuntimeEvent, SnapshotDescriptor,
    StreamPlan, Task, TaskBatch, TaskHandle, TaskOutcome, TraceSpan, WritePlan,
};
use mutsuki_runtime_core::{ReloadDecision, RunnerLoopReport};
use mutsuki_runtime_sdk::HostTaskSnapshot;

use crate::PreparedRuntimeReload;

// Variant boxing is part of this public control-plane API and must not drift for a lint.
#[allow(clippy::large_enum_variant)]
pub enum HostRuntimeCommand {
    SubmitTask(Box<Task>),
    SubmitBatch(Box<TaskBatch>),
    TickOnce,
    RunUntilIdle {
        max_ticks: usize,
    },
    CancelTask(TaskHandle),
    TaskSnapshots,
    TaskOutcome(TaskHandle),
    EventsAfter(u64),
    TraceSpansAfter(usize),
    OpenResourceDescriptor(String),
    CreateBlobResource {
        provider_id: String,
        schema: String,
        bytes: Vec<u8>,
    },
    CreateCowStateResource {
        provider_id: String,
        kind_id: String,
        schema: String,
        bytes: Vec<u8>,
    },
    CreateCapabilityResource {
        provider_id: String,
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
// Keep reply payloads directly matchable without changing the public API to boxed variants.
#[allow(clippy::large_enum_variant)]
pub enum HostRuntimeReply {
    TaskSubmitted(TaskHandle),
    TaskBatchSubmitted(Vec<TaskHandle>),
    Tick(RunnerLoopReport),
    Idle(RunnerLoopReport),
    TaskCancelled(TaskHandle),
    TaskSnapshots(Vec<HostTaskSnapshot>),
    TaskOutcome(Option<TaskOutcome>),
    Events(Vec<RuntimeEvent>),
    TraceSpans {
        next_index: usize,
        spans: Vec<TraceSpan>,
    },
    ResourceDescriptor(ResourceRef),
    ResourceCreated(ResourceRef),
    ResourceBytes(Vec<u8>),
    Snapshot(SnapshotDescriptor),
    StreamPlan(StreamPlan),
    PlanReceipt(PlanReceipt),
    PlanReceipts(Vec<PlanReceipt>),
    Reloaded(ReloadDecision),
}
