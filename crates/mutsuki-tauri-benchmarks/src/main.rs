use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use mutsuki_runtime_contracts::{
    CompletionBatch, EntryCompletion, ExecutionClass, InvocationMode, RunnerConcurrency,
    RunnerDescriptor, RunnerPurity, RunnerResult, Task, TaskBatch, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_tauri_bridge::{FrontendTaskRequest, FrontendTaskResult, TaskResultRequest};
use mutsuki_tauri_host::{MutsukiTauriConfig, MutsukiTauriHost, PathsConfig};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const PROTOCOL_ID: &str = "benchmark.echo";

#[tokio::main]
async fn main() {
    let output = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "target/mutsuki-benchmarks/tauri-bridge-resource-raw.json".into());
    let workspace = tempfile::tempdir().expect("benchmark tempdir");
    let host = Arc::new(
        MutsukiTauriHost::builder()
            .config(benchmark_config(workspace.path()))
            .runner(Box::new(EchoRunner::new()))
            .build()
            .expect("benchmark host"),
    );
    let mut cases = Vec::new();
    benchmark_bridge(&host, &mut cases);
    benchmark_resources(&host, &mut cases).await;
    let report = json!({
        "schema": "mutsuki.tauri.bridge-resource.raw/v1",
        "boundary": "executable JSON command/event adapter over embedded Host; excludes OS WebView IPC and rendering",
        "cases": cases,
        "rss_bytes": current_rss_bytes(),
    });
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).expect("output directory");
    }
    std::fs::write(&output, serde_json::to_vec_pretty(&report).unwrap()).expect("write raw report");
    println!("wrote {}", output.display());
    host.shutdown();
}

