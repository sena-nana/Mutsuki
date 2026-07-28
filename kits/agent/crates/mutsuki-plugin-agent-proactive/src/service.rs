//! Proactive schedule service. Timer loops stay in Host SchedulerService.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mutsuki_agent_contracts::{
    AgentError, AgentResult, AgentSchedule, CreateScheduleRequest, ListSchedulesRequest,
    MisfirePolicy, ScheduleExecution, ScheduleExecutionStatus, ScheduleIdRequest, ScheduleStatus,
    ScheduleTrigger, ScheduledAgentRun, ScheduledRunResult, SchedulerService,
    UpdateScheduleRequest,
};

const DEFAULT_MAX_SCHEDULES: usize = 1_024;
const MIN_RECURRING_INTERVAL_MS: u64 = 60_000;

#[derive(Clone)]
pub struct ProactiveScheduleService {
    inner: Arc<ProactiveInner>,
}

struct ProactiveInner {
    next_id: AtomicU64,
    schedules: Mutex<BTreeMap<String, AgentSchedule>>,
    executions: Mutex<BTreeMap<String, Vec<ScheduleExecution>>>,
    completed_epochs: Mutex<BTreeMap<String, u64>>,
    scheduler: Arc<dyn SchedulerService>,
    max_schedules: usize,
    max_recurring_hz_guard_ms: u64,
}

impl ProactiveScheduleService {
    pub fn new(scheduler: Arc<dyn SchedulerService>) -> Self {
        Self::with_limits(scheduler, DEFAULT_MAX_SCHEDULES, MIN_RECURRING_INTERVAL_MS)
    }

    pub fn with_limits(
        scheduler: Arc<dyn SchedulerService>,
        max_schedules: usize,
        max_recurring_hz_guard_ms: u64,
    ) -> Self {
        Self {
            inner: Arc::new(ProactiveInner {
                next_id: AtomicU64::new(1),
                schedules: Mutex::new(BTreeMap::new()),
                executions: Mutex::new(BTreeMap::new()),
                completed_epochs: Mutex::new(BTreeMap::new()),
                scheduler,
                max_schedules: max_schedules.max(1),
                max_recurring_hz_guard_ms: max_recurring_hz_guard_ms.max(1_000),
            }),
        }
    }

