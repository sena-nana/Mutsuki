use crate::approval::ApprovalBridge;
use crate::config::MutsukiTauriConfig;
use crate::error::{HostError, HostResult};
use mutsuki_runtime_contracts::{
    RuntimeEvent, RuntimeEventKind, TaskOutcome, TaskStatus, TraceSpan,
};
use mutsuki_runtime_host::{HostRuntime, HostRuntimeCommand, HostRuntimeReply};
use mutsuki_tauri_bridge::{
    ApprovalAttribution, ApprovalRequest, ApprovalResponse, FrontendContext, FrontendLogRecord,
    FrontendTaskRequest, FrontendTaskResult, FrontendTaskRun, HostStatus, MutsukiFrontendEvent,
    PluginSummary, PreviewHandle, ResourceBytes, ResourceText, RunnerSummary, TaskCancelRequest,
    TaskResultRequest, redact_log_record, redact_runtime_event,
};
use mutsuki_tauri_resource::TauriResourceStore;
use parking_lot::{Condvar, Mutex};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
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
    plugins: Vec<PluginSummary>,
    runners: Vec<RunnerSummary>,
}

#[derive(Debug, Default)]
struct TaskSupervisor {
    state: Mutex<TaskSupervisorState>,
    changed: Condvar,
}

#[derive(Debug, Default)]
struct TaskSupervisorState {
    active: BTreeSet<String>,
    events_by_task: BTreeMap<String, Vec<RuntimeEvent>>,
    results: BTreeMap<String, FrontendTaskResult>,
    errors: BTreeMap<String, String>,
    last_runtime_event_sequence: u64,
    last_trace_span_index: usize,
    pump_running: bool,
}

impl TaskSupervisor {
    fn track(&self, task_id: &str) {
        let mut state = self.state.lock();
        state.active.insert(task_id.to_string());
        state.errors.remove(task_id);
        state.results.remove(task_id);
        state.events_by_task.remove(task_id);
    }

    fn forget(&self, task_id: &str) {
        self.state.lock().active.remove(task_id);
        self.changed.notify_all();
    }

    fn wait_result(&self, task_id: &str) -> HostResult<FrontendTaskResult> {
        let mut state = self.state.lock();
        loop {
            if let Some(result) = state.results.get(task_id) {
                return Ok(result.clone());
            }
            if let Some(error) = state.errors.remove(task_id) {
                return Err(HostError::Runtime(error));
            }
            if !state.active.contains(task_id) {
                return Err(HostError::Runtime(format!(
                    "task result is not tracked: {task_id}"
                )));
            }
            self.changed.wait(&mut state);
        }
    }

    fn start_pump(&self) -> bool {
        let mut state = self.state.lock();
        if state.pump_running {
            return false;
        }
        state.pump_running = true;
        true
    }

    fn active_snapshot(&self) -> Option<Vec<String>> {
        let mut state = self.state.lock();
        if state.active.is_empty() {
            state.pump_running = false;
            self.changed.notify_all();
            return None;
        }
        Some(state.active.iter().cloned().collect())
    }

    fn observe_cursor(&self) -> (u64, usize) {
        let state = self.state.lock();
        (
            state.last_runtime_event_sequence,
            state.last_trace_span_index,
        )
    }

    fn ingest_runtime_events(
        &self,
        core_events: Vec<RuntimeEvent>,
        events: &mutsuki_tauri_bridge::EventHub,
    ) {
        let mut state = self.state.lock();
        for event in core_events {
            state.last_runtime_event_sequence =
                state.last_runtime_event_sequence.max(event.sequence);
            let event = redact_runtime_event(event);
            if let Some(task_id) = task_event_id(&event) {
                if state.active.contains(&task_id) {
                    state
                        .events_by_task
                        .entry(task_id)
                        .or_default()
                        .push(event.clone());
                }
            }
            let _ = events.emit(frontend_event_for_runtime_event(event));
        }
    }

    fn ingest_trace_spans(
        &self,
        next_index: usize,
        trace_spans: Vec<TraceSpan>,
        events: &mutsuki_tauri_bridge::EventHub,
    ) {
        self.state.lock().last_trace_span_index = next_index;
        for span in trace_spans {
            let _ = events.emit(MutsukiFrontendEvent::Trace { span });
        }
    }

    fn finish_tasks(&self, outcomes: Vec<(String, Option<TaskStatus>, Option<TaskOutcome>)>) {
        let mut state = self.state.lock();
        let mut completed = false;
        for (task_id, status, outcome) in outcomes {
            if outcome.is_none() {
                continue;
            }
            let result = FrontendTaskResult {
                task_id: task_id.clone(),
                status,
                outcome,
                events: state
                    .events_by_task
                    .get(&task_id)
                    .cloned()
                    .unwrap_or_default(),
            };
            state.active.remove(&task_id);
            state.results.insert(task_id, result);
            completed = true;
        }
        if completed {
            self.changed.notify_all();
        }
    }

    fn fail_active(&self, message: String) {
        let mut state = self.state.lock();
        state.pump_running = false;
        let active = std::mem::take(&mut state.active);
        for task_id in active {
            state.errors.insert(task_id, message.clone());
        }
        self.changed.notify_all();
    }
}

