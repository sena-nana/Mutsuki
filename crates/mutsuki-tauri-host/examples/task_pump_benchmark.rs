use cpu_time::ProcessTime;
use mutsuki_runtime_contracts::{
    CancelPolicy, CompletionBatch, EntryCompletion, ExecutionClass, ResourceAccess, ResourceId,
    ResourceLifetime, ResourceRef, ResourceSealState, ResourceSemantic, RunnerDescriptor,
    RunnerMode, RunnerPurity, RunnerResult, RunnerSideEffect, RunnerStatus, Task, TaskAwait,
    TaskBatch, TaskHandle, TaskStatus, TaskStepContinuation, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_host::{HostRuntimeConfig, RunnerLimits};
use mutsuki_tauri_host::{MutsukiTauriConfig, MutsukiTauriHost, PathsConfig};
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PROTOCOL_ID: &str = "benchmark.waiting";
const IDLE_WINDOW: Duration = Duration::from_millis(500);

#[derive(Serialize)]
struct BenchmarkReport {
    schema: &'static str,
    generated_at_unix_ms: u128,
    os: &'static str,
    arch: &'static str,
    idle_window_ms: u128,
    scenarios: Vec<ScenarioReport>,
}

#[derive(Serialize)]
struct ScenarioReport {
    active_tasks: usize,
    setup_latency_ms: f64,
    idle_wall_ms: f64,
    idle_cpu_ms: f64,
    idle_actor_commands: u64,
    idle_task_status_queries: u64,
    idle_task_state_batch_queries: u64,
    completion_latency_ms: f64,
    completion_actor_commands: u64,
}

fn main() {
    let scenarios = [1, 100, 1000].into_iter().map(run_scenario).collect();
    let report = BenchmarkReport {
        schema: "mutsuki.tauri.task_pump_benchmark.v1",
        generated_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        idle_window_ms: IDLE_WINDOW.as_millis(),
        scenarios,
    };
    let json = serde_json::to_string_pretty(&report).expect("benchmark report serializes");
    if let Some(output) = std::env::args_os().nth(1) {
        let output = PathBuf::from(output);
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).expect("benchmark output directory creates");
        }
        std::fs::write(&output, format!("{json}\n")).expect("benchmark report writes");
    }
    println!("{json}");
}

fn run_scenario(active_tasks: usize) -> ScenarioReport {
    let workspace = BenchmarkWorkspace::new(active_tasks);
    let host = MutsukiTauriHost::builder()
        .config(workspace.config.clone())
        .runtime_config(HostRuntimeConfig {
            default_runner_limits: RunnerLimits {
                max_waiting: active_tasks,
                max_inflight: active_tasks,
                ..RunnerLimits::default()
            },
            ..HostRuntimeConfig::default()
        })
        .runner(Box::new(WaitingRunner::new(active_tasks)))
        .build()
        .expect("benchmark host builds");
    let setup_started = Instant::now();
    let handles = host
        .submit_batch(TaskBatch {
            batch_id: format!("benchmark-waiting-{active_tasks}"),
            tick_id: None,
            tasks: (0..active_tasks)
                .map(|index| {
                    Task::new(
                        format!("bench-parent:{active_tasks}:{index}"),
                        PROTOCOL_ID,
                        json!({}),
                    )
                })
                .collect(),
            resource_plan: None,
        })
        .expect("benchmark tasks submit");
    wait_for_waiting(&host, active_tasks);
    let setup_latency_ms = setup_started.elapsed().as_secs_f64() * 1000.0;
    settle_task_pump(&host);

    let metrics_before = host.runtime_metrics();
    let cpu_started = ProcessTime::now();
    let wall_started = Instant::now();
    std::thread::sleep(IDLE_WINDOW);
    let idle_wall_ms = wall_started.elapsed().as_secs_f64() * 1000.0;
    let idle_cpu_ms = cpu_started.elapsed().as_secs_f64() * 1000.0;
    let metrics_after_idle = host.runtime_metrics();

    let completion_started = Instant::now();
    host.cancel_task_handle(handles[0].clone())
        .expect("benchmark task cancels");
    host.task_result(mutsuki_tauri_bridge::TaskResultRequest {
        task_id: handles[0].task_id.clone(),
    })
    .expect("cancelled task result resolves");
    let completion_latency_ms = completion_started.elapsed().as_secs_f64() * 1000.0;
    let metrics_after_completion = host.runtime_metrics();
    host.shutdown();

    ScenarioReport {
        active_tasks,
        setup_latency_ms,
        idle_wall_ms,
        idle_cpu_ms,
        idle_actor_commands: metrics_after_idle
            .actor_commands
            .saturating_sub(metrics_before.actor_commands),
        idle_task_status_queries: metrics_after_idle
            .task_status_queries
            .saturating_sub(metrics_before.task_status_queries),
        idle_task_state_batch_queries: metrics_after_idle
            .task_state_batch_queries
            .saturating_sub(metrics_before.task_state_batch_queries),
        completion_latency_ms,
        completion_actor_commands: metrics_after_completion
            .actor_commands
            .saturating_sub(metrics_after_idle.actor_commands),
    }
}