    pub fn create(
        &self,
        request: CreateScheduleRequest,
        now_unix_ms: u64,
    ) -> AgentResult<AgentSchedule> {
        validate_create(&request, self.inner.max_recurring_hz_guard_ms)?;
        let mut schedules = self.inner.schedules.lock().expect("schedule mutex");
        if schedules.len() >= self.inner.max_schedules {
            return Err(AgentError::invalid_input(
                "schedule count exceeds configured maximum",
            ));
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let schedule_id = format!("agent-schedule-{id}");
        let next_run_at_unix_ms = next_run_at(&request.trigger, now_unix_ms)?;
        let schedule = AgentSchedule {
            schedule_id: schedule_id.clone(),
            profile_id: request.profile_id,
            profile_revision: request.profile_revision,
            trigger: request.trigger.clone(),
            status: ScheduleStatus::Active,
            input_ref: request.input_ref,
            context_refs: request.context_refs,
            target: request.target,
            budget: request.budget,
            allowed_tools: request.allowed_tools,
            allowed_plugins: request.allowed_plugins,
            misfire: request.misfire,
            retry: request.retry,
            created_at_unix_ms: now_unix_ms,
            updated_at_unix_ms: now_unix_ms,
            next_run_at_unix_ms: Some(next_run_at_unix_ms),
            occurrence_count: 0,
        };
        self.inner
            .scheduler
            .register_trigger(&schedule_id, &request.trigger)?;
        schedules.insert(schedule_id, schedule.clone());
        Ok(schedule)
    }

    pub fn list(&self, request: ListSchedulesRequest) -> AgentResult<Vec<AgentSchedule>> {
        let mut values: Vec<_> = self
            .inner
            .schedules
            .lock()
            .expect("schedule mutex")
            .values()
            .filter(|schedule| {
                request
                    .status
                    .as_ref()
                    .is_none_or(|status| schedule.status == *status)
            })
            .cloned()
            .collect();
        values.sort_by(|left, right| left.schedule_id.cmp(&right.schedule_id));
        values.truncate(request.limit.max(1));
        Ok(values)
    }

    pub fn get(&self, request: ScheduleIdRequest) -> AgentResult<AgentSchedule> {
        self.schedule(&request.schedule_id)
    }

    pub fn update(
        &self,
        request: UpdateScheduleRequest,
        now_unix_ms: u64,
    ) -> AgentResult<AgentSchedule> {
        let mut schedules = self.inner.schedules.lock().expect("schedule mutex");
        let schedule = schedules.get_mut(&request.schedule_id).ok_or_else(|| {
            AgentError::not_found(format!("schedule `{}` not found", request.schedule_id))
        })?;
        if matches!(
            schedule.status,
            ScheduleStatus::Cancelled | ScheduleStatus::Completed
        ) {
            return Err(AgentError::invalid_input(
                "cancelled or completed schedules cannot be updated",
            ));
        }
        if let Some(trigger) = request.trigger {
            validate_trigger(&trigger, self.inner.max_recurring_hz_guard_ms)?;
            self.inner
                .scheduler
                .update_trigger(&schedule.schedule_id, &trigger)?;
            schedule.next_run_at_unix_ms = Some(next_run_at(&trigger, now_unix_ms)?);
            schedule.trigger = trigger;
        }
        if let Some(input_ref) = request.input_ref {
            schedule.input_ref = Some(input_ref);
        }
        if let Some(target) = request.target {
            schedule.target = Some(target);
        }
        if let Some(misfire) = request.misfire {
            schedule.misfire = misfire;
        }
        if let Some(retry) = request.retry {
            schedule.retry = retry;
        }
        schedule.updated_at_unix_ms = now_unix_ms;
        Ok(schedule.clone())
    }

    pub fn pause(
        &self,
        request: ScheduleIdRequest,
        now_unix_ms: u64,
    ) -> AgentResult<AgentSchedule> {
        self.set_status(&request.schedule_id, ScheduleStatus::Paused, now_unix_ms)?;
        self.inner.scheduler.pause_trigger(&request.schedule_id)?;
        self.schedule(&request.schedule_id)
    }

    pub fn resume(
        &self,
        request: ScheduleIdRequest,
        now_unix_ms: u64,
    ) -> AgentResult<AgentSchedule> {
        self.set_status(&request.schedule_id, ScheduleStatus::Active, now_unix_ms)?;
        self.inner.scheduler.resume_trigger(&request.schedule_id)?;
        self.schedule(&request.schedule_id)
    }

    pub fn cancel(
        &self,
        request: ScheduleIdRequest,
        now_unix_ms: u64,
    ) -> AgentResult<AgentSchedule> {
        self.set_status(&request.schedule_id, ScheduleStatus::Cancelled, now_unix_ms)?;
        self.inner.scheduler.cancel_trigger(&request.schedule_id)?;
        self.schedule(&request.schedule_id)
    }

    pub fn history(&self, request: ScheduleIdRequest) -> AgentResult<Vec<ScheduleExecution>> {
        Ok(self
            .inner
            .executions
            .lock()
            .expect("schedule mutex")
            .get(&request.schedule_id)
            .cloned()
            .unwrap_or_default())
    }

    /// Convert a due scheduler event into a durable run request with epoch fencing.
    pub fn due(
        &self,
        schedule_id: &str,
        due_at_unix_ms: u64,
        epoch: u64,
    ) -> AgentResult<Option<ScheduledAgentRun>> {
        let mut schedules = self.inner.schedules.lock().expect("schedule mutex");
        let schedule = schedules
            .get_mut(schedule_id)
            .ok_or_else(|| AgentError::not_found(format!("schedule `{schedule_id}` not found")))?;
        if schedule.status != ScheduleStatus::Active {
            return Ok(None);
        }
        let completed = self
            .inner
            .completed_epochs
            .lock()
            .expect("schedule mutex")
            .get(schedule_id)
            .copied()
            .unwrap_or(0);
        if epoch <= completed {
            self.record_execution(
                schedule_id,
                epoch,
                due_at_unix_ms,
                ScheduleExecutionStatus::DuplicateSuppressed,
                None,
            );
            return Ok(None);
        }
        {
            let mut executions = self.inner.executions.lock().expect("schedule mutex");
            let entries = executions.entry(schedule_id.to_owned()).or_default();
            if entries.iter().any(|entry| {
                entry.epoch == epoch
                    && !matches!(
                        entry.status,
                        ScheduleExecutionStatus::Failed | ScheduleExecutionStatus::Skipped
                    )
            }) {
                entries.push(ScheduleExecution {
                    execution_id: format!("{schedule_id}:exec:{epoch}:dup"),
                    schedule_id: schedule_id.to_owned(),
                    epoch,
                    attempt: 1,
                    status: ScheduleExecutionStatus::DuplicateSuppressed,
                    due_at_unix_ms,
                    started_at_unix_ms: None,
                    finished_at_unix_ms: None,
                    error_code: None,
                });
                return Ok(None);
            }
        }
        if let Some(next) = schedule.next_run_at_unix_ms
            && due_at_unix_ms + 1 < next
        {
            match schedule.misfire {
                MisfirePolicy::Skip => {
                    self.record_execution(
                        schedule_id,
                        epoch,
                        due_at_unix_ms,
                        ScheduleExecutionStatus::Skipped,
                        None,
                    );
                    return Ok(None);
                }
                MisfirePolicy::FireOnce | MisfirePolicy::CatchUpBounded { .. } => {}
            }
        }
        let execution_id = format!("{schedule_id}:exec:{epoch}");
        self.record_execution(
            schedule_id,
            epoch,
            due_at_unix_ms,
            ScheduleExecutionStatus::Pending,
            Some(&execution_id),
        );
        let run = ScheduledAgentRun {
            schedule_id: schedule_id.to_owned(),
            execution_id,
            epoch,
            profile_id: schedule.profile_id.clone(),
            profile_revision: schedule.profile_revision,
            due_at_unix_ms,
            input_ref: schedule.input_ref.clone(),
            context_refs: schedule.context_refs.clone(),
            budget: schedule.budget.clone(),
        };
        Ok(Some(run))
    }

    pub fn complete_execution(
        &self,
        result: ScheduledRunResult,
        now_unix_ms: u64,
    ) -> AgentResult<()> {
        let mut schedules = self.inner.schedules.lock().expect("schedule mutex");
        let schedule = schedules
            .get_mut(&result.schedule_id)
            .ok_or_else(|| AgentError::not_found("schedule was not found"))?;
        let epoch = result
            .execution_id
            .rsplit(':')
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        self.inner
            .completed_epochs
            .lock()
            .expect("schedule mutex")
            .insert(result.schedule_id.clone(), epoch);
        if let Some(entries) = self
            .inner
            .executions
            .lock()
            .expect("schedule mutex")
            .get_mut(&result.schedule_id)
        {
            if let Some(entry) = entries
                .iter_mut()
                .rev()
                .find(|entry| entry.execution_id == result.execution_id)
            {
                entry.status = result.status.clone();
                entry.finished_at_unix_ms = Some(now_unix_ms);
            }
        }
        schedule.occurrence_count = schedule.occurrence_count.saturating_add(1);
        schedule.updated_at_unix_ms = now_unix_ms;
        match &schedule.trigger {
            ScheduleTrigger::OneShot { .. } => {
                schedule.status = ScheduleStatus::Completed;
                schedule.next_run_at_unix_ms = None;
            }
            ScheduleTrigger::Recurring {
                until_unix_ms,
                max_occurrences,
                ..
            } => {
                if max_occurrences.is_some_and(|max| schedule.occurrence_count >= max)
                    || until_unix_ms.is_some_and(|until| now_unix_ms >= until)
                {
                    schedule.status = ScheduleStatus::Completed;
                    schedule.next_run_at_unix_ms = None;
                } else {
                    schedule.next_run_at_unix_ms =
                        Some(next_run_at(&schedule.trigger, now_unix_ms)?);
                }
            }
        }
        Ok(())
    }

    pub fn snapshot(&self) -> Vec<AgentSchedule> {
        self.inner
            .schedules
            .lock()
            .expect("schedule mutex")
            .values()
            .cloned()
            .collect()
    }

    pub fn restore(&self, schedules: Vec<AgentSchedule>) -> AgentResult<()> {
        let mut guard = self.inner.schedules.lock().expect("schedule mutex");
        guard.clear();
        for schedule in schedules {
            if schedule.status == ScheduleStatus::Active {
                self.inner
                    .scheduler
                    .register_trigger(&schedule.schedule_id, &schedule.trigger)?;
            }
            guard.insert(schedule.schedule_id.clone(), schedule);
        }
        Ok(())
    }

    fn schedule(&self, schedule_id: &str) -> AgentResult<AgentSchedule> {
        self.inner
            .schedules
            .lock()
            .expect("schedule mutex")
            .get(schedule_id)
            .cloned()
            .ok_or_else(|| AgentError::not_found(format!("schedule `{schedule_id}` not found")))
    }

    fn set_status(
        &self,
        schedule_id: &str,
        status: ScheduleStatus,
        now_unix_ms: u64,
    ) -> AgentResult<()> {
        let mut schedules = self.inner.schedules.lock().expect("schedule mutex");
        let schedule = schedules
            .get_mut(schedule_id)
            .ok_or_else(|| AgentError::not_found(format!("schedule `{schedule_id}` not found")))?;
        schedule.status = status;
        schedule.updated_at_unix_ms = now_unix_ms;
        Ok(())
    }

    fn record_execution(
        &self,
        schedule_id: &str,
        epoch: u64,
        due_at_unix_ms: u64,
        status: ScheduleExecutionStatus,
        execution_id: Option<&str>,
    ) {
        let execution = ScheduleExecution {
            execution_id: execution_id
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{schedule_id}:exec:{epoch}")),
            schedule_id: schedule_id.to_owned(),
            epoch,
            attempt: 1,
            status,
            due_at_unix_ms,
            started_at_unix_ms: None,
            finished_at_unix_ms: None,
            error_code: None,
        };
        self.inner
            .executions
            .lock()
            .expect("schedule mutex")
            .entry(schedule_id.to_owned())
            .or_default()
            .push(execution);
    }
}

/// In-memory SchedulerService used by tests. Not a production timer loop.
#[derive(Clone, Default)]
pub struct InMemorySchedulerService {
    inner: Arc<Mutex<BTreeMap<String, ScheduleTrigger>>>,
}

impl SchedulerService for InMemorySchedulerService {
    fn register_trigger(
        &self,
        schedule_id: &str,
        trigger: &ScheduleTrigger,
    ) -> Result<(), AgentError> {
        self.inner
            .lock()
            .expect("scheduler mutex")
            .insert(schedule_id.to_owned(), trigger.clone());
        Ok(())
    }

