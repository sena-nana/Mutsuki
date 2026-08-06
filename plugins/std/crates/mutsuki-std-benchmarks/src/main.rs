use std::{
    alloc::{GlobalAlloc, Layout, System},
    collections::BTreeMap,
    env, fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use async_trait::async_trait;
struct CountingAllocator;
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let pointer = unsafe { System.realloc(pointer, layout, size) };
        if !pointer.is_null() {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(size as u64, Ordering::Relaxed);
        }
        pointer
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

use mutsuki_plugin_config_permission::{ConfigPermissionRunner, PERMISSION_CHECK_PROTOCOL};
use mutsuki_plugin_db_sqlite::{
    EFFECT_DB_EXECUTE_PROTOCOL, EFFECT_DB_QUERY_PROTOCOL, EFFECT_DB_TRANSACTION_PROTOCOL,
    EFFECT_RUNNER_ID as DB_RUNNER_ID, SqliteEffectRunner,
};
use mutsuki_plugin_dev_mock::{DevMockRunner, ECHO_PROTOCOL, RUNNER_ID as DEV_RUNNER_ID};
use mutsuki_plugin_io_fs::{
    EFFECT_FS_READ_PROTOCOL, EFFECT_RUNNER_ID as FS_RUNNER_ID, IoFsEffectRunner,
};
use mutsuki_plugin_io_http_client::{
    EFFECT_HTTP_REQUEST_PROTOCOL, EFFECT_RUNNER_ID as HTTP_RUNNER_ID, FetchedHttpResponse,
    HttpEffectHandler, HttpGateway, HttpGatewayError,
};
use mutsuki_plugin_observe_log::{LOG_EMIT_PROTOCOL, ObserveLogRunner, RUNNER_ID as LOG_RUNNER_ID};
use mutsuki_plugin_resource_memory::MemoryResourceProvider;
use mutsuki_plugin_resource_shared_memory::SharedMemoryResourceProvider;
use mutsuki_plugin_workflow_broadcast::{
    BROADCAST_EMIT_PROTOCOL, RUNNER_ID as BROADCAST_RUNNER_ID, WorkflowBroadcastRunner,
};
use mutsuki_plugin_workflow_linear::{
    LINEAR_RUN_PROTOCOL, RUNNER_ID as LINEAR_RUNNER_ID, WorkflowLinearRunner,
};
use mutsuki_protocol_http::{HttpRequest, HttpResponseMetadata};
use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    BatchEntry, BatchPayload, CommandPlan, CompletionBatch, DispatchLane, ExportPlan,
    OrderingRequirement, PatchDescriptor, PlanReceipt, ReadPlan, ResourceRef, RunnerContext,
    SnapshotDescriptor, StreamPlan, Task, WorkBatch, WorkResourcePlan, WritePlan,
};
use mutsuki_runtime_core::{AsyncBatchHandler, Runner, RuntimeFailure};
use mutsuki_runtime_sdk::{ResourcePlanGateway, ResourceProviderGateway, ResourceRegistryGateway};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

#[derive(Serialize)]
struct RawReport {
    schema_version: &'static str,
    workload_version: &'static str,
    mode: String,
    fixed_seed: u64,
    network_boundary: &'static str,
    cases: Vec<RawCase>,
    correctness: BTreeMap<String, u64>,
}

#[derive(Serialize)]
struct RawCase {
    case_id: &'static str,
    dimensions: Value,
    samples_ns: Vec<u128>,
    units: u64,
    output_hash: String,
    handler_only: bool,
    bytes: u64,
    allocations: u64,
    allocated_bytes: u64,
}