fn wait_for_waiting(host: &MutsukiTauriHost, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let waiting = host
            .task_snapshots()
            .expect("benchmark task snapshots")
            .into_iter()
            .filter(|snapshot| {
                snapshot.task_id.starts_with("bench-parent:")
                    && !snapshot.task_id.ends_with(":child")
                    && snapshot.status == TaskStatus::Waiting
            })
            .count();
        if waiting == expected {
            return;
        }
        assert!(Instant::now() < deadline, "waiting tasks did not settle");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn settle_task_pump(host: &MutsukiTauriHost) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let before = host.runtime_metrics();
        std::thread::sleep(Duration::from_millis(50));
        let after = host.runtime_metrics();
        if before.actor_commands == after.actor_commands {
            return;
        }
        assert!(Instant::now() < deadline, "task pump did not settle");
    }
}

struct WaitingRunner {
    descriptor: RunnerDescriptor,
}

impl WaitingRunner {
    fn new(batch_size: usize) -> Self {
        let mut descriptor = runner_descriptor();
        descriptor.batch.mode = RunnerMode::NativeBatch;
        descriptor.batch.preferred_batch_size = batch_size;
        descriptor.batch.max_batch_entries = batch_size;
        descriptor.batch.side_effect = RunnerSideEffect::None;
        Self { descriptor }
    }
}

impl Runner for WaitingRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        let tasks = match batch.row_payload_tasks() {
            Ok(tasks) => tasks,
            Err(error) => return Ok(CompletionBatch::from_error(&batch, error)),
        };
        let results = batch
            .entries
            .iter()
            .map(|entry| {
                let task = tasks
                    .iter()
                    .find(|task| task.task_id == entry.task_id)
                    .expect("benchmark batch task exists");
                EntryCompletion {
                    entry_id: entry.entry_id.clone(),
                    task_id: entry.task_id.clone(),
                    result: Some(waiting_result(task)),
                    error: None,
                }
            })
            .collect();
        Ok(CompletionBatch::from_results(&batch, results))
    }
}

fn waiting_result(task: &Task) -> RunnerResult {
    let child_id = format!("{}:child", task.task_id);
    let mut child = Task::new(&child_id, PROTOCOL_ID, json!({}));
    child.ready_at_step = Some(u64::MAX / 4);
    let continuation_ref = format!("continuation:{}", task.task_id);
    RunnerResult {
        task_id: task.task_id.clone(),
        deltas: Vec::new(),
        events: Vec::new(),
        tasks: vec![child],
        effects: Vec::new(),
        values: Vec::new(),
        resources: Vec::new(),
        task_await: Some(TaskAwait {
            parent_task_id: task.task_id.clone(),
            child: TaskHandle {
                task_id: child_id,
                protocol_id: PROTOCOL_ID.into(),
                target_binding_id: None,
                cancel_policy: CancelPolicy::Cascade,
                trace_id: task.trace_id.clone(),
                correlation_id: task.correlation_id.clone(),
            },
            continuation: TaskStepContinuation {
                continuation: ResourceRef {
                    ref_id: continuation_ref.clone(),
                    resource_id: ResourceId {
                        kind_id: "continuation".into(),
                        slot_id: continuation_ref,
                        generation: 1,
                        version: 1,
                    },
                    semantic: ResourceSemantic::FrozenValue,
                    provider_id: "benchmark".into(),
                    resource_kind: "continuation".into(),
                    schema: "benchmark.continuation.v1".into(),
                    version: 1,
                    generation: 1,
                    access: ResourceAccess::Inline,
                    size_hint: None,
                    content_hash: None,
                    lifetime: ResourceLifetime::BorrowedUntilTaskEnd,
                    lease: None,
                    seal_state: ResourceSealState::Sealed,
                },
                wake: None,
                reason: Some("task pump benchmark".into()),
            },
            cancel_policy: CancelPolicy::Cascade,
        }),
        status: RunnerStatus::Waiting,
    }
}

fn runner_descriptor() -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: "benchmark.waiting.runner".into(),
        plugin_id: "benchmark.waiting".into(),
        plugin_generation: 1,
        accepted_protocol_ids: vec![PROTOCOL_ID.into()],
        purity: RunnerPurity::Pure,
        execution_class: ExecutionClass::Io,
        input_schema: json!({ "type": "object" }),
        output_schema: json!({ "type": "object" }),
        batch: Default::default(),
        payload: Default::default(),
        resources: Default::default(),
        ordering: Default::default(),
        control: Default::default(),
        metadata: BTreeMap::new(),
        contract_surfaces: vec![format!("task_protocol:{PROTOCOL_ID}")],
    }
}

struct BenchmarkWorkspace {
    root: PathBuf,
    config: MutsukiTauriConfig,
}

impl BenchmarkWorkspace {
    fn new(active_tasks: usize) -> Self {
        let root = std::env::temp_dir().join(format!(
            "mutsuki-tauri-task-pump-bench-{active_tasks}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let mut config = MutsukiTauriConfig::for_app(format!("TaskPumpBench{active_tasks}"));
        config.paths = benchmark_paths(&root);
        Self { root, config }
    }
}

impl Drop for BenchmarkWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn benchmark_paths(root: &Path) -> PathsConfig {
    PathsConfig {
        app_data_dir: root.into(),
        config_dir: root.join("config"),
        data_dir: root.join("data"),
        cache_dir: root.join("cache"),
        logs_dir: root.join("logs"),
        plugins_dir: root.join("plugins"),
        resources_dir: root.join("resources"),
        runners_dir: root.join("runners"),
    }
}