    fn update_trigger(
        &self,
        schedule_id: &str,
        trigger: &ScheduleTrigger,
    ) -> Result<(), AgentError> {
        let mut guard = self.inner.lock().expect("scheduler mutex");
        if !guard.contains_key(schedule_id) {
            return Err(AgentError::not_found("scheduler trigger was not found"));
        }
        guard.insert(schedule_id.to_owned(), trigger.clone());
        Ok(())
    }

    fn cancel_trigger(&self, schedule_id: &str) -> Result<(), AgentError> {
        self.inner
            .lock()
            .expect("scheduler mutex")
            .remove(schedule_id);
        Ok(())
    }

    fn pause_trigger(&self, _schedule_id: &str) -> Result<(), AgentError> {
        Ok(())
    }

    fn resume_trigger(&self, _schedule_id: &str) -> Result<(), AgentError> {
        Ok(())
    }
}

fn validate_create(request: &CreateScheduleRequest, min_interval_ms: u64) -> AgentResult<()> {
    if request.profile_id.trim().is_empty() || request.profile_revision == 0 {
        return Err(AgentError::invalid_input(
            "schedule profile_id and non-zero profile_revision are required",
        ));
    }
    validate_trigger(&request.trigger, min_interval_ms)
}

fn validate_trigger(trigger: &ScheduleTrigger, min_interval_ms: u64) -> AgentResult<()> {
    match trigger {
        ScheduleTrigger::OneShot {
            run_at_unix_ms,
            timezone,
        } => {
            if *run_at_unix_ms == 0 || timezone.trim().is_empty() {
                return Err(AgentError::invalid_input(
                    "one-shot trigger requires absolute time and timezone",
                ));
            }
        }
        ScheduleTrigger::Recurring {
            cron,
            timezone,
            max_occurrences,
            ..
        } => {
            if cron.trim().is_empty() || timezone.trim().is_empty() {
                return Err(AgentError::invalid_input(
                    "recurring trigger requires cron and timezone",
                ));
            }
            if cron_interval_hint(cron).is_some_and(|interval| interval < min_interval_ms) {
                return Err(AgentError::invalid_input(
                    "recurring schedule interval is below the configured minimum",
                ));
            }
            if max_occurrences == &Some(0) {
                return Err(AgentError::invalid_input(
                    "max_occurrences must be positive when set",
                ));
            }
        }
    }
    Ok(())
}

fn cron_interval_hint(cron: &str) -> Option<u64> {
    // Accept `every_ms:<n>` for deterministic tests; real cron parsing stays in Host Scheduler.
    cron.strip_prefix("every_ms:")
        .and_then(|value| value.parse::<u64>().ok())
}

fn next_run_at(trigger: &ScheduleTrigger, now_unix_ms: u64) -> AgentResult<u64> {
    match trigger {
        ScheduleTrigger::OneShot { run_at_unix_ms, .. } => Ok(*run_at_unix_ms),
        ScheduleTrigger::Recurring { cron, .. } => {
            let interval = cron_interval_hint(cron).unwrap_or(MIN_RECURRING_INTERVAL_MS);
            Ok(now_unix_ms.saturating_add(interval))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_agent_contracts::{AgentRunBudget, ScheduleTargetRef};
    use serde_json::json;

    fn service() -> ProactiveScheduleService {
        ProactiveScheduleService::new(Arc::new(InMemorySchedulerService::default()))
    }

    fn one_shot(at: u64) -> CreateScheduleRequest {
        CreateScheduleRequest {
            profile_id: "profile.proactive".into(),
            profile_revision: 3,
            trigger: ScheduleTrigger::OneShot {
                run_at_unix_ms: at,
                timezone: "UTC".into(),
            },
            input_ref: None,
            context_refs: Vec::new(),
            target: Some(ScheduleTargetRef {
                target_id: "delivery-1".into(),
                kind: "callback".into(),
                metadata: json!({}),
            }),
            budget: AgentRunBudget::default(),
            allowed_tools: vec!["read".into()],
            allowed_plugins: Vec::new(),
            misfire: MisfirePolicy::FireOnce,
            retry: Default::default(),
        }
    }

    #[test]
    fn one_shot_and_recurring_e2e_with_fencing() {
        let service = service();
        let created = service.create(one_shot(1_000), 100).unwrap();
        assert_eq!(created.status, ScheduleStatus::Active);
        assert_eq!(created.next_run_at_unix_ms, Some(1_000));

        let run = service
            .due(&created.schedule_id, 1_000, 1)
            .unwrap()
            .unwrap();
        assert_eq!(run.profile_revision, 3);
        assert!(
            service
                .due(&created.schedule_id, 1_000, 1)
                .unwrap()
                .is_none(),
            "duplicate epoch must be suppressed"
        );
        service
            .complete_execution(
                ScheduledRunResult {
                    schedule_id: created.schedule_id.clone(),
                    execution_id: run.execution_id,
                    status: ScheduleExecutionStatus::Succeeded,
                    summary: "done".into(),
                    output_ref: None,
                    target: created.target.clone(),
                },
                1_100,
            )
            .unwrap();
        assert_eq!(
            service
                .get(ScheduleIdRequest {
                    schedule_id: created.schedule_id.clone()
                })
                .unwrap()
                .status,
            ScheduleStatus::Completed
        );

        let recurring = service
            .create(
                CreateScheduleRequest {
                    trigger: ScheduleTrigger::Recurring {
                        cron: "every_ms:60000".into(),
                        timezone: "Asia/Shanghai".into(),
                        until_unix_ms: None,
                        max_occurrences: Some(2),
                    },
                    ..one_shot(0)
                },
                0,
            )
            .unwrap();
        let first = service
            .due(&recurring.schedule_id, 60_000, 1)
            .unwrap()
            .unwrap();
        service
            .complete_execution(
                ScheduledRunResult {
                    schedule_id: recurring.schedule_id.clone(),
                    execution_id: first.execution_id,
                    status: ScheduleExecutionStatus::Succeeded,
                    summary: "1".into(),
                    output_ref: None,
                    target: None,
                },
                60_000,
            )
            .unwrap();
        let second = service
            .due(&recurring.schedule_id, 120_000, 2)
            .unwrap()
            .unwrap();
        service
            .complete_execution(
                ScheduledRunResult {
                    schedule_id: recurring.schedule_id.clone(),
                    execution_id: second.execution_id,
                    status: ScheduleExecutionStatus::Succeeded,
                    summary: "2".into(),
                    output_ref: None,
                    target: None,
                },
                120_000,
            )
            .unwrap();
        assert_eq!(
            service
                .get(ScheduleIdRequest {
                    schedule_id: recurring.schedule_id
                })
                .unwrap()
                .status,
            ScheduleStatus::Completed
        );
    }

    #[test]
    fn pause_resume_cancel_and_restore_survive_restart() {
        let service = service();
        let created = service.create(one_shot(5_000), 0).unwrap();
        service
            .pause(
                ScheduleIdRequest {
                    schedule_id: created.schedule_id.clone(),
                },
                10,
            )
            .unwrap();
        assert!(
            service
                .due(&created.schedule_id, 5_000, 1)
                .unwrap()
                .is_none()
        );
        service
            .resume(
                ScheduleIdRequest {
                    schedule_id: created.schedule_id.clone(),
                },
                20,
            )
            .unwrap();
        let snapshot = service.snapshot();
        let restored = ProactiveScheduleService::new(Arc::new(InMemorySchedulerService::default()));
        restored.restore(snapshot).unwrap();
        assert_eq!(
            restored
                .list(ListSchedulesRequest::default())
                .unwrap()
                .len(),
            1
        );
        restored
            .cancel(
                ScheduleIdRequest {
                    schedule_id: created.schedule_id,
                },
                30,
            )
            .unwrap();
    }

    #[test]
    fn rejects_high_frequency_recurring_schedules() {
        let service = service();
        let err = service
            .create(
                CreateScheduleRequest {
                    trigger: ScheduleTrigger::Recurring {
                        cron: "every_ms:1000".into(),
                        timezone: "UTC".into(),
                        until_unix_ms: None,
                        max_occurrences: None,
                    },
                    ..one_shot(0)
                },
                0,
            )
            .unwrap_err();
        assert_eq!(err.code, "agent.invalid_input");
    }
}