impl MutsukiTauriHost {
    pub(crate) fn new(
        config: MutsukiTauriConfig,
        runtime: HostRuntime,
        resources: Arc<TauriResourceStore>,
        events: Arc<mutsuki_tauri_bridge::EventHub>,
        plugins: Vec<PluginSummary>,
        runners: Vec<RunnerSummary>,
    ) -> Self {
        Self {
            config,
            runtime: Arc::new(Mutex::new(runtime)),
            resources,
            events,
            tasks: Arc::new(TaskSupervisor::default()),
            approvals: ApprovalBridge::default(),
            plugins,
            runners,
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
        HostStatus {
            app_name: self.config.app_name.clone(),
            profile_id: self.config.profile_id.clone(),
            mode: format!("{:?}", self.config.mode).to_lowercase(),
            healthy: true,
            plugins: self.plugins(),
            runners: self.runners(),
        }
    }

    pub fn plugins(&self) -> Vec<PluginSummary> {
        self.plugins.clone()
    }

    pub fn runners(&self) -> Vec<RunnerSummary> {
        self.runners.clone()
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
        let task = request.into_task();
        let task_id = task.task_id.clone();
        self.tasks.track(&task_id);

        let mut runtime = self.runtime.lock();
        let submitted = runtime.dispatch(HostRuntimeCommand::SubmitTask(Box::new(task)));
        drop(runtime);

        let submitted = match submitted {
            Ok(reply) => reply,
            Err(error) => {
                self.tasks.forget(&task_id);
                self.emit_runtime_error_log(
                    "mutsuki_tauri_host.runtime",
                    "runtime task submit failed",
                    format!("{:?}", error.error()),
                );
                return Err(error.into());
            }
        };
        match submitted {
            HostRuntimeReply::TaskSubmitted(_) => {}
            _ => {
                self.tasks.forget(&task_id);
                return Err(HostError::Runtime("unexpected submit reply".into()));
            }
        }

        self.drain_observability()?;
        self.ensure_task_pump();
        Ok(FrontendTaskRun { task_id })
    }

    pub fn task_result(&self, request: TaskResultRequest) -> HostResult<FrontendTaskResult> {
        self.tasks.wait_result(&request.task_id)
    }

    pub fn cancel_task(&self, request: TaskCancelRequest) -> HostResult<String> {
        let mut runtime = self.runtime.lock();
        let reply = runtime.dispatch(HostRuntimeCommand::CancelTask(request.task_id.clone()));
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
            HostRuntimeReply::TaskCancelled(task_id) => {
                self.drain_observability()?;
                self.ensure_task_pump();
                Ok(task_id)
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

        let runtime = self.runtime.clone();
        let events = self.events.clone();
        let tasks = self.tasks.clone();
        let spawn_result = thread::Builder::new()
            .name("mutsuki-tauri-task-pump".into())
            .spawn(move || run_task_pump(runtime, events, tasks));

        if let Err(error) = spawn_result {
            self.tasks
                .fail_active(format!("failed to spawn task pump: {error}"));
        }
    }

    fn drain_observability(&self) -> HostResult<()> {
        drain_observability(&self.runtime, &self.events, &self.tasks).map_err(|error| {
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
        emit_log_record(
            &self.events,
            "error".into(),
            target.into(),
            message.into(),
            None,
            None,
            BTreeMap::from([("error".into(), json!(error.into()))]),
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
}

fn run_task_pump(
    runtime: Arc<Mutex<HostRuntime>>,
    events: Arc<mutsuki_tauri_bridge::EventHub>,
    tasks: Arc<TaskSupervisor>,
) {
    loop {
        let Some(active) = tasks.active_snapshot() else {
            return;
        };

        let snapshot = {
            let mut runtime = runtime.lock();
            let tick = runtime
                .dispatch(HostRuntimeCommand::TickOnce)
                .map_err(|error| format!("{:?}", error.error()));
            tick.and_then(|_| {
                let mut outcomes = Vec::new();
                for task_id in active {
                    let status = runtime.task_status(&task_id);
                    if status.is_none() {
                        outcomes.push((task_id, status, None));
                        continue;
                    }
                    let outcome = match runtime
                        .dispatch(HostRuntimeCommand::TaskOutcome(task_id.clone()))
                        .map_err(|error| format!("{:?}", error.error()))?
                    {
                        HostRuntimeReply::TaskOutcome(outcome) => outcome,
                        _ => return Err("unexpected task outcome reply".into()),
                    };
                    outcomes.push((task_id, status, outcome));
                }
                Ok(outcomes)
            })
        };

        match snapshot {
            Ok(outcomes) => {
                if let Err(error) = drain_observability(&runtime, &events, &tasks) {
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
                tasks.finish_tasks(outcomes);
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

        thread::sleep(Duration::from_millis(10));
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

fn drain_observability(
    runtime: &Arc<Mutex<HostRuntime>>,
    events: &Arc<mutsuki_tauri_bridge::EventHub>,
    tasks: &Arc<TaskSupervisor>,
) -> Result<(), String> {
    let (last_event_sequence, last_trace_span_index) = tasks.observe_cursor();
    let (core_events, next_trace_index, trace_spans) = {
        let mut runtime = runtime.lock();
        let core_events = runtime
            .events_after(last_event_sequence)
            .map_err(|error| format!("{:?}", error.error()))?;
        let (next_trace_index, trace_spans) = runtime
            .trace_spans_after(last_trace_span_index)
            .map_err(|error| format!("{:?}", error.error()))?;
        (core_events, next_trace_index, trace_spans)
    };
    tasks.ingest_runtime_events(core_events, events);
    tasks.ingest_trace_spans(next_trace_index, trace_spans, events);
    Ok(())
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