fn main() {
    let mode = env::var("MUTSUKI_BENCH_MODE").unwrap_or_else(|_| "smoke".into());
    assert!(matches!(mode.as_str(), "smoke" | "reference"));
    let samples = if mode == "smoke" { 3 } else { 30 };
    let mut correctness = BTreeMap::from([
        ("hash_mismatches".into(), 0),
        ("runner_errors".into(), 0),
        ("public_network_requests".into(), 0),
    ]);
    let mut cases = workflow_cases(samples, &mut correctness);
    for size in if mode == "smoke" {
        vec![1024 * 1024]
    } else {
        vec![1024 * 1024, 64 * 1024 * 1024]
    } {
        cases.push(memory_read_case(size, samples));
        cases.push(memory_cow_case(size, samples));
    }
    cases.push(shared_memory_case(1024 * 1024, samples));
    let temp = TempDir::new().unwrap();
    cases.push(fs_case(&temp, samples, &mut correctness));
    cases.push(sqlite_case(&temp, samples, &mut correctness));
    cases.push(http_case(samples, &mut correctness));
    let observe_tasks = (0..64)
        .map(|index| {
            Task::new(
                format!("log-{index:02}"),
                LOG_EMIT_PROTOCOL,
                json!({"message": "fixed", "index": index}),
            )
        })
        .collect::<Vec<_>>();
    cases.push(runner_tasks_case(
        "std.observe.log",
        json!({"events": 64}),
        samples,
        64,
        &mut ObserveLogRunner::new(),
        LOG_RUNNER_ID,
        &observe_tasks,
        &mut correctness,
    ));
    cases.push(runner_case(
        "std.config.permission",
        json!({"matrix": "allow"}),
        samples,
        1,
        &mut ConfigPermissionRunner::new(),
        mutsuki_plugin_config_permission::RUNNER_ID,
        Task::new(
            "permission",
            PERMISSION_CHECK_PROTOCOL,
            json!({
                "request": {"kind": "fs", "path": "/fixture/file"},
                "grants": {"fs_paths": ["/fixture/"]}
            }),
        ),
        &mut correctness,
    ));
    cases.push(runner_case(
        "std.dev.mock",
        json!({"fixture": "echo"}),
        samples,
        1,
        &mut DevMockRunner::new(),
        DEV_RUNNER_ID,
        Task::new("dev", ECHO_PROTOCOL, json!({"seed": 1_297_435_713_u64})),
        &mut correctness,
    ));
    assert!(correctness.values().all(|value| *value == 0));
    let report = RawReport {
        schema_version: "mutsuki.std.performance.raw/v1",
        workload_version: "mutsuki.performance.std-workloads/v1",
        mode,
        fixed_seed: 1_297_435_713,
        network_boundary: "injected-deterministic-http-gateway",
        cases,
        correctness,
    };
    let output = env::var_os("MUTSUKI_BENCH_OUTPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/mutsuki-benchmarks/std-plugins.raw.json"));
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("{}", output.display());
}

fn workflow_cases(samples: usize, correctness: &mut BTreeMap<String, u64>) -> Vec<RawCase> {
    let mut cases = Vec::new();
    for scale in [1, 8, 64] {
        let children = (0..scale)
            .map(|index| json!({"protocol_id": "mutsuki.dev.echo", "payload": {"index": index}}))
            .collect::<Vec<_>>();
        cases.push(runner_case(
            "std.workflow.linear",
            json!({"steps": scale}),
            samples,
            scale,
            &mut WorkflowLinearRunner::new(),
            LINEAR_RUNNER_ID,
            Task::new(
                "workflow",
                LINEAR_RUN_PROTOCOL,
                json!({"sequence_id": "fixed", "steps": children}),
            ),
            correctness,
        ));
        let targets = (0..scale)
            .map(|index| json!({"protocol_id": "mutsuki.dev.echo", "payload": {"index": index}}))
            .collect::<Vec<_>>();
        cases.push(runner_case(
            "std.workflow.broadcast",
            json!({"fan_out": scale}),
            samples,
            scale,
            &mut WorkflowBroadcastRunner::new(),
            BROADCAST_RUNNER_ID,
            Task::new(
                "broadcast",
                BROADCAST_EMIT_PROTOCOL,
                json!({"mode": "fixed", "targets": targets}),
            ),
            correctness,
        ));
    }
    cases
}

