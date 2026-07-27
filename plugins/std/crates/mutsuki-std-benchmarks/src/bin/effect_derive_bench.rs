//! Pure Effect-facade derivation microbench for StdPlugins Issue #6.
//!
//! Measures time and allocation for 1 / 16 / 256 entries across
//! 1 KiB / 64 KiB / 1 MiB payloads. Mode `baseline` deep-clones payload and
//! always emits queued events; mode `optimized` uses the shared-payload helper
//! with quiet observation (no queued-event allocation).

use std::{
    alloc::{GlobalAlloc, Layout, System},
    env,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

use mutsuki_plugin_io_fs::{EFFECT_RUNNER_ID, FS_READ_PROTOCOL};
use mutsuki_runtime_contracts::{DomainEvent, RunnerResult, Task};
use mutsuki_std_effect::{EffectDeriveOptions, EffectObservation, derive_effect_task};
use serde_json::{Value, json};

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

fn reset_alloc() {
    ALLOCATIONS.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
}

fn take_alloc() -> (u64, u64) {
    (
        ALLOCATIONS.swap(0, Ordering::Relaxed),
        ALLOCATED_BYTES.swap(0, Ordering::Relaxed),
    )
}

fn sample_payload(bytes: usize) -> Value {
    json!({
        "path": "/tmp/issue6-bench.txt",
        "allowlist": ["/tmp"],
        "content": "x".repeat(bytes),
    })
}

fn baseline_derive(task: &Task) -> RunnerResult {
    let mut effect_task = Task::new(
        format!("{}:effect", task.task_id),
        "effect.mutsuki.fs.read",
        task.payload.to_value(),
    );
    effect_task.runner_hint = Some(EFFECT_RUNNER_ID.into());
    effect_task.trace_id = task.trace_id.clone();
    effect_task.correlation_id = task
        .correlation_id
        .clone()
        .or_else(|| Some(task.task_id.clone()));
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.tasks.push(effect_task);
    result.events.push(DomainEvent {
        event_id: format!("event:{}:queued", task.task_id),
        kind: task.protocol_id.clone(),
        payload: json!({"effect_task_id": format!("{}:effect", task.task_id)}),
    });
    result
}

fn optimized_derive(task: &Task) -> RunnerResult {
    derive_effect_task(
        task,
        "effect.mutsuki.fs.read",
        EFFECT_RUNNER_ID,
        EffectDeriveOptions {
            observation: EffectObservation::Quiet,
            ..EffectDeriveOptions::default()
        },
    )
}

fn measure(
    label: &str,
    entries: usize,
    payload_bytes: usize,
    iterations: usize,
    derive: impl Fn(&Task) -> RunnerResult,
) {
    let payload = sample_payload(payload_bytes);
    let tasks: Vec<Task> = (0..entries)
        .map(|index| {
            let mut task = Task::new(format!("task-{index}"), FS_READ_PROTOCOL, payload.clone());
            task.trace_id = Some(format!("trace-{index}"));
            task
        })
        .collect();

    // Warmup
    for task in &tasks {
        std::hint::black_box(derive(task));
    }

    let mut total_ns = 0u128;
    let mut total_allocs = 0u64;
    let mut total_bytes = 0u64;
    for _ in 0..iterations {
        reset_alloc();
        let started = Instant::now();
        for task in &tasks {
            std::hint::black_box(derive(task));
        }
        total_ns += started.elapsed().as_nanos();
        let (allocs, bytes) = take_alloc();
        total_allocs += allocs;
        total_bytes += bytes;
    }

    let entry_ops = (entries * iterations) as u64;
    println!(
        "{label}\tentries={entries}\tpayload={payload_bytes}\tns_per_entry={:.1}\tallocs_per_entry={:.3}\tbytes_per_entry={:.1}",
        total_ns as f64 / entry_ops as f64,
        total_allocs as f64 / entry_ops as f64,
        total_bytes as f64 / entry_ops as f64,
    );
}

fn main() {
    let mode = env::args().nth(1).unwrap_or_else(|| "both".into());
    let iterations: usize = env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(50);

    let cases = [
        (1usize, 1024),
        (16, 1024),
        (256, 1024),
        (1, 64 * 1024),
        (16, 64 * 1024),
        (256, 64 * 1024),
        (1, 1024 * 1024),
        (16, 1024 * 1024),
    ];

    if mode == "baseline" || mode == "both" {
        for (entries, payload_bytes) in cases {
            measure(
                "baseline",
                entries,
                payload_bytes,
                iterations,
                baseline_derive,
            );
        }
    }
    if mode == "optimized" || mode == "both" {
        for (entries, payload_bytes) in cases {
            measure(
                "optimized",
                entries,
                payload_bytes,
                iterations,
                optimized_derive,
            );
        }
    }
}
