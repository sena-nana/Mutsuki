// This file is on the workspace `unsafe_code` exception list.
// Benchmarks measure allocator traffic, which requires implementing `GlobalAlloc`; that trait
// is `unsafe` by definition. Every operation is delegated unchanged to `System`.
#![allow(unsafe_code)]

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub struct CountingAllocator;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

// SAFETY: every method forwards its arguments unchanged to the `System` allocator, so the
// layout and pointer contract of `GlobalAlloc` is upheld by `System` itself. The only
// additions are relaxed atomic counters, which touch no allocator state.
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

#[derive(Clone, Debug)]
pub struct Sample {
    pub elapsed_ns: u128,
    pub simulated_wall_ns: u128,
    pub simulated_work_ns: u128,
    pub tasks: u64,
    pub continuations: u64,
    pub tool_routes: u64,
    pub max_tool_inflight: u64,
    pub retained_bytes: u64,
    pub post_warmup_growth_bytes: u64,
    pub output: Value,
    pub allocations: u64,
    pub allocated_bytes: u64,
}

#[derive(Serialize)]
pub struct RawCase {
    pub case_id: String,
    pub dimensions: Value,
    pub elapsed_ns: Vec<u128>,
    pub simulated_wall_ns: Vec<u128>,
    pub simulated_work_ns: Vec<u128>,
    pub orchestration_ns: Vec<u128>,
    pub tasks: Vec<u64>,
    pub continuations: Vec<u64>,
    pub tool_routes: Vec<u64>,
    pub max_tool_inflight: Vec<u64>,
    pub retained_bytes: Vec<u64>,
    pub post_warmup_growth_bytes: Vec<u64>,
    pub allocations: Vec<u64>,
    pub allocated_bytes: Vec<u64>,
    pub output_hash: String,
}

pub fn raw_case(case_id: impl Into<String>, dimensions: Value, samples: Vec<Sample>) -> RawCase {
    assert!(!samples.is_empty());
    let case_id = case_id.into();
    let hashes = samples
        .iter()
        .map(|sample| canonical_hash(&sample.output))
        .collect::<Vec<_>>();
    assert!(
        hashes.iter().all(|hash| hash == &hashes[0]),
        "benchmark case `{case_id}` produced non-deterministic output hashes: {hashes:?}"
    );
    RawCase {
        case_id,
        dimensions,
        elapsed_ns: samples.iter().map(|sample| sample.elapsed_ns).collect(),
        simulated_wall_ns: samples
            .iter()
            .map(|sample| sample.simulated_wall_ns)
            .collect(),
        simulated_work_ns: samples
            .iter()
            .map(|sample| sample.simulated_work_ns)
            .collect(),
        orchestration_ns: samples
            .iter()
            .map(|sample| sample.elapsed_ns.saturating_sub(sample.simulated_wall_ns))
            .collect(),
        tasks: samples.iter().map(|sample| sample.tasks).collect(),
        continuations: samples.iter().map(|sample| sample.continuations).collect(),
        tool_routes: samples.iter().map(|sample| sample.tool_routes).collect(),
        max_tool_inflight: samples
            .iter()
            .map(|sample| sample.max_tool_inflight)
            .collect(),
        retained_bytes: samples.iter().map(|sample| sample.retained_bytes).collect(),
        post_warmup_growth_bytes: samples
            .iter()
            .map(|sample| sample.post_warmup_growth_bytes)
            .collect(),
        allocations: samples.iter().map(|sample| sample.allocations).collect(),
        allocated_bytes: samples
            .iter()
            .map(|sample| sample.allocated_bytes)
            .collect(),
        output_hash: hashes[0].clone(),
    }
}

pub fn allocation_snapshot() -> (u64, u64) {
    (
        ALLOCATIONS.load(Ordering::Relaxed),
        ALLOCATED_BYTES.load(Ordering::Relaxed),
    )
}

pub fn allocation_delta(start: (u64, u64)) -> (u64, u64) {
    let end = allocation_snapshot();
    (end.0.saturating_sub(start.0), end.1.saturating_sub(start.1))
}

pub fn canonical_hash(value: &Value) -> String {
    let mut value = value.clone();
    normalize_event_timestamps(&mut value);
    hex::encode(Sha256::digest(serde_json::to_vec(&value).unwrap()))
}

fn normalize_event_timestamps(value: &mut Value) {
    match value {
        Value::Array(values) => values.iter_mut().for_each(normalize_event_timestamps),
        Value::Object(values) => {
            if values.contains_key("event_id")
                && values.contains_key("visibility")
                && values.contains_key("timestamp_unix_ms")
            {
                values.insert("timestamp_unix_ms".into(), Value::from(0));
            }
            values.values_mut().for_each(normalize_event_timestamps);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn output_hash_ignores_only_event_envelope_timestamps() {
        let first = json!({
            "meta": {
                "event_id": "turn-1:1",
                "visibility": "default",
                "timestamp_unix_ms": 10,
            },
            "expires_at_unix_ms": 10,
        });
        let later_event = json!({
            "meta": {
                "event_id": "turn-1:1",
                "visibility": "default",
                "timestamp_unix_ms": 20,
            },
            "expires_at_unix_ms": 10,
        });
        let different_business_time = json!({
            "meta": {
                "event_id": "turn-1:1",
                "visibility": "default",
                "timestamp_unix_ms": 20,
            },
            "expires_at_unix_ms": 20,
        });

        assert_eq!(canonical_hash(&first), canonical_hash(&later_event));
        assert_ne!(
            canonical_hash(&first),
            canonical_hash(&different_business_time)
        );
    }
}