fn benchmark_bridge(host: &Arc<MutsukiTauriHost>, cases: &mut Vec<Value>) {
    for payload_bytes in [256_usize, 4 * 1024, 64 * 1024] {
        for concurrency in [1_usize, 16, 56] {
            let started = Instant::now();
            let cpu = cpu_time::ProcessTime::now();
            let mut request_bytes = 0_usize;
            let mut response_bytes = 0_usize;
            std::thread::scope(|scope| {
                let jobs = (0..concurrency)
                    .map(|index| {
                        let host = host.clone();
                        scope.spawn(move || {
                            invoke_equivalent(
                                &host,
                                FrontendTaskRequest {
                                    protocol_id: PROTOCOL_ID.into(),
                                    payload: json!({"payload": "x".repeat(payload_bytes)}),
                                    task_id: Some(format!(
                                        "bridge-{payload_bytes}-{concurrency}-{index}"
                                    )),
                                    trace_id: Some(format!("trace-{index}")),
                                    correlation_id: Some(format!("correlation-{index}")),
                                    idempotency_key: None,
                                    input_refs: Vec::new(),
                                    priority: 0,
                                    context: Default::default(),
                                },
                            )
                        })
                    })
                    .collect::<Vec<_>>();
                for job in jobs {
                    let (request, response) = job.join().expect("bridge worker");
                    request_bytes += request;
                    response_bytes += response;
                }
            });
            cases.push(json!({
                "case_id": "tauri.bridge.command-roundtrip",
                "dimensions": {"concurrency": concurrency, "payload_bytes": payload_bytes},
                "latency_ns": started.elapsed().as_nanos(),
                "cpu_time_ns": cpu.elapsed().as_nanos(),
                "request_frame_bytes": request_bytes,
                "response_frame_bytes": response_bytes,
                "operations": concurrency,
            }));
        }
    }

    for batch_size in [1_usize, 32] {
        let batch = TaskBatch {
            batch_id: format!("bridge-batch-{batch_size}"),
            tick_id: None,
            tasks: (0..batch_size)
                .map(|index| {
                    Task::new(
                        format!("batch-{batch_size}-{index}"),
                        PROTOCOL_ID,
                        json!({}),
                    )
                })
                .collect(),
            resource_plan: None,
        };
        let encoded = serde_json::to_vec(&batch).expect("serialize batch");
        let decoded: TaskBatch = serde_json::from_slice(&encoded).expect("deserialize batch");
        let started = Instant::now();
        let handles = host.submit_batch(decoded).expect("submit bridge batch");
        for handle in &handles {
            host.task_result(TaskResultRequest {
                task_id: handle.task_id.clone(),
            })
            .expect("batch task completes");
        }
        cases.push(json!({
            "case_id": "tauri.bridge.submit-batch",
            "dimensions": {"batch_size": batch_size},
            "latency_ns": started.elapsed().as_nanos(),
            "request_frame_bytes": encoded.len(),
            "operations": batch_size,
        }));
    }

    let mut receiver = host.event_hub().subscribe();
    let started = Instant::now();
    let _ = invoke_equivalent(
        host,
        FrontendTaskRequest {
            protocol_id: PROTOCOL_ID.into(),
            payload: json!({}),
            task_id: Some("event-notification".into()),
            trace_id: None,
            correlation_id: None,
            idempotency_key: None,
            input_refs: Vec::new(),
            priority: 0,
            context: Default::default(),
        },
    );
    let mut retained = std::collections::VecDeque::with_capacity(256);
    let mut event_frame_bytes = 0_usize;
    while let Ok(event) = receiver.try_recv() {
        let bytes = serde_json::to_vec(&event).expect("serialize frontend event");
        event_frame_bytes += bytes.len();
        if retained.len() == 256 {
            retained.pop_front();
        }
        retained.push_back(bytes);
    }
    cases.push(json!({
        "case_id": "tauri.bridge.frontend-notification",
        "dimensions": {"retention_capacity": 256},
        "latency_ns": started.elapsed().as_nanos(),
        "event_frame_bytes": event_frame_bytes,
        "retained_events": retained.len(),
    }));

    let refresh_started = Instant::now();
    let snapshots = host.task_snapshots().expect("task state batch refresh");
    let frontend_snapshots = snapshots
        .iter()
        .map(|snapshot| {
            json!({
                "task_id": snapshot.task_id,
                "protocol_id": snapshot.protocol_id,
                "status": snapshot.status,
                "trace_id": snapshot.trace_id,
                "correlation_id": snapshot.correlation_id,
            })
        })
        .collect::<Vec<_>>();
    let refresh_frame =
        serde_json::to_vec(&frontend_snapshots).expect("serialize task state batch");
    cases.push(json!({
        "case_id": "tauri.bridge.task-state-batch-refresh",
        "dimensions": {"tasks": snapshots.len()},
        "latency_ns": refresh_started.elapsed().as_nanos(),
        "response_frame_bytes": refresh_frame.len(),
        "operations": snapshots.len(),
    }));

    for (case_id, frame) in [
        (
            "tauri.bridge.events-cursor-page",
            serde_json::to_vec(&host.events_after(0, 56).expect("event cursor page")).unwrap(),
        ),
        (
            "tauri.bridge.traces-cursor-page",
            serde_json::to_vec(&host.trace_spans_after(0, 56).expect("trace cursor page")).unwrap(),
        ),
    ] {
        let started = Instant::now();
        let decoded: Value = serde_json::from_slice(&frame).expect("deserialize cursor page");
        cases.push(json!({
            "case_id": case_id,
            "dimensions": {"page_limit": 56},
            "latency_ns": started.elapsed().as_nanos(),
            "response_frame_bytes": frame.len(),
            "page_items": decoded.get("items").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        }));
    }
}

