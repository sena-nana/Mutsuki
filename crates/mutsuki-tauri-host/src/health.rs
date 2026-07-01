use mutsuki_runtime_contracts::{RuntimeError, RuntimeEvent, RuntimeEventKind, TaskStatus};
use mutsuki_runtime_host::HostTaskSnapshot;
use mutsuki_tauri_bridge::{HostRecentError, PluginSummary, RunnerSummary, RuntimeHealth};
use parking_lot::Mutex;
use std::collections::{BTreeMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

const RECENT_ERROR_LIMIT: usize = 20;

#[derive(Debug, Default)]
pub(crate) struct HostHealthState {
    state: Mutex<HealthState>,
}

#[derive(Debug, Default)]
struct HealthState {
    recent_errors: VecDeque<HostRecentError>,
    runner_failures: BTreeMap<String, RunnerRuntimeFailure>,
}

#[derive(Clone, Debug)]
pub(crate) struct RunnerRuntimeFailure {
    message: String,
}

impl HostHealthState {
    pub(crate) fn record_summary_failures(
        &self,
        plugins: &[PluginSummary],
        runners: &[RunnerSummary],
    ) {
        for plugin in plugins {
            if plugin.status == "failed" || plugin.error.is_some() {
                self.record_plugin_error(
                    &plugin.plugin_id,
                    plugin.error.as_deref().unwrap_or("plugin load failed"),
                );
            }
        }
        for runner in runners {
            if runner.status == "failed" || runner.error.is_some() {
                self.record_runner_load_error(
                    &runner.runner_id,
                    &runner.plugin_id,
                    runner.error.as_deref().unwrap_or("runner load failed"),
                );
            }
        }
    }

    pub(crate) fn record_runtime_event_error(&self, event: &RuntimeEvent) {
        let Some(error) = event.error.as_ref() else {
            return;
        };
        let message = format!("{} at {}", error.code, error.route);
        let task_id = (event.kind == RuntimeEventKind::Task)
            .then(|| event.subject_id.clone())
            .flatten();
        self.push_error(HostRecentError {
            source: error.source.clone(),
            message,
            timestamp_ms: current_timestamp_ms(),
            plugin_id: scalar_string(error, "plugin_id"),
            runner_id: scalar_string(error, "runner_id"),
            task_id,
            code: Some(error.code.clone()),
            route: Some(error.route.clone()),
        });
    }

    pub(crate) fn record_runtime_probe_error(&self, message: impl Into<String>) {
        self.record_host_error("mutsuki_tauri_host.runtime", message);
    }

    pub(crate) fn record_host_error(&self, source: impl Into<String>, message: impl Into<String>) {
        self.push_error(HostRecentError {
            source: source.into(),
            message: message.into(),
            timestamp_ms: current_timestamp_ms(),
            plugin_id: None,
            runner_id: None,
            task_id: None,
            code: None,
            route: None,
        });
    }

    pub(crate) fn record_runner_runtime_error(
        &self,
        runner_id: &str,
        plugin_id: &str,
        error: &RuntimeError,
    ) {
        let message = format!("{} at {}", error.code, error.route);
        {
            let mut state = self.state.lock();
            state.runner_failures.insert(
                runner_id.into(),
                RunnerRuntimeFailure {
                    message: message.clone(),
                },
            );
        }
        self.push_error(HostRecentError {
            source: "mutsuki_tauri_host.runner".into(),
            message,
            timestamp_ms: current_timestamp_ms(),
            plugin_id: Some(plugin_id.into()),
            runner_id: Some(runner_id.into()),
            task_id: None,
            code: Some(error.code.clone()),
            route: Some(error.route.clone()),
        });
    }

    pub(crate) fn recent_errors(&self) -> Vec<HostRecentError> {
        self.state.lock().recent_errors.iter().cloned().collect()
    }

    pub(crate) fn runner_failure(&self, runner_id: &str) -> Option<RunnerRuntimeFailure> {
        self.state.lock().runner_failures.get(runner_id).cloned()
    }

    fn record_plugin_error(&self, plugin_id: &str, message: &str) {
        self.push_error(HostRecentError {
            source: "mutsuki_tauri_host.plugin".into(),
            message: message.into(),
            timestamp_ms: current_timestamp_ms(),
            plugin_id: Some(plugin_id.into()),
            runner_id: None,
            task_id: None,
            code: None,
            route: None,
        });
    }

    fn record_runner_load_error(&self, runner_id: &str, plugin_id: &str, message: &str) {
        {
            let mut state = self.state.lock();
            state.runner_failures.insert(
                runner_id.into(),
                RunnerRuntimeFailure {
                    message: message.into(),
                },
            );
        }
        self.push_error(HostRecentError {
            source: "mutsuki_tauri_host.runner".into(),
            message: message.into(),
            timestamp_ms: current_timestamp_ms(),
            plugin_id: Some(plugin_id.into()),
            runner_id: Some(runner_id.into()),
            task_id: None,
            code: None,
            route: None,
        });
    }

    fn push_error(&self, error: HostRecentError) {
        let mut state = self.state.lock();
        if state.recent_errors.len() == RECENT_ERROR_LIMIT {
            state.recent_errors.pop_front();
        }
        state.recent_errors.push_back(error);
    }
}

impl RunnerRuntimeFailure {
    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

pub(crate) fn runtime_health_from_snapshots(snapshots: &[HostTaskSnapshot]) -> RuntimeHealth {
    RuntimeHealth {
        healthy: true,
        status: "running".into(),
        active_tasks: snapshots
            .iter()
            .filter(|snapshot| is_active_status(&snapshot.status))
            .count(),
        failed_tasks: snapshots
            .iter()
            .filter(|snapshot| is_failed_status(&snapshot.status))
            .count(),
        error: None,
    }
}

pub(crate) fn failed_runtime_health(error: impl Into<String>) -> RuntimeHealth {
    RuntimeHealth {
        healthy: false,
        status: "failed".into(),
        active_tasks: 0,
        failed_tasks: 0,
        error: Some(error.into()),
    }
}

fn is_active_status(status: &TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Created
            | TaskStatus::Ready
            | TaskStatus::Running
            | TaskStatus::Waiting
            | TaskStatus::Blocked
    )
}

fn is_failed_status(status: &TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Failed | TaskStatus::Expired | TaskStatus::DeadLetter
    )
}

fn scalar_string(error: &RuntimeError, key: &str) -> Option<String> {
    match error.evidence.get(key) {
        Some(mutsuki_runtime_contracts::ScalarValue::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn current_timestamp_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}
