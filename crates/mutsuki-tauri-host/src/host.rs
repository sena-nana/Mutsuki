use crate::approval::ApprovalBridge;
use crate::config::MutsukiTauriConfig;
use crate::error::{HostError, HostResult};
use crate::health::{HostHealthState, failed_runtime_health, runtime_health_from_snapshots};
use mutsuki_runtime_contracts::{
    ERR_RUNNER_NOT_FOUND, ObservabilityPage, RuntimeError, RuntimeEvent, RuntimeEventKind,
    ScalarValue, Task, TaskBatch, TaskHandle, TaskStatus, TraceSpan,
};
use mutsuki_runtime_core::RuntimeFailure;
use mutsuki_runtime_host::{
    HostRuntime, HostRuntimeCommand, HostRuntimeReply, HostTaskSnapshot, HostTaskState,
    TaskCompletionSubscription,
};
use mutsuki_tauri_bridge::{
    ApprovalAttribution, ApprovalRequest, ApprovalResponse, FrontendContext, FrontendLogRecord,
    FrontendTaskRequest, FrontendTaskResult, FrontendTaskRun, HealthComponent, HostStatus,
    MutsukiFrontendEvent, PluginSummary, PreviewHandle, ResourceBytes, ResourceText, RunnerSummary,
    RuntimeHealth, TaskCancelRequest, TaskResultRequest, redact_log_record, redact_runtime_event,
};
use mutsuki_tauri_resource::TauriResourceStore;
use parking_lot::{Condvar, Mutex};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct MutsukiTauriHost {
    config: MutsukiTauriConfig,
    runtime: Arc<Mutex<HostRuntime>>,
    resources: Arc<TauriResourceStore>,
    events: Arc<mutsuki_tauri_bridge::EventHub>,
    tasks: Arc<TaskSupervisor>,
    approvals: ApprovalBridge,
    health: Arc<HostHealthState>,
    plugins: Vec<PluginSummary>,
    runners: Vec<RunnerSummary>,
    active_protocols: BTreeSet<String>,
}

pub(crate) struct HostComponents {
    pub runtime: HostRuntime,
    pub resources: Arc<TauriResourceStore>,
    pub events: Arc<mutsuki_tauri_bridge::EventHub>,
    pub health: Arc<HostHealthState>,
    pub plugins: Vec<PluginSummary>,
    pub runners: Vec<RunnerSummary>,
    pub active_protocols: BTreeSet<String>,
}

#[derive(Debug)]
struct TaskSupervisor {
    state: Mutex<TaskSupervisorState>,
    observation_drain: Mutex<()>,
    changed: Condvar,
    task_event_capacity_per_task: usize,
    task_event_capacity_total: usize,
}

#[derive(Debug, Default)]
struct TaskSupervisorState {
    active: BTreeMap<String, TaskHandle>,
    events_by_task: BTreeMap<String, TaskEventBuffer>,
    task_event_order: VecDeque<(String, u64)>,
    retained_task_events: usize,
    terminal_results: BTreeMap<String, Result<FrontendTaskResult, String>>,
    terminal_order: VecDeque<String>,
    last_runtime_event_sequence: u64,
    last_trace_span_sequence: u64,
    pump_running: bool,
    pump_subscription: Option<TaskCompletionSubscription>,
    pump_thread: Option<thread::JoinHandle<()>>,
    shutting_down: bool,
}

#[derive(Debug, Default)]
struct TaskEventBuffer {
    events: VecDeque<RuntimeEvent>,
    dropped: u64,
    truncated: bool,
}

impl Default for TaskSupervisor {
    fn default() -> Self {
        Self::new(256, 4096)
    }
}

impl TaskSupervisor {
    const MAX_RETAINED_RESULTS: usize = 256;

    fn new(task_event_capacity_per_task: usize, task_event_capacity_total: usize) -> Self {
        Self {
            state: Mutex::new(TaskSupervisorState::default()),
            observation_drain: Mutex::new(()),
            changed: Condvar::new(),
            task_event_capacity_per_task,
            task_event_capacity_total,
        }
    }

    fn track(&self, handle: TaskHandle) {
        let mut state = self.state.lock();
        let task_id = handle.task_id.clone();
        state.active.insert(task_id.clone(), handle);
        state.terminal_results.remove(&task_id);
        state.terminal_order.retain(|retained| retained != &task_id);
        Self::take_task_events(&mut state, &task_id);
    }

    fn handle_for(&self, task_id: &str) -> Option<TaskHandle> {
        self.state.lock().active.get(task_id).cloned()
    }

    fn wait_result(&self, task_id: &str, consume: bool) -> HostResult<FrontendTaskResult> {
        let mut state = self.state.lock();
        loop {
            if let Some(result) = state.terminal_results.get(task_id).cloned() {
                if consume && result.is_ok() {
                    state.terminal_results.remove(task_id);
                    state.terminal_order.retain(|retained| retained != task_id);
                }
                return result.map_err(HostError::Runtime);
            }
            if !state.active.contains_key(task_id) {
                return Err(HostError::Runtime(format!(
                    "task result is not tracked: {task_id}"
                )));
            }
            self.changed.wait(&mut state);
        }
    }

