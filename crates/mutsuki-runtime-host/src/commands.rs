use mutsuki_runtime_contracts::{
    CommandBatch, CommandPlan, ExportPlan, PlanReceipt, ReadPlan, ResourceRef, RuntimeEvent,
    SagaPlan, SnapshotDescriptor, StreamPlan, Task, TaskOutcome, TaskStatus, TraceSpan, WritePlan,
};
use mutsuki_runtime_core::{ReloadDecision, RunnerLoopReport};

use crate::PreparedRuntimeReload;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostTaskFailureSummary {
    pub code: String,
    pub source: String,
    pub route: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HostTaskSnapshot {
    pub task_id: String,
    pub protocol_id: String,
    pub status: TaskStatus,
    pub priority: i64,
    pub ready_at_step: Option<u64>,
    pub created_sequence: u64,
    pub registry_generation: u64,
    pub target_binding_id: Option<String>,
    pub runner_hint: Option<String>,
    pub claimed_by: Option<String>,
    pub owner_runner: Option<String>,
    pub lease_id: Option<String>,
    pub trace_id: Option<String>,
    pub correlation_id: Option<String>,
    pub input_refs: Vec<String>,
    pub output_ref: Option<String>,
    pub continuation_ref: Option<String>,
    pub required_surfaces: Vec<String>,
    pub failure: Option<HostTaskFailureSummary>,
}

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