#[allow(clippy::too_many_arguments)]
fn runner_case(
    case_id: &'static str,
    dimensions: Value,
    samples: usize,
    units: u64,
    runner: &mut dyn Runner,
    runner_id: &str,
    task: Task,
    correctness: &mut BTreeMap<String, u64>,
) -> RawCase {
    runner_tasks_case(
        case_id,
        dimensions,
        samples,
        units,
        runner,
        runner_id,
        std::slice::from_ref(&task),
        correctness,
    )
}

#[allow(clippy::too_many_arguments)]
fn runner_tasks_case(
    case_id: &'static str,
    dimensions: Value,
    samples: usize,
    units: u64,
    runner: &mut dyn Runner,
    runner_id: &str,
    tasks: &[Task],
    correctness: &mut BTreeMap<String, u64>,
) -> RawCase {
    let allocation_start = allocation_snapshot();
    let mut samples_ns = Vec::with_capacity(samples);
    let mut expected = None;
    for _ in 0..samples {
        let started = Instant::now();
        let result = runner.run_batch(context(case_id), batch_tasks(runner_id, tasks));
        samples_ns.push(started.elapsed().as_nanos());
        match result {
            Ok(completion) => {
                let hash = completion_hash(&completion);
                if expected.as_ref().is_some_and(|value| value != &hash) {
                    *correctness.get_mut("hash_mismatches").unwrap() += 1;
                }
                expected.get_or_insert(hash);
                if completion.results.iter().any(|entry| entry.error.is_some()) {
                    *correctness.get_mut("runner_errors").unwrap() += 1;
                }
            }
            Err(_) => *correctness.get_mut("runner_errors").unwrap() += 1,
        }
    }
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    RawCase {
        case_id,
        dimensions,
        samples_ns,
        units,
        output_hash: expected.unwrap_or_else(|| canonical_hash(&Value::Null)),
        handler_only: true,
        bytes: 0,
        allocations,
        allocated_bytes,
    }
}

fn memory_read_case(size: usize, samples: usize) -> RawCase {
    let provider = MemoryResourceProvider::new();
    let source = deterministic_bytes(size);
    let expected = digest(&source);
    let resource = provider
        .create_blob_resource("fixture.bytes.v1", source)
        .unwrap();
    let plan = ReadPlan {
        plan_id: "benchmark-read".into(),
        resource,
        operation: "collect".into(),
        args: Value::Null,
    };
    let mut samples_ns = Vec::new();
    let allocation_start = allocation_snapshot();
    for _ in 0..samples {
        let started = Instant::now();
        let bytes = provider.collect_read_plan(&plan).unwrap();
        samples_ns.push(started.elapsed().as_nanos());
        assert_eq!(digest(&bytes), expected);
    }
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    RawCase {
        case_id: "std.resource.memory.read",
        dimensions: json!({"bytes": size, "access": "shared-read"}),
        samples_ns,
        units: size as u64,
        output_hash: canonical_hash(&json!({"sha256": expected})),
        handler_only: true,
        bytes: size as u64,
        allocations,
        allocated_bytes,
    }
}

fn memory_cow_case(size: usize, samples: usize) -> RawCase {
    let mut samples_ns = Vec::new();
    let allocation_start = allocation_snapshot();
    for index in 0..samples {
        let provider = MemoryResourceProvider::new();
        let resource = provider
            .create_cow_state_resource(
                "fixture.state",
                "fixture.bytes.v1",
                deterministic_bytes(size),
            )
            .unwrap();
        let write = WritePlan {
            plan_id: format!("write-{index}"),
            resource: resource.clone(),
            base_version: resource.version,
            conflict_policy: "replace".into(),
            patch: PatchDescriptor {
                patch_id: format!("patch-{index}"),
                target_ref: resource.clone(),
                base_version: resource.version,
                conflict_policy: "replace".into(),
                operations: json!({"replace": true}),
            },
            returning: None,
        };
        let started = Instant::now();
        let receipt = provider
            .commit_write_plan(&write, deterministic_bytes(size))
            .unwrap();
        samples_ns.push(started.elapsed().as_nanos());
        assert_eq!(receipt.new_version, Some(2));
    }
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    RawCase {
        case_id: "std.resource.memory.cow",
        dimensions: json!({"bytes": size}),
        samples_ns,
        units: size as u64,
        output_hash: canonical_hash(&json!({"new_version": 2})),
        handler_only: true,
        bytes: size as u64,
        allocations,
        allocated_bytes,
    }
}