    fn start_pump(&self) -> bool {
        let mut state = self.state.lock();
        if state.pump_running || state.shutting_down {
            return false;
        }
        state.pump_running = true;
        true
    }

    fn set_pump_subscription(&self, subscription: TaskCompletionSubscription) -> bool {
        let mut state = self.state.lock();
        if state.shutting_down || !state.pump_running {
            return false;
        }
        state.pump_subscription = Some(subscription);
        true
    }

    fn set_pump_thread(&self, handle: thread::JoinHandle<()>) -> Option<thread::JoinHandle<()>> {
        let mut state = self.state.lock();
        if state.shutting_down {
            Some(handle)
        } else {
            state.pump_thread.replace(handle)
        }
    }

    fn active_snapshot(&self) -> Option<Vec<TaskHandle>> {
        let mut state = self.state.lock();
        if state.active.is_empty() || state.shutting_down {
            state.pump_running = false;
            state.pump_subscription.take();
            self.changed.notify_all();
            return None;
        }
        Some(state.active.values().cloned().collect())
    }

    fn observe_cursor(&self) -> (u64, u64) {
        let state = self.state.lock();
        (
            state.last_runtime_event_sequence,
            state.last_trace_span_sequence,
        )
    }

    fn ingest_runtime_page(
        &self,
        page: ObservabilityPage<RuntimeEvent>,
        health: &HostHealthState,
    ) -> Vec<MutsukiFrontendEvent> {
        let mut state = self.state.lock();
        state.last_runtime_event_sequence = page.next_sequence;
        let mut frontend_events = Vec::with_capacity(page.items.len() + usize::from(page.lost > 0));
        if page.lost > 0 {
            for task_id in state.active.keys().cloned().collect::<Vec<_>>() {
                state.events_by_task.entry(task_id).or_default().truncated = true;
            }
            frontend_events.push(MutsukiFrontendEvent::ObservabilityGap {
                stream: "runtime_event".into(),
                lost: page.lost,
                dropped: page.dropped,
            });
        }
        for event in page.items {
            let event = redact_runtime_event(event);
            health.record_runtime_event_error(&event);
            if let Some(task_id) = task_event_id(&event)
                && state.active.contains_key(&task_id)
            {
                self.retain_task_event(&mut state, task_id, event.clone());
            }
            frontend_events.push(frontend_event_for_runtime_event(event));
        }
        frontend_events
    }

    fn ingest_trace_page(&self, page: ObservabilityPage<TraceSpan>) -> Vec<MutsukiFrontendEvent> {
        self.state.lock().last_trace_span_sequence = page.next_sequence;
        let mut frontend_events = Vec::with_capacity(page.items.len() + usize::from(page.lost > 0));
        if page.lost > 0 {
            frontend_events.push(MutsukiFrontendEvent::ObservabilityGap {
                stream: "trace".into(),
                lost: page.lost,
                dropped: page.dropped,
            });
        }
        frontend_events.extend(
            page.items
                .into_iter()
                .map(|span| MutsukiFrontendEvent::Trace { span }),
        );
        frontend_events
    }

    fn finish_tasks(&self, states: Vec<HostTaskState>) -> bool {
        let mut state = self.state.lock();
        let mut completed = false;
        for task_state in states {
            if task_state.outcome.is_none() {
                continue;
            }
            let task_id = task_state.handle.task_id;
            let task_events = Self::take_task_events(&mut state, &task_id);
            let result = FrontendTaskResult {
                task_id: task_id.clone(),
                status: task_state.status,
                outcome: task_state.outcome,
                events: task_events.events.into_iter().collect(),
                events_dropped: task_events.dropped,
                events_truncated: task_events.truncated || task_events.dropped > 0,
            };
            state.active.remove(&task_id);
            Self::retain_terminal(&mut state, task_id, Ok(result));
            completed = true;
        }
        if completed {
            self.changed.notify_all();
        }
        let has_active = !state.active.is_empty();
        if !has_active {
            state.pump_running = false;
            state.pump_subscription.take();
        }
        has_active
    }

    fn fail_active(&self, message: String) {
        let mut state = self.state.lock();
        state.pump_running = false;
        if let Some(subscription) = state.pump_subscription.take() {
            subscription.close();
        }
        let active = std::mem::take(&mut state.active);
        for task_id in active.into_keys() {
            Self::take_task_events(&mut state, &task_id);
            Self::retain_terminal(&mut state, task_id, Err(message.clone()));
        }
        self.changed.notify_all();
    }

