use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AgentRunBudget, ResourceRef};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleStatus {
    Active,
    Paused,
    Cancelled,
    Completed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleTrigger {
    OneShot {
        run_at_unix_ms: u64,
        timezone: String,
    },
    Recurring {
        cron: String,
        timezone: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until_unix_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_occurrences: Option<u32>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MisfirePolicy {
    FireOnce,
    Skip,
    CatchUpBounded { max_catch_up: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleRetryPolicy {
    None,
    Fixed { max_attempts: u32, delay_ms: u64 },
}

/// Neutral delivery handle. Products map this to QQ/webhook/etc. outside AgentKit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleTargetRef {
    pub target_id: String,
    pub kind: String,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSchedule {
    pub schedule_id: String,
    pub profile_id: String,
    pub profile_revision: u64,
    pub trigger: ScheduleTrigger,
    pub status: ScheduleStatus,
    #[serde(default)]
    pub input_ref: Option<ResourceRef>,
    #[serde(default)]
    pub context_refs: Vec<ResourceRef>,
    #[serde(default)]
    pub target: Option<ScheduleTargetRef>,
    #[serde(default)]
    pub budget: AgentRunBudget,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub allowed_plugins: Vec<String>,
    #[serde(default)]
    pub misfire: MisfirePolicy,
    #[serde(default)]
    pub retry: ScheduleRetryPolicy,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub occurrence_count: u32,
}

impl Default for MisfirePolicy {
    fn default() -> Self {
        Self::FireOnce
    }
}

impl Default for ScheduleRetryPolicy {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScheduledAgentRun {
    pub schedule_id: String,
    pub execution_id: String,
    pub epoch: u64,
    pub profile_id: String,
    pub profile_revision: u64,
    pub due_at_unix_ms: u64,
    #[serde(default)]
    pub input_ref: Option<ResourceRef>,
    #[serde(default)]
    pub context_refs: Vec<ResourceRef>,
    #[serde(default)]
    pub budget: AgentRunBudget,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleExecutionStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
    DuplicateSuppressed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScheduleExecution {
    pub execution_id: String,
    pub schedule_id: String,
    pub epoch: u64,
    pub attempt: u32,
    pub status: ScheduleExecutionStatus,
    pub due_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScheduledRunResult {
    pub schedule_id: String,
    pub execution_id: String,
    pub status: ScheduleExecutionStatus,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_ref: Option<ResourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ScheduleTargetRef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateScheduleRequest {
    pub profile_id: String,
    pub profile_revision: u64,
    pub trigger: ScheduleTrigger,
    #[serde(default)]
    pub input_ref: Option<ResourceRef>,
    #[serde(default)]
    pub context_refs: Vec<ResourceRef>,
    #[serde(default)]
    pub target: Option<ScheduleTargetRef>,
    #[serde(default)]
    pub budget: AgentRunBudget,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub allowed_plugins: Vec<String>,
    #[serde(default)]
    pub misfire: MisfirePolicy,
    #[serde(default)]
    pub retry: ScheduleRetryPolicy,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UpdateScheduleRequest {
    pub schedule_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<ScheduleTrigger>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_ref: Option<ResourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ScheduleTargetRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub misfire: Option<MisfirePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<ScheduleRetryPolicy>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleIdRequest {
    pub schedule_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListSchedulesRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ScheduleStatus>,
    #[serde(default = "default_list_limit")]
    pub limit: usize,
}

fn default_list_limit() -> usize {
    32
}

/// Contract for Host/Std Scheduler. AgentKit must not own a timer loop.
pub trait SchedulerService: Send + Sync {
    fn register_trigger(
        &self,
        schedule_id: &str,
        trigger: &ScheduleTrigger,
    ) -> Result<(), crate::AgentError>;

    fn update_trigger(
        &self,
        schedule_id: &str,
        trigger: &ScheduleTrigger,
    ) -> Result<(), crate::AgentError>;

    fn cancel_trigger(&self, schedule_id: &str) -> Result<(), crate::AgentError>;

    fn pause_trigger(&self, schedule_id: &str) -> Result<(), crate::AgentError>;

    fn resume_trigger(&self, schedule_id: &str) -> Result<(), crate::AgentError>;
}