fn shared_memory_case(size: usize, samples: usize) -> RawCase {
    let provider = SharedMemoryResourceProvider::new();
    let resource = provider
        .create_blob_resource("fixture.bytes.v1", deterministic_bytes(size))
        .unwrap();
    let mut samples_ns = Vec::new();
    let mut hash = String::new();
    let allocation_start = allocation_snapshot();
    for _ in 0..samples {
        let started = Instant::now();
        let view = provider.mapped_view(&resource).unwrap();
        hash = digest(view.bytes());
        drop(view);
        samples_ns.push(started.elapsed().as_nanos());
    }
    assert_eq!(provider.copy_metrics().mapped_view_copied_bytes, 0);
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    RawCase {
        case_id: "std.resource.shared-memory",
        dimensions: json!({"bytes": size, "operations": "descriptor-open-read-release"}),
        samples_ns,
        units: size as u64,
        output_hash: canonical_hash(&json!({"sha256": hash})),
        handler_only: true,
        bytes: size as u64,
        allocations,
        allocated_bytes,
    }
}

fn fs_case(temp: &TempDir, samples: usize, correctness: &mut BTreeMap<String, u64>) -> RawCase {
    let tree = temp.path().join("fixture-tree");
    fs::create_dir_all(&tree).unwrap();
    let tasks = (0..8)
        .map(|index| {
            let path = tree.join(format!("fixture-{index:02}.txt"));
            let content = format!("mutsuki-fixed-filesystem-{index:02}");
            fs::write(&path, content).unwrap();
            Task::new(
                format!("fs-{index:02}"),
                EFFECT_FS_READ_PROTOCOL,
                json!({"path": path, "allowlist": [temp.path()]}),
            )
        })
        .collect::<Vec<_>>();
    let mut case = runner_tasks_case(
        "std.io.fs.read",
        json!({"files": 8, "boundary": "temporary-directory-tree"}),
        samples,
        8,
        &mut IoFsEffectRunner::new(),
        FS_RUNNER_ID,
        &tasks,
        correctness,
    );
    case.output_hash = canonical_hash(&json!({
        "files": (0..8)
            .map(|index| json!({
                "name": format!("fixture-{index:02}.txt"),
                "content": format!("mutsuki-fixed-filesystem-{index:02}")
            }))
            .collect::<Vec<_>>()
    }));
    case
}

fn sqlite_case(temp: &TempDir, samples: usize, correctness: &mut BTreeMap<String, u64>) -> RawCase {
    let path = temp.path().join("fixture.sqlite");
    let allowlist = json!([temp.path()]);
    let mut runner = SqliteEffectRunner::new();
    run_setup(
        &mut runner,
        Task::new(
            "create",
            EFFECT_DB_EXECUTE_PROTOCOL,
            json!({
                "path": path,
                "db_path_allowlist": allowlist,
                "sql": "CREATE TABLE fixture(id INTEGER PRIMARY KEY, value TEXT)",
                "params": []
            }),
        ),
    );
    let statements = (0..64)
        .map(|index| {
            json!({
                "sql": "INSERT INTO fixture(id, value) VALUES (?1, ?2)",
                "params": [index, format!("value-{index:02}")]
            })
        })
        .collect::<Vec<_>>();
    run_setup(
        &mut runner,
        Task::new(
            "seed",
            EFFECT_DB_TRANSACTION_PROTOCOL,
            json!({
                "path": path,
                "db_path_allowlist": allowlist,
                "statements": statements
            }),
        ),
    );
    let mut case = runner_case(
        "std.db.sqlite.query",
        json!({"seed_rows": 64}),
        samples,
        64,
        &mut runner,
        DB_RUNNER_ID,
        Task::new(
            "query",
            EFFECT_DB_QUERY_PROTOCOL,
            json!({
                "path": path,
                "db_path_allowlist": allowlist,
                "sql": "SELECT id, value FROM fixture ORDER BY id",
                "params": []
            }),
        ),
        correctness,
    );
    case.output_hash = canonical_hash(&json!({
        "rows": (0..64)
            .map(|index| json!({"id": index, "value": format!("value-{index:02}")}))
            .collect::<Vec<_>>()
    }));
    case
}