    fn retain_terminal(
        state: &mut TaskSupervisorState,
        task_id: String,
        result: Result<FrontendTaskResult, String>,
    ) {
        state.terminal_results.insert(task_id.clone(), result);
        state.terminal_order.push_back(task_id);
        while state.terminal_order.len() > Self::MAX_RETAINED_RESULTS {
            if let Some(evicted) = state.terminal_order.pop_front() {
                state.terminal_results.remove(&evicted);
                Self::take_task_events(state, &evicted);
            }
        }
    }

    fn retain_task_event(
        &self,
        state: &mut TaskSupervisorState,
        task_id: String,
        event: RuntimeEvent,
    ) {
        if self.task_event_capacity_per_task == 0 || self.task_event_capacity_total == 0 {
            let buffer = state.events_by_task.entry(task_id).or_default();
            buffer.dropped = buffer.dropped.saturating_add(1);
            buffer.truncated = true;
            return;
        }

        let evicted_sequence = {
            let buffer = state.events_by_task.entry(task_id.clone()).or_default();
            (buffer.events.len() >= self.task_event_capacity_per_task)
                .then(|| buffer.events.pop_front())
                .flatten()
                .map(|evicted| {
                    buffer.dropped = buffer.dropped.saturating_add(1);
                    buffer.truncated = true;
                    evicted.sequence
                })
        };
        if let Some(sequence) = evicted_sequence {
            if let Some(index) = state
                .task_event_order
                .iter()
                .position(|item| item == &(task_id.clone(), sequence))
            {
                state.task_event_order.remove(index);
            }
            state.retained_task_events = state.retained_task_events.saturating_sub(1);
        }

        let sequence = event.sequence;
        state
            .events_by_task
            .entry(task_id.clone())
            .or_default()
            .events
            .push_back(event);
        state.task_event_order.push_back((task_id, sequence));
        state.retained_task_events += 1;

        while state.retained_task_events > self.task_event_capacity_total {
            let Some((evicted_task_id, evicted_sequence)) = state.task_event_order.pop_front()
            else {
                break;
            };
            let removed = state
                .events_by_task
                .get_mut(&evicted_task_id)
                .and_then(|buffer| {
                    buffer
                        .events
                        .iter()
                        .position(|event| event.sequence == evicted_sequence)
                        .map(|index| {
                            buffer.events.remove(index);
                            buffer.dropped = buffer.dropped.saturating_add(1);
                            buffer.truncated = true;
                        })
                })
                .is_some();
            if removed {
                state.retained_task_events = state.retained_task_events.saturating_sub(1);
            }
        }
    }

    fn take_task_events(state: &mut TaskSupervisorState, task_id: &str) -> TaskEventBuffer {
        let buffer = state.events_by_task.remove(task_id).unwrap_or_default();
        state.retained_task_events = state
            .retained_task_events
            .saturating_sub(buffer.events.len());
        state
            .task_event_order
            .retain(|(retained_task_id, _)| retained_task_id != task_id);
        buffer
    }

