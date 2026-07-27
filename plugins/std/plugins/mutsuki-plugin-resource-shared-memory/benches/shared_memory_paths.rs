use std::hint::black_box;
use std::time::Instant;

use mutsuki_plugin_resource_shared_memory::{
    SharedMemoryProviderConfig, SharedMemoryResourceProvider,
};
use mutsuki_runtime_contracts::ReadPlan;
use mutsuki_runtime_sdk::{ResourcePlanGateway, ResourceProviderGateway};
use serde_json::{Value, json};

const RESOURCE_BYTES: usize = 100 * 1024 * 1024;

fn main() {
    let provider = SharedMemoryResourceProvider::with_config(SharedMemoryProviderConfig {
        max_collect_bytes: RESOURCE_BYTES as u64,
        retained_generations: 2,
    });
    let resource = provider
        .create_blob_resource("benchmark.bytes.v1", vec![0x5a; RESOURCE_BYTES])
        .expect("create benchmark resource");
    let read = ReadPlan {
        plan_id: "benchmark.read".into(),
        resource,
        operation: "collect".into(),
        args: json!({"max_bytes": RESOURCE_BYTES}),
    };

    provider.reset_copy_metrics();
    let view_iterations = 10_000_u64;
    let started = Instant::now();
    for _ in 0..view_iterations {
        let view = provider.mapped_view(&read.resource).expect("mapped view");
        black_box(view.bytes()[0]);
        black_box(view.bytes()[RESOURCE_BYTES - 1]);
    }
    let view_elapsed = started.elapsed();
    let view_copied = provider.copy_metrics().mapped_view_copied_bytes;

    provider.reset_copy_metrics();
    let snapshot_iterations = 100_u64;
    let started = Instant::now();
    for index in 0..snapshot_iterations {
        let snapshot = provider
            .snapshot_read_plan(
                &read,
                "benchmark.snapshot",
                &format!("benchmark.snapshot.{index}"),
            )
            .expect("readonly snapshot");
        black_box(snapshot.snapshot_ref);
    }
    let snapshot_elapsed = started.elapsed();
    let snapshot_copied = provider.copy_metrics().snapshot_copied_bytes;

    provider.reset_copy_metrics();
    let collect_iterations = 3_u64;
    let started = Instant::now();
    for _ in 0..collect_iterations {
        let bytes = provider.collect_read_plan(&read).expect("collect");
        black_box(bytes);
    }
    let collect_elapsed = started.elapsed();
    let collect_copied = provider.copy_metrics().collect_copied_bytes;

    let report = json!({
        "resource_bytes": RESOURCE_BYTES,
        "cases": [
            benchmark_case("mapped_view", view_iterations, view_elapsed.as_nanos(), view_copied),
            benchmark_case(
                "readonly_snapshot",
                snapshot_iterations,
                snapshot_elapsed.as_nanos(),
                snapshot_copied,
            ),
            benchmark_case(
                "collect_owned",
                collect_iterations,
                collect_elapsed.as_nanos(),
                collect_copied,
            ),
        ]
    });
    println!("{report}");

    assert_eq!(view_copied, 0, "mapped view must not copy resource bytes");
    assert_eq!(
        snapshot_copied, 0,
        "readonly snapshot must not copy resource bytes"
    );
    assert_eq!(
        collect_copied,
        RESOURCE_BYTES as u64 * collect_iterations,
        "collect copy accounting must match owned output bytes"
    );
}

fn benchmark_case(name: &str, iterations: u64, elapsed_ns: u128, copied_bytes: u64) -> Value {
    json!({
        "name": name,
        "iterations": iterations,
        "elapsed_ns": elapsed_ns,
        "copied_bytes": copied_bytes,
        "copied_bytes_per_iteration": copied_bytes / iterations,
    })
}