async fn benchmark_resources(host: &Arc<MutsukiTauriHost>, cases: &mut Vec<Value>) {
    for size in [1024_usize * 1024, 64 * 1024 * 1024] {
        let create_started = Instant::now();
        let resource = host
            .resource_store()
            .create_blob(
                "application/octet-stream",
                vec![0x5a; size],
                Some("application/octet-stream".into()),
            )
            .await
            .expect("create benchmark resource");
        let create_ns = create_started.elapsed().as_nanos();
        let open_started = Instant::now();
        let preview = host
            .create_preview_handle(&resource.ref_id)
            .expect("preview handle");
        let descriptor_frame = serde_json::to_vec(&resource).unwrap();
        let preview_frame = serde_json::to_vec(&preview).unwrap();
        assert!(descriptor_frame.len() < 8 * 1024);
        assert!(preview_frame.len() < 8 * 1024);
        assert!(host.read_resource_bytes(&resource.ref_id).await.is_err());
        let open_ns = open_started.elapsed().as_nanos();

        let read_started = Instant::now();
        let chunk = host
            .read_resource_chunk(&resource.ref_id, 0, 64 * 1024)
            .await
            .expect("bounded resource chunk");
        let chunk_frame = serde_json::to_vec(&chunk).unwrap();
        assert!(chunk_frame.len() < 256 * 1024);
        let read_ns = read_started.elapsed().as_nanos();

        let concurrent_started = Instant::now();
        let reads = (0..8)
            .map(|index| {
                let host = host.clone();
                let ref_id = resource.ref_id.clone();
                tokio::spawn(async move {
                    host.read_resource_chunk(&ref_id, (index * 4096) as u64, 4096)
                        .await
                        .expect("concurrent resource chunk")
                })
            })
            .collect::<Vec<_>>();
        for read in reads {
            read.await.expect("resource read task");
        }
        let concurrent_ns = concurrent_started.elapsed().as_nanos();

        let preview_started = Instant::now();
        let (preview_bytes, _) = host
            .resource_store()
            .read_preview_token(&preview.token)
            .expect("custom protocol backing reads resource");
        assert_eq!(preview_bytes.len(), size);
        let preview_ns = preview_started.elapsed().as_nanos();
        let release_started = Instant::now();
        host.release_preview_handle(&preview.token)
            .expect("release preview");
        assert!(
            host.resource_store()
                .read_preview_token(&preview.token)
                .is_err()
        );
        let release_ns = release_started.elapsed().as_nanos();

        for (operation, latency_ns) in [
            ("create", create_ns),
            ("open", open_ns),
            ("read-chunk", read_ns),
            ("concurrent-read", concurrent_ns),
            ("preview-protocol", preview_ns),
            ("release", release_ns),
        ] {
            cases.push(json!({
                "case_id": format!("tauri.resource.{operation}"),
                "dimensions": {"resource_bytes": size, "chunk_bytes": 64 * 1024},
                "latency_ns": latency_ns,
                "descriptor_frame_bytes": descriptor_frame.len(),
                "preview_frame_bytes": preview_frame.len(),
                "chunk_frame_bytes": chunk_frame.len(),
                "content_in_command_or_event_json": false,
            }));
        }
    }
}

fn invoke_equivalent(host: &MutsukiTauriHost, request: FrontendTaskRequest) -> (usize, usize) {
    let request_bytes = serde_json::to_vec(&request).expect("serialize invoke request");
    let decoded = serde_json::from_slice(&request_bytes).expect("deserialize invoke request");
    let result: FrontendTaskResult = host.call(decoded).expect("invoke-equivalent call");
    let response = serde_json::to_vec(&result).expect("serialize invoke response");
    (request_bytes.len(), response.len())
}

struct EchoRunner {
    descriptor: RunnerDescriptor,
}

impl EchoRunner {
    fn new() -> Self {
        Self {
            descriptor: RunnerDescriptor {
                runner_id: "benchmark.echo.runner".into(),
                plugin_id: "benchmark.echo".into(),
                plugin_generation: 1,
                accepted_protocol_ids: vec![PROTOCOL_ID.into()],
                purity: RunnerPurity::Pure,
                execution_class: ExecutionClass::Cpu,
                invocation_mode: InvocationMode::SyncExclusive,
                concurrency: RunnerConcurrency::Exclusive,
                input_schema: json!({}),
                output_schema: json!({}),
                batch: Default::default(),
                payload: Default::default(),
                resources: Default::default(),
                ordering: Default::default(),
                control: Default::default(),
                metadata: BTreeMap::new(),
                contract_surfaces: vec![format!("task_protocol:{PROTOCOL_ID}")],
            },
        }
    }
}

impl Runner for EchoRunner {
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
                    .unwrap();
                let mut result = RunnerResult::completed(task.task_id.clone());
                result.output = Some(json!({"echo": task.payload}));
                EntryCompletion {
                    entry_id: entry.entry_id.clone(),
                    task_id: entry.task_id.clone(),
                    result: Some(result),
                    error: None,
                }
            })
            .collect();
        Ok(CompletionBatch::from_results(&batch, results))
    }
}

fn benchmark_config(root: &std::path::Path) -> MutsukiTauriConfig {
    let mut config = MutsukiTauriConfig::for_app("TauriPerformanceBenchmark");
    config.paths = PathsConfig {
        app_data_dir: root.into(),
        config_dir: root.join("config"),
        data_dir: root.join("data"),
        cache_dir: root.join("cache"),
        logs_dir: root.join("logs"),
        plugins_dir: root.join("plugins"),
        resources_dir: root.join("resources"),
        runners_dir: root.join("runners"),
    };
    config
}

fn current_rss_bytes() -> u64 {
    if cfg!(windows) {
        return std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-Process -Id {}).WorkingSet64", std::process::id()),
            ])
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0);
    }
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(0)
        * 1024
}

#[allow(dead_code)]
fn stable_hash(value: &Value) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(value).unwrap()))
}