    fn shutdown(&self, message: &str) {
        let (subscription, thread) = {
            let mut state = self.state.lock();
            state.shutting_down = true;
            state.pump_running = false;
            let active = std::mem::take(&mut state.active);
            for task_id in active.into_keys() {
                Self::take_task_events(&mut state, &task_id);
                Self::retain_terminal(&mut state, task_id, Err(message.to_string()));
            }
            self.changed.notify_all();
            (state.pump_subscription.take(), state.pump_thread.take())
        };
        if let Some(subscription) = subscription {
            subscription.close();
        }
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

impl MutsukiTauriHost {
    pub(crate) fn new(config: MutsukiTauriConfig, components: HostComponents) -> Self {
        let HostComponents {
            runtime,
            resources,
            events,
            health,
            plugins,
            runners,
            active_protocols,
        } = components;
        health.record_summary_failures(&plugins, &runners);
        let tasks = Arc::new(TaskSupervisor::new(
            config.task_event_capacity_per_task,
            config.task_event_capacity_total,
        ));
        Self {
            config,
            runtime: Arc::new(Mutex::new(runtime)),
            resources,
            events,
            tasks,
            approvals: ApprovalBridge::default(),
            health,
            plugins,
            runners,
            active_protocols,
        }
    }

    pub fn builder() -> crate::MutsukiTauriHostBuilder {
        crate::MutsukiTauriHostBuilder::new()
    }

    pub fn config(&self) -> &MutsukiTauriConfig {
        &self.config
    }

    pub fn event_hub(&self) -> Arc<mutsuki_tauri_bridge::EventHub> {
        self.events.clone()
    }

    pub fn resource_store(&self) -> Arc<TauriResourceStore> {
        self.resources.clone()
    }

    pub fn status(&self) -> HostStatus {
        let runtime = self.runtime_health();
        let runners = self.runners();
        let plugins = self.plugins_with_runner_state(&runners);
        let host = health_component(true, "running", None);
        let plugins_health = health_component(
            plugins.iter().all(|plugin| plugin.status == "loaded"),
            "ok",
            first_plugin_error(&plugins),
        );
        let runners_health = health_component(
            runners.iter().all(|runner| runner.status != "failed"),
            "ok",
            first_runner_error(&runners),
        );
        let healthy =
            runtime.healthy && host.healthy && plugins_health.healthy && runners_health.healthy;
        HostStatus {
            app_name: self.config.app_name.clone(),
            profile_id: self.config.profile_id.clone(),
            mode: format!("{:?}", self.config.mode).to_lowercase(),
            healthy,
            runtime,
            host,
            plugins_health,
            runners_health,
            recent_errors: self.health.recent_errors(),
            plugins,
            runners,
        }
    }

    pub fn plugins(&self) -> Vec<PluginSummary> {
        let runners = self.runners();
        self.plugins_with_runner_state(&runners)
    }

    pub fn runners(&self) -> Vec<RunnerSummary> {
        self.runners
            .iter()
            .cloned()
            .map(|mut runner| {
                if runner.status != "failed"
                    && let Some(failure) = self.health.runner_failure(&runner.runner_id)
                {
                    runner.status = "failed".into();
                    runner.error = Some(failure.message().into());
                }
                runner
            })
            .collect()
    }

    fn runtime_health(&self) -> RuntimeHealth {
        let result = self.runtime.lock().task_snapshots();
        match result {
            Ok(snapshots) => runtime_health_from_snapshots(&snapshots),
            Err(error) => {
                let message = format!("{:?}", error.error());
                self.health.record_runtime_probe_error(message.clone());
                failed_runtime_health(message)
            }
        }
    }

    fn plugins_with_runner_state(&self, runners: &[RunnerSummary]) -> Vec<PluginSummary> {
        self.plugins
            .iter()
            .cloned()
            .map(|mut plugin| {
                if plugin.status == "failed" {
                    return plugin;
                }
                if let Some(runner) = runners.iter().find(|runner| {
                    runner.plugin_id == plugin.plugin_id && runner.status == "failed"
                }) {
                    plugin.status = "degraded".into();
                    plugin.error = Some(format!("runner {} failed", runner.runner_id));
                }
                plugin
            })
            .collect()
    }

    pub fn emit_log(
        &self,
        level: impl Into<String>,
        target: impl Into<String>,
        message: impl Into<String>,
        fields: BTreeMap<String, Value>,
    ) {
        emit_log_record(
            &self.events,
            level.into(),
            target.into(),
            message.into(),
            None,
            None,
            fields,
        );
    }

    pub fn call(&self, request: FrontendTaskRequest) -> HostResult<FrontendTaskResult> {
        let run = self.start_task(request)?;
        self.task_result(TaskResultRequest {
            task_id: run.task_id,
        })
    }

    pub fn start_task(&self, request: FrontendTaskRequest) -> HostResult<FrontendTaskRun> {
        let handle = self.submit_task(request.into_task())?;
        Ok(FrontendTaskRun {
            task_id: handle.task_id,
        })
    }

    pub fn submit_task(&self, task: Task) -> HostResult<TaskHandle> {
        self.ensure_protocol_available(&task.protocol_id)?;
        let submitted = self.dispatch_submission(HostRuntimeCommand::SubmitTask(Box::new(task)))?;
        let handle = match submitted {
            HostRuntimeReply::TaskSubmitted(handle) => handle,
            _ => return Err(HostError::Runtime("unexpected submit reply".into())),
        };
        self.track_submitted(std::slice::from_ref(&handle))?;
        Ok(handle)
    }

    pub fn submit_batch(&self, batch: TaskBatch) -> HostResult<Vec<TaskHandle>> {
        for task in &batch.tasks {
            self.ensure_protocol_available(&task.protocol_id)?;
        }
        let submitted =
            self.dispatch_submission(HostRuntimeCommand::SubmitBatch(Box::new(batch)))?;
        let handles = match submitted {
            HostRuntimeReply::TaskBatchSubmitted(handles) => handles,
            _ => return Err(HostError::Runtime("unexpected submit batch reply".into())),
        };
        self.track_submitted(&handles)?;
        Ok(handles)
    }

    fn ensure_protocol_available(&self, protocol_id: &str) -> HostResult<()> {
        if self.active_protocols.contains(protocol_id) {
            return Ok(());
        }
        let mut error = RuntimeError::new(
            ERR_RUNNER_NOT_FOUND,
            "mutsuki_tauri_host",
            "host.task.submit",
        );
        error.evidence.insert(
            "protocol_id".into(),
            ScalarValue::String(protocol_id.into()),
        );
        Err(HostError::RuntimeFailure(RuntimeFailure::new(error)))
    }

    pub fn task_snapshots(&self) -> HostResult<Vec<HostTaskSnapshot>> {
        self.runtime.lock().task_snapshots().map_err(Into::into)
    }

    pub fn runtime_metrics(&self) -> mutsuki_runtime_host::HostRuntimeMetricsSnapshot {
        self.runtime.lock().metrics()
    }

    fn dispatch_submission(&self, command: HostRuntimeCommand) -> HostResult<HostRuntimeReply> {
        let submitted = self.runtime.lock().dispatch(command);

        let reply = match submitted {
            Ok(reply) => reply,
            Err(error) => {
                self.emit_runtime_error_log(
                    "mutsuki_tauri_host.runtime",
                    "runtime task submit failed",
                    format!("{:?}", error.error()),
                );
                return Err(error.into());
            }
        };
        Ok(reply)
    }

    fn track_submitted(&self, handles: &[TaskHandle]) -> HostResult<()> {
        for handle in handles {
            self.tasks.track(handle.clone());
        }
        let observed = self.drain_observability();
        self.ensure_task_pump();
        observed
    }

    pub fn task_result(&self, request: TaskResultRequest) -> HostResult<FrontendTaskResult> {
        self.tasks.wait_result(&request.task_id, true)
    }

    pub fn peek_task_result(&self, request: TaskResultRequest) -> HostResult<FrontendTaskResult> {
        self.tasks.wait_result(&request.task_id, false)
    }

    pub fn cancel_task(&self, request: TaskCancelRequest) -> HostResult<String> {
        let handle = self.tasks.handle_for(&request.task_id).ok_or_else(|| {
            HostError::Runtime(format!("task is not tracked: {}", request.task_id))
        })?;
        self.cancel_task_handle(handle).map(|handle| handle.task_id)
    }

    pub fn cancel_task_handle(&self, handle: TaskHandle) -> HostResult<TaskHandle> {
        let runtime = self.runtime.lock();
        let reply = runtime.dispatch(HostRuntimeCommand::CancelTask(handle));
        drop(runtime);
        let reply = match reply {
            Ok(reply) => reply,
            Err(error) => {
                self.emit_runtime_error_log(
                    "mutsuki_tauri_host.runtime",
                    "runtime task cancel failed",
                    format!("{:?}", error.error()),
                );
                return Err(error.into());
            }
        };
        match reply {
            HostRuntimeReply::TaskCancelled(handle) => {
                let observed = self.drain_observability();
                self.ensure_task_pump();
                observed.map(|_| handle)
            }
            _ => Err(HostError::Runtime("unexpected cancel reply".into())),
        }
    }

    pub fn task_status(&self, task_id: &str) -> Option<TaskStatus> {
        self.runtime.lock().task_status(task_id)
    }

    fn ensure_task_pump(&self) {
        if !self.tasks.start_pump() {
            return;
        }
        let subscription = self.runtime.lock().subscribe_task_completions();
        if !self.tasks.set_pump_subscription(subscription.clone()) {
            subscription.close();
            return;
        }

        let runtime = self.runtime.clone();
        let events = self.events.clone();
        let tasks = self.tasks.clone();
        let health = self.health.clone();
        let frontend_event_batch_size = self.config.frontend_event_batch_size;
        let spawn_result = thread::Builder::new()
            .name("mutsuki-tauri-task-pump".into())
            .spawn(move || {
                run_task_pump(
                    runtime,
                    events,
                    tasks,
                    health,
                    subscription,
                    frontend_event_batch_size,
                )
            });

        match spawn_result {
            Ok(handle) => {
                if let Some(handle) = self.tasks.set_pump_thread(handle) {
                    let _ = handle.join();
                }
            }
            Err(error) => self
                .tasks
                .fail_active(format!("failed to spawn task pump: {error}")),
        }
    }

    fn drain_observability(&self) -> HostResult<()> {
        drain_observability(
            &self.runtime,
            &self.events,
            &self.tasks,
            &self.health,
            self.config.frontend_event_batch_size,
        )
        .map_err(|error| {
            self.emit_runtime_error_log(
                "mutsuki_tauri_host.observe",
                "runtime observability drain failed",
                error.clone(),
            );
            HostError::Runtime(error)
        })
    }

    fn emit_runtime_error_log(
        &self,
        target: impl Into<String>,
        message: impl Into<String>,
        error: impl Into<String>,
    ) {
        let target = target.into();
        let message = message.into();
        let error = error.into();
        self.health
            .record_host_error(&target, format!("{message}: {error}"));
        emit_log_record(
            &self.events,
            "error".into(),
            target,
            message,
            None,
            None,
            BTreeMap::from([("error".into(), json!(error))]),
        );
    }

    pub async fn import_file(
        &self,
        path: impl AsRef<Path>,
    ) -> HostResult<mutsuki_runtime_contracts::ResourceRef> {
        let resource = self.resources.import_file(path).await?;
        let _ = self.events.emit(MutsukiFrontendEvent::Resource {
            ref_id: resource.ref_id.clone(),
            operation: "import_file".into(),
        });
        Ok(resource)
    }

    pub async fn read_resource_bytes(&self, ref_id: &str) -> HostResult<ResourceBytes> {
        let resource = self.resources.descriptor(ref_id)?;
        let bytes = self.resources.read_bytes(ref_id).await?;
        Ok(ResourceBytes {
            media_type: None,
            resource,
            bytes,
        })
    }

    pub async fn read_resource_text(&self, ref_id: &str) -> HostResult<ResourceText> {
        Ok(ResourceText {
            ref_id: ref_id.into(),
            text: self.resources.read_text(ref_id).await?,
        })
    }

    pub async fn write_resource_bytes(
        &self,
        ref_id: &str,
        bytes: Vec<u8>,
    ) -> HostResult<mutsuki_runtime_contracts::ResourceRef> {
        let resource = self.resources.write_bytes(ref_id, bytes).await?;
        let _ = self.events.emit(MutsukiFrontendEvent::Resource {
            ref_id: ref_id.into(),
            operation: "write".into(),
        });
        Ok(resource)
    }

    pub async fn export_resource_to_file(
        &self,
        ref_id: &str,
        target: impl AsRef<Path>,
    ) -> HostResult<()> {
        self.resources.export_to_file(ref_id, target).await?;
        let _ = self.events.emit(MutsukiFrontendEvent::Resource {
            ref_id: ref_id.into(),
            operation: "export_file".into(),
        });
        Ok(())
    }

    pub fn create_preview_handle(&self, ref_id: &str) -> HostResult<PreviewHandle> {
        Ok(self
            .resources
            .create_preview_handle(ref_id, Duration::from_secs(self.config.preview_ttl_secs))?)
    }

    pub fn request_approval(
        &self,
        requester: impl Into<String>,
        operation: impl Into<String>,
        risk: impl Into<String>,
        payload: Value,
        context: FrontendContext,
    ) -> ApprovalRequest {
        self.emit_approval_request(
            self.approvals
                .request(requester, operation, risk, payload, context),
        )
    }

    pub fn request_approval_with_attribution(
        &self,
        requester: impl Into<String>,
        operation: impl Into<String>,
        risk: impl Into<String>,
        payload: Value,
        attribution: ApprovalAttribution,
    ) -> ApprovalRequest {
        self.emit_approval_request(self.approvals.request_with_attribution(
            requester,
            operation,
            risk,
            payload,
            attribution,
        ))
    }

    fn emit_approval_request(&self, request: ApprovalRequest) -> ApprovalRequest {
        let _ = self.events.emit(MutsukiFrontendEvent::Approval {
            request: request.clone(),
        });
        request
    }

    pub fn resolve_approval(
        &self,
        response: ApprovalResponse,
    ) -> HostResult<mutsuki_tauri_bridge::ApprovalDecision> {
        self.approvals.resolve(response)
    }

    pub fn pending_approvals(&self) -> Vec<ApprovalRequest> {
        self.approvals.pending()
    }

    pub fn shutdown(&self) {
        let abort_error = self
            .runtime
            .lock()
            .abort("tauri_host.shutdown")
            .err()
            .map(|error| format!("{:?}", error.error()));
        self.tasks.shutdown(
            abort_error
                .as_deref()
                .unwrap_or("MutsukiTauriHost is shutting down"),
        );
    }
}

impl Drop for MutsukiTauriHost {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run_task_pump(
    runtime: Arc<Mutex<HostRuntime>>,
    events: Arc<mutsuki_tauri_bridge::EventHub>,
    tasks: Arc<TaskSupervisor>,
    health: Arc<HostHealthState>,
    completion_subscription: TaskCompletionSubscription,
    frontend_event_batch_size: usize,
) {
    let mut completion_revision = completion_subscription.revision();
    loop {
        let Some(active) = tasks.active_snapshot() else {
            return;
        };

        let snapshot = runtime
            .lock()
            .task_states(active)
            .map_err(|error| format!("{:?}", error.error()));

        match snapshot {
            Ok(states) => {
                if let Err(error) = drain_observability(
                    &runtime,
                    &events,
                    &tasks,
                    &health,
                    frontend_event_batch_size,
                ) {
                    emit_log_record(
                        &events,
                        "error".into(),
                        "mutsuki_tauri_host.observe".into(),
                        "runtime observability drain failed".into(),
                        None,
                        None,
                        BTreeMap::from([("error".into(), json!(error.clone()))]),
                    );
                    tasks.fail_active(error);
                    return;
                }
                if !tasks.finish_tasks(states) {
                    return;
                }
            }
            Err(error) => {
                emit_log_record(
                    &events,
                    "error".into(),
                    "mutsuki_tauri_host.runtime".into(),
                    "runtime task pump failed".into(),
                    None,
                    None,
                    BTreeMap::from([("error".into(), json!(error.clone()))]),
                );
                tasks.fail_active(error);
                return;
            }
        }
        let Some(revision) = completion_subscription.wait_after(completion_revision) else {
            tasks.fail_active("runtime completion subscription closed".into());
            return;
        };
        completion_revision = revision;
    }
}

fn task_event_id(event: &RuntimeEvent) -> Option<String> {
    (event.kind == RuntimeEventKind::Task)
        .then(|| event.subject_id.clone())
        .flatten()
}

fn frontend_event_for_runtime_event(event: RuntimeEvent) -> MutsukiFrontendEvent {
    if let Some(task_id) = task_event_id(&event) {
        MutsukiFrontendEvent::Task { task_id, event }
    } else {
        MutsukiFrontendEvent::Runtime { event }
    }
}

fn health_component(healthy: bool, healthy_status: &str, error: Option<String>) -> HealthComponent {
    HealthComponent {
        healthy,
        status: if healthy {
            healthy_status.into()
        } else {
            "failed".into()
        },
        error: if healthy { None } else { error },
    }
}

fn first_plugin_error(plugins: &[PluginSummary]) -> Option<String> {
    plugins
        .iter()
        .find(|plugin| plugin.status != "loaded")
        .and_then(|plugin| {
            plugin
                .error
                .clone()
                .or_else(|| Some(format!("plugin {} is {}", plugin.plugin_id, plugin.status)))
        })
}

fn first_runner_error(runners: &[RunnerSummary]) -> Option<String> {
    runners
        .iter()
        .find(|runner| runner.status == "failed")
        .and_then(|runner| {
            runner
                .error
                .clone()
                .or_else(|| Some(format!("runner {} failed", runner.runner_id)))
        })
}

fn drain_observability(
    runtime: &Arc<Mutex<HostRuntime>>,
    events: &Arc<mutsuki_tauri_bridge::EventHub>,
    tasks: &Arc<TaskSupervisor>,
    health: &HostHealthState,
    frontend_event_batch_size: usize,
) -> Result<(), String> {
    const PAGE_LIMIT: usize = 256;
    let _drain = tasks.observation_drain.lock();
    let (mut event_sequence, mut trace_sequence) = tasks.observe_cursor();

    loop {
        let page = runtime
            .lock()
            .events_after(event_sequence, PAGE_LIMIT)
            .map_err(|error| format!("{:?}", error.error()))?;
        let next_sequence = page.next_sequence;
        let truncated = page.truncated;
        emit_frontend_events(
            events,
            tasks.ingest_runtime_page(page, health),
            frontend_event_batch_size,
        );
        if !truncated {
            break;
        }
        if next_sequence <= event_sequence {
            return Err("runtime event cursor did not advance".into());
        }
        event_sequence = next_sequence;
    }

    loop {
        let page = runtime
            .lock()
            .trace_spans_after(trace_sequence, PAGE_LIMIT)
            .map_err(|error| format!("{:?}", error.error()))?;
        let next_sequence = page.next_sequence;
        let truncated = page.truncated;
        emit_frontend_events(
            events,
            tasks.ingest_trace_page(page),
            frontend_event_batch_size,
        );
        if !truncated {
            break;
        }
        if next_sequence <= trace_sequence {
            return Err("trace cursor did not advance".into());
        }
        trace_sequence = next_sequence;
    }
    Ok(())
}

fn emit_frontend_events(
    events: &mutsuki_tauri_bridge::EventHub,
    frontend_events: Vec<MutsukiFrontendEvent>,
    batch_size: usize,
) {
    let batch_size = batch_size.max(1);
    let mut pending = frontend_events;
    while !pending.is_empty() {
        let tail = if pending.len() > batch_size {
            pending.split_off(batch_size)
        } else {
            Vec::new()
        };
        let _ = events.emit_batch(pending);
        pending = tail;
    }
}

fn emit_log_record(
    events: &mutsuki_tauri_bridge::EventHub,
    level: String,
    target: String,
    message: String,
    trace_id: Option<String>,
    correlation_id: Option<String>,
    fields: BTreeMap<String, Value>,
) {
    let record = redact_log_record(FrontendLogRecord {
        level,
        target,
        message,
        timestamp_ms: current_timestamp_ms(),
        trace_id,
        correlation_id,
        fields,
    });
    let _ = events.emit(MutsukiFrontendEvent::Log { record });
}

fn current_timestamp_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod supervisor_tests {
    use super::TaskSupervisor;
    use crate::health::HostHealthState;
    use mutsuki_runtime_contracts::{
        CancelPolicy, ObservabilityPage, RuntimeEvent, RuntimeEventKind, TaskHandle, TaskOutcome,
        TaskStatus,
    };
    use mutsuki_runtime_host::HostTaskState;
    use std::collections::BTreeMap;

    #[test]
    fn unread_terminal_results_are_bounded() {
        let supervisor = TaskSupervisor::default();
        let handles = (0..=TaskSupervisor::MAX_RETAINED_RESULTS)
            .map(|index| TaskHandle {
                task_id: format!("task:retained:{index:03}"),
                protocol_id: "test.retention".into(),
                target_binding_id: None,
                cancel_policy: CancelPolicy::Cascade,
                trace_id: None,
                correlation_id: None,
            })
            .collect::<Vec<_>>();
        for handle in &handles {
            supervisor.track(handle.clone());
        }
        supervisor.finish_tasks(
            handles
                .iter()
                .map(|handle| HostTaskState {
                    handle: handle.clone(),
                    status: Some(TaskStatus::Completed),
                    outcome: Some(TaskOutcome::Completed {
                        task_id: handle.task_id.clone(),
                        output_ref: None,
                    }),
                })
                .collect(),
        );
        assert_eq!(
            supervisor.state.lock().terminal_results.len(),
            TaskSupervisor::MAX_RETAINED_RESULTS
        );

        assert!(
            supervisor
                .wait_result(&handles.last().expect("newest handle").task_id, true)
                .is_ok()
        );
        assert!(
            supervisor
                .wait_result(&handles.first().expect("oldest handle").task_id, true)
                .is_err()
        );

        let failed = TaskSupervisor::default();
        for handle in &handles {
            failed.track(handle.clone());
        }
        failed.fail_active("pump failed".into());
        let state = failed.state.lock();
        assert_eq!(
            state.terminal_results.len(),
            TaskSupervisor::MAX_RETAINED_RESULTS
        );
        assert!(!state.terminal_results.contains_key(&handles[0].task_id));
        assert!(
            state
                .terminal_results
                .contains_key(&handles.last().expect("newest handle").task_id)
        );
    }

    #[test]
    fn task_event_cache_enforces_per_task_and_global_limits() {
        let supervisor = TaskSupervisor::new(2, 3);
        let handles = ["task:event:a", "task:event:b"].map(|task_id| TaskHandle {
            task_id: task_id.into(),
            protocol_id: "test.events".into(),
            target_binding_id: None,
            cancel_policy: CancelPolicy::Cascade,
            trace_id: None,
            correlation_id: None,
        });
        for handle in &handles {
            supervisor.track(handle.clone());
        }
        let items = (1..=6)
            .map(|sequence| RuntimeEvent {
                sequence,
                kind: RuntimeEventKind::Task,
                name: format!("task.event.{sequence}"),
                subject_id: Some(handles[(sequence as usize) % 2].task_id.clone()),
                attributes: BTreeMap::new(),
                error: None,
            })
            .collect();
        supervisor.ingest_runtime_page(
            ObservabilityPage {
                items,
                next_sequence: 6,
                earliest_available_sequence: Some(1),
                latest_sequence: 6,
                lost: 0,
                truncated: false,
                dropped: 0,
            },
            &HostHealthState::default(),
        );
        {
            let state = supervisor.state.lock();
            assert!(state.retained_task_events <= 3);
            assert!(
                state
                    .events_by_task
                    .values()
                    .all(|buffer| buffer.events.len() <= 2)
            );
        }

        supervisor.finish_tasks(
            handles
                .iter()
                .map(|handle| HostTaskState {
                    handle: handle.clone(),
                    status: Some(TaskStatus::Completed),
                    outcome: Some(TaskOutcome::Completed {
                        task_id: handle.task_id.clone(),
                        output_ref: None,
                    }),
                })
                .collect(),
        );
        let results = handles
            .iter()
            .map(|handle| {
                supervisor
                    .wait_result(&handle.task_id, false)
                    .expect("peek result")
            })
            .collect::<Vec<_>>();
        assert!(results.iter().all(|result| result.events.len() <= 2));
        assert!(
            results
                .iter()
                .any(|result| { result.events_dropped > 0 && result.events_truncated })
        );
    }

    #[test]
    fn result_peek_is_repeatable_and_failed_reads_are_retained() {
        let supervisor = TaskSupervisor::default();
        let handle = TaskHandle {
            task_id: "task:peek".into(),
            protocol_id: "test.peek".into(),
            target_binding_id: None,
            cancel_policy: CancelPolicy::Cascade,
            trace_id: None,
            correlation_id: None,
        };
        supervisor.track(handle.clone());
        supervisor.finish_tasks(vec![HostTaskState {
            handle: handle.clone(),
            status: Some(TaskStatus::Completed),
            outcome: Some(TaskOutcome::Completed {
                task_id: handle.task_id.clone(),
                output_ref: None,
            }),
        }]);

        assert!(supervisor.wait_result(&handle.task_id, false).is_ok());
        assert!(supervisor.wait_result(&handle.task_id, false).is_ok());
        assert!(supervisor.wait_result(&handle.task_id, true).is_ok());
        assert!(supervisor.wait_result(&handle.task_id, true).is_err());

        let failed = TaskSupervisor::default();
        failed.track(handle.clone());
        failed.fail_active("pump failed".into());
        assert!(failed.wait_result(&handle.task_id, true).is_err());
        assert!(failed.wait_result(&handle.task_id, true).is_err());
    }
}
