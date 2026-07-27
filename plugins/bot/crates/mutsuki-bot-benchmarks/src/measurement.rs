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

pub struct Sample {
    pub elapsed_ns: u128,
    pub cpu_time_ns: u128,
    pub idle_cpu_time_ns: u128,
    pub simulated_platform_ns: u128,
    pub events: u64,
    pub queue_depth: u64,
    pub dropped: u64,
    pub deferred: u64,
    pub retried: u64,
    pub fairness: f64,
    pub duplicate_executions: u64,
    pub retained_units: u64,
    pub output: Value,
    pub allocations: u64,
    pub allocated_bytes: u64,
}

#[derive(Serialize)]
pub struct RawCase {
    pub case_id: String,
    pub dimensions: Value,
    pub elapsed_ns: Vec<u128>,
    pub cpu_time_ns: Vec<u128>,
    pub idle_cpu_time_ns: Vec<u128>,
    pub simulated_platform_ns: Vec<u128>,
    pub bot_orchestration_ns: Vec<u128>,
    pub events: Vec<u64>,
    pub queue_depth: Vec<u64>,
    pub dropped: Vec<u64>,
    pub deferred: Vec<u64>,
    pub retried: Vec<u64>,
    pub fairness: Vec<f64>,
    pub duplicate_executions: Vec<u64>,
    pub retained_units: Vec<u64>,
    pub allocations: Vec<u64>,
    pub allocated_bytes: Vec<u64>,
    pub output_hash: String,
}

pub fn raw_case(case_id: impl Into<String>, dimensions: Value, samples: Vec<Sample>) -> RawCase {
    assert!(!samples.is_empty());
    let hashes = samples
        .iter()
        .map(|sample| canonical_hash(&sample.output))
        .collect::<Vec<_>>();
    assert!(hashes.iter().all(|hash| hash == &hashes[0]));
    RawCase {
        case_id: case_id.into(),
        dimensions,
        elapsed_ns: samples.iter().map(|sample| sample.elapsed_ns).collect(),
        cpu_time_ns: samples.iter().map(|sample| sample.cpu_time_ns).collect(),
        idle_cpu_time_ns: samples
            .iter()
            .map(|sample| sample.idle_cpu_time_ns)
            .collect(),
        simulated_platform_ns: samples
            .iter()
            .map(|sample| sample.simulated_platform_ns)
            .collect(),
        bot_orchestration_ns: samples
            .iter()
            .map(|sample| {
                sample
                    .elapsed_ns
                    .saturating_sub(sample.simulated_platform_ns)
            })
            .collect(),
        events: samples.iter().map(|sample| sample.events).collect(),
        queue_depth: samples.iter().map(|sample| sample.queue_depth).collect(),
        dropped: samples.iter().map(|sample| sample.dropped).collect(),
        deferred: samples.iter().map(|sample| sample.deferred).collect(),
        retried: samples.iter().map(|sample| sample.retried).collect(),
        fairness: samples.iter().map(|sample| sample.fairness).collect(),
        duplicate_executions: samples
            .iter()
            .map(|sample| sample.duplicate_executions)
            .collect(),
        retained_units: samples.iter().map(|sample| sample.retained_units).collect(),
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

#[cfg(unix)]
pub fn process_cpu_time_ns() -> u128 {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let status = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut value) };
    assert_eq!(status, 0, "clock_gettime(CLOCK_PROCESS_CPUTIME_ID) failed");
    (value.tv_sec as u128) * 1_000_000_000 + value.tv_nsec as u128
}

#[cfg(windows)]
pub fn process_cpu_time_ns() -> u128 {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetCurrentProcess, GetProcessTimes},
    };

    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let status = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    };
    assert_ne!(status, 0, "GetProcessTimes failed");
    let ticks =
        |value: FILETIME| ((value.dwHighDateTime as u128) << 32) | value.dwLowDateTime as u128;
    (ticks(kernel) + ticks(user)) * 100
}

pub fn canonical_hash(value: &Value) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(value).unwrap()))
}