fn run_setup(runner: &mut dyn Runner, task: Task) {
    let completion = runner
        .run_batch(context("setup"), batch(DB_RUNNER_ID, &task))
        .unwrap();
    assert!(completion.results[0].error.is_none());
}

fn http_case(samples: usize, correctness: &mut BTreeMap<String, u64>) -> RawCase {
    let mut samples_ns = Vec::new();
    let mut expected = None;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let resources = Arc::new(BenchmarkResources(Arc::new(MemoryResourceProvider::new())));
    let handler = HttpEffectHandler::new(
        Arc::new(BenchmarkHttpGateway),
        resources,
        "benchmark.memory",
    );
    let allocation_start = allocation_snapshot();
    for _ in 0..samples {
        let task = Task::new(
            "http",
            EFFECT_HTTP_REQUEST_PROTOCOL,
            serde_json::to_value(HttpRequest::get("https://benchmark.invalid/fixture")).unwrap(),
        );
        let started = Instant::now();
        let completion = runtime
            .block_on(handler.run_batch(context("http"), batch(HTTP_RUNNER_ID, &task)))
            .unwrap();
        samples_ns.push(started.elapsed().as_nanos());
        let event = &completion.results[0].result.as_ref().unwrap().events[0];
        let hash = canonical_hash(&json!({
            "status": event.payload["metadata"]["status"],
            "body_bytes": event.payload["metadata"]["body_bytes"]
        }));
        if expected.as_ref().is_some_and(|value| value != &hash) {
            *correctness.get_mut("hash_mismatches").unwrap() += 1;
        }
        expected.get_or_insert(hash);
    }
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    RawCase {
        case_id: "std.io.http-client",
        dimensions: json!({
            "gateway": "injected-deterministic",
            "allocation_boundary": "async-handler-plus-resource-write"
        }),
        samples_ns,
        units: 1,
        output_hash: expected.unwrap(),
        handler_only: true,
        bytes: 13,
        allocations,
        allocated_bytes,
    }
}

struct BenchmarkHttpGateway;

#[async_trait]
impl HttpGateway for BenchmarkHttpGateway {
    async fn execute(
        &self,
        request: HttpRequest,
        _request_body: Option<Vec<u8>>,
    ) -> Result<FetchedHttpResponse, HttpGatewayError> {
        Ok(FetchedHttpResponse {
            metadata: HttpResponseMetadata {
                status: 200,
                final_url: request.url,
                headers: BTreeMap::new(),
                body_bytes: 13,
                redirects_followed: 0,
            },
            body: b"mutsuki-fixed".to_vec(),
            peak_buffered_bytes: 13,
        })
    }
}

struct BenchmarkResources(Arc<MemoryResourceProvider>);

