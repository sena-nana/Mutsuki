use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::bounded;
use serde_json::json;

const LOGICAL_CORES: usize = 16;
const BLOCKING_THREADS: usize = 4;
const JOBS: usize = 200_000;
const SAMPLES: usize = 5;
const WORK_ITERATIONS: usize = 256;
const QUEUE_CAPACITY: usize = 65_536;

#[derive(Clone, Copy, Default)]
struct Usage {
    voluntary: i64,
    involuntary: i64,
}

struct Sample {
    elapsed: Duration,
    usage: Usage,
}

fn main() {
    let legacy_threads = LOGICAL_CORES * 3 + BLOCKING_THREADS;
    let optimized_threads = LOGICAL_CORES + BLOCKING_THREADS;
    let legacy = median_sample(|| run_legacy(legacy_threads));
    let optimized = median_sample(|| run_bounded(optimized_threads));
    let legacy_throughput = JOBS as f64 / legacy.elapsed.as_secs_f64();
    let optimized_throughput = JOBS as f64 / optimized.elapsed.as_secs_f64();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": "mutsuki.worker-pool.issue13.v1",
            "logical_cores": LOGICAL_CORES,
            "jobs_per_sample": JOBS,
            "samples": SAMPLES,
            "queue_capacity": QUEUE_CAPACITY,
            "legacy": {
                "topology": "3xcpu_plus_blocking",
                "queue": "std_mpsc_arc_mutex_receiver",
                "worker_threads": legacy_threads,
                "median_elapsed_ms": legacy.elapsed.as_secs_f64() * 1000.0,
                "throughput_jobs_per_second": legacy_throughput,
                "voluntary_context_switches": legacy.usage.voluntary,
                "involuntary_context_switches": legacy.usage.involuntary,
            },
            "optimized": {
                "topology": "shared_compute_plus_bounded_blocking",
                "queue": "crossbeam_bounded_multi_consumer",
                "worker_threads": optimized_threads,
                "median_elapsed_ms": optimized.elapsed.as_secs_f64() * 1000.0,
                "throughput_jobs_per_second": optimized_throughput,
                "voluntary_context_switches": optimized.usage.voluntary,
                "involuntary_context_switches": optimized.usage.involuntary,
            },
            "delta": {
                "worker_thread_reduction_percent":
                    (legacy_threads - optimized_threads) as f64 / legacy_threads as f64 * 100.0,
                "throughput_change_percent":
                    (optimized_throughput / legacy_throughput - 1.0) * 100.0,
                "voluntary_context_switch_change_percent": percent_change(
                    legacy.usage.voluntary,
                    optimized.usage.voluntary,
                ),
                "involuntary_context_switch_change_percent": percent_change(
                    legacy.usage.involuntary,
                    optimized.usage.involuntary,
                ),
            }
        }))
        .expect("benchmark result must serialize")
    );
}

fn median_sample(mut run: impl FnMut() -> Sample) -> Sample {
    let mut samples = (0..SAMPLES).map(|_| run()).collect::<Vec<_>>();
    samples.sort_by_key(|sample| sample.elapsed);
    samples.swap_remove(SAMPLES / 2)
}

fn run_legacy(threads: usize) -> Sample {
    let before = usage();
    let started = Instant::now();
    let completed = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::channel::<usize>();
    let receiver = Arc::new(Mutex::new(receiver));
    let handles = (0..threads)
        .map(|_| {
            let receiver = receiver.clone();
            let completed = completed.clone();
            thread::spawn(move || {
                loop {
                    let job = receiver.lock().expect("legacy receiver poisoned").recv();
                    if job.is_err() {
                        break;
                    }
                    do_work(job.expect("checked above"));
                    completed.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect::<Vec<_>>();
    for job in 0..JOBS {
        sender.send(job).expect("legacy queue disconnected");
    }
    drop(sender);
    for handle in handles {
        handle.join().expect("legacy worker panicked");
    }
    assert_eq!(completed.load(Ordering::Relaxed), JOBS);
    Sample {
        elapsed: started.elapsed(),
        usage: usage_delta(before, usage()),
    }
}

fn run_bounded(threads: usize) -> Sample {
    let before = usage();
    let started = Instant::now();
    let completed = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = bounded::<usize>(QUEUE_CAPACITY);
    let handles = (0..threads)
        .map(|_| {
            let receiver = receiver.clone();
            let completed = completed.clone();
            thread::spawn(move || {
                while let Ok(job) = receiver.recv() {
                    do_work(job);
                    completed.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect::<Vec<_>>();
    for job in 0..JOBS {
        sender.send(job).expect("bounded queue disconnected");
    }
    drop(sender);
    for handle in handles {
        handle.join().expect("bounded worker panicked");
    }
    assert_eq!(completed.load(Ordering::Relaxed), JOBS);
    Sample {
        elapsed: started.elapsed(),
        usage: usage_delta(before, usage()),
    }
}

fn percent_change(before: i64, after: i64) -> Option<f64> {
    (before != 0).then(|| (after as f64 / before as f64 - 1.0) * 100.0)
}

fn do_work(job: usize) {
    let mut value = job as u64 ^ 0x9e37_79b9_7f4a_7c15;
    for _ in 0..WORK_ITERATIONS {
        value = value
            .rotate_left(7)
            .wrapping_mul(0xbf58_476d_1ce4_e5b9)
            .wrapping_add(0x94d0_49bb_1331_11eb);
    }
    std::hint::black_box(value);
}

fn usage_delta(before: Usage, after: Usage) -> Usage {
    Usage {
        voluntary: after.voluntary - before.voluntary,
        involuntary: after.involuntary - before.involuntary,
    }
}

#[cfg(unix)]
fn usage() -> Usage {
    let mut value = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage initializes the provided rusage structure for RUSAGE_SELF on success.
    let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, value.as_mut_ptr()) };
    assert_eq!(status, 0, "getrusage failed");
    // SAFETY: status == 0 guarantees the structure was initialized.
    let value = unsafe { value.assume_init() };
    Usage {
        voluntary: value.ru_nvcsw,
        involuntary: value.ru_nivcsw,
    }
}

#[cfg(not(unix))]
fn usage() -> Usage {
    Usage::default()
}
