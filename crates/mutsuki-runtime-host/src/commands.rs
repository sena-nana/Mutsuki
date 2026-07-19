use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, ExportPlan, ObservabilityPage, PlanReceipt, ReadPlan, ResourceRef, RuntimeEvent,
    SnapshotDescriptor, StreamPlan, Task, TaskBatch, TaskHandle, TaskOutcome, TraceSpan, WritePlan,
};
use mutsuki_runtime_core::{ReloadDecision, RunnerLoopReport, RuntimeStatistics, RuntimeStopState};
use mutsuki_runtime_sdk::HostTaskSnapshot;

use crate::{
    AsyncExecutorSnapshot, HostRuntimeDriveState, PreparedRuntimeReload, WorkerPoolSnapshot,
};

#[derive(Clone, Debug, PartialEq)]
pub struct HostTaskState {
    pub handle: TaskHandle,
    pub status: Option<mutsuki_runtime_contracts::TaskStatus>,
    pub outcome: Option<TaskOutcome>,
}

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
    BeginDrain,
    Abort {
        reason: String,
    },
    StopState,
    Statistics,
    DriveState,
    WorkerPools,
    AsyncExecutor,
    TaskSnapshots,
    TaskStatesBatch(Vec<TaskHandle>),
    TaskOutcome(TaskHandle),
    EventsAfter {
        sequence: u64,
        limit: usize,
    },
    TraceSpansAfter {
        sequence: u64,
        limit: usize,
    },
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
    DrainStarted(RuntimeStopState),
    RuntimeAborted { cancelled_tasks: usize },
    StopState(RuntimeStopState),
    Statistics(RuntimeStatistics),
    DriveState(HostRuntimeDriveState),
    WorkerPools(Vec<WorkerPoolSnapshot>),
    AsyncExecutor(Option<AsyncExecutorSnapshot>),
    TaskSnapshots(Vec<HostTaskSnapshot>),
    TaskStatesBatch(Vec<HostTaskState>),
    TaskOutcome(Option<TaskOutcome>),
    Events(ObservabilityPage<RuntimeEvent>),
    TraceSpans(ObservabilityPage<TraceSpan>),
    ResourceDescriptor(ResourceRef),
    ResourceCreated(ResourceRef),
    ResourceBytes(Vec<u8>),
    Snapshot(SnapshotDescriptor),
    StreamPlan(StreamPlan),
    PlanReceipt(PlanReceipt),
    PlanReceipts(Vec<PlanReceipt>),
    Reloaded(ReloadDecision),
}