impl ResourcePlanGateway for BenchmarkResources {
    fn collect_read_plan(&self, plan: &ReadPlan) -> Result<Vec<u8>, RuntimeFailure> {
        self.0.collect_read_plan(plan)
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> Result<SnapshotDescriptor, RuntimeFailure> {
        self.0.snapshot_read_plan(plan, kind_id, schema)
    }

    fn open_stream_plan(&self, plan: &ReadPlan) -> Result<StreamPlan, RuntimeFailure> {
        self.0.open_stream_plan(plan)
    }

    fn execute_export_plan(&self, plan: &ExportPlan) -> Result<PlanReceipt, RuntimeFailure> {
        self.0.execute_export_plan(plan)
    }

    fn commit_write_plan(
        &self,
        plan: &WritePlan,
        bytes: Vec<u8>,
    ) -> Result<PlanReceipt, RuntimeFailure> {
        self.0.commit_write_plan(plan, bytes)
    }

    fn execute_command_plan(&self, plan: &CommandPlan) -> Result<PlanReceipt, RuntimeFailure> {
        self.0.execute_command_plan(plan)
    }

    fn execute_command_batch(
        &self,
        batch: &CommandBatch,
    ) -> Result<Vec<PlanReceipt>, RuntimeFailure> {
        self.0.execute_command_batch(batch)
    }

    fn execute_saga_plan(&self, saga: &SagaPlan) -> Result<Vec<PlanReceipt>, RuntimeFailure> {
        self.0.execute_saga_plan(saga)
    }
}

impl ResourceRegistryGateway for BenchmarkResources {
    fn open_resource_descriptor(&self, ref_id: &str) -> Result<ResourceRef, RuntimeFailure> {
        Err(RuntimeFailure::new(
            mutsuki_runtime_contracts::RuntimeError::new(
                mutsuki_runtime_contracts::ERR_RESOURCE_NOT_FOUND,
                "std.benchmark.http",
                format!("resource.{ref_id}.not_opened"),
            ),
        ))
    }

    fn create_blob_resource(
        &self,
        _provider_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> Result<ResourceRef, RuntimeFailure> {
        self.0.create_blob_resource(schema, bytes)
    }

    fn create_cow_state_resource(
        &self,
        _provider_id: &str,
        kind_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> Result<ResourceRef, RuntimeFailure> {
        self.0.create_cow_state_resource(kind_id, schema, bytes)
    }

    fn create_capability_resource(
        &self,
        _provider_id: &str,
        kind_id: &str,
        schema: &str,
    ) -> Result<ResourceRef, RuntimeFailure> {
        self.0.create_capability_resource(kind_id, schema)
    }
}

fn context(id: &str) -> RunnerContext {
    RunnerContext::new(1, 1, "benchmark", Vec::<String>::new(), id).with_batch(id, 1)
}

fn batch(runner_id: &str, task: &Task) -> WorkBatch {
    batch_tasks(runner_id, std::slice::from_ref(task))
}

fn batch_tasks(runner_id: &str, tasks: &[Task]) -> WorkBatch {
    WorkBatch {
        batch_id: format!("batch:{}", tasks[0].task_id),
        tick_id: "tick:1".into(),
        batch_key: runner_id.into(),
        entries: tasks
            .iter()
            .enumerate()
            .map(|(index, task)| BatchEntry {
                entry_id: task.task_id.clone(),
                task_id: task.task_id.clone(),
                trace_id: None,
                parent_id: None,
                payload_index: index,
                resource_requirement_indices: Vec::new(),
                cancel_index: Some(index),
                deadline_tick: None,
                priority: 0,
                lane: DispatchLane::Normal,
                ordering: OrderingRequirement::None,
            })
            .collect(),
        payload: BatchPayload::from_task_refs(tasks),
        resource_plan: WorkResourcePlan::empty(),
        task_leases: Vec::new(),
    }
}

fn completion_hash(completion: &CompletionBatch) -> String {
    digest(&serde_json::to_vec(completion).unwrap())
}

fn canonical_hash(value: &Value) -> String {
    digest(&serde_json::to_vec(value).unwrap())
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn deterministic_bytes(size: usize) -> Vec<u8> {
    (0..size)
        .map(|index| u8::try_from(index % 251).unwrap())
        .collect()
}

fn allocation_snapshot() -> (u64, u64) {
    (
        ALLOCATIONS.load(Ordering::Relaxed),
        ALLOCATED_BYTES.load(Ordering::Relaxed),
    )
}

fn allocation_delta(start: (u64, u64)) -> (u64, u64) {
    let end = allocation_snapshot();
    (end.0.saturating_sub(start.0), end.1.saturating_sub(start.1))
}
