use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{TrySendError, bounded};
use serde_json::{Value, json};

const BLOCKING_THREADS: usize = 4;
const JOBS: usize = 200_000;
const SAMPLES: usize = 5;
const WORK_ITERATIONS: usize = 256;
const QUEUE_CAPACITY: usize = 65_536;

#[derive(Clone, Copy, Default)]
struct Usage {
    total: Option<i64>,
    voluntary: Option<i64>,
    involuntary: Option<i64>,
}

struct Sample {
    elapsed: Duration,
    usage: Usage,
}

#[derive(Clone, Copy)]
struct Summary {
    median: f64,
    p95: f64,
    p99: f64,
    mad: f64,
    min: f64,
    max: f64,
}

fn main() {
    let host_logical_cores = thread::available_parallelism().map_or(1, usize::from);
    let logical_cores = requested_logical_cores().unwrap_or(host_logical_cores);
    let legacy_threads = logical_cores * 3 + BLOCKING_THREADS;
    let optimized_threads = logical_cores + BLOCKING_THREADS;
    let (legacy, optimized) = interleaved_samples(legacy_threads, optimized_threads);
    let legacy_elapsed = summarize_durations(&legacy);
    let optimized_elapsed = summarize_durations(&optimized);
    let legacy_total = summarize_usage(&legacy, |usage| usage.total);
    let optimized_total = summarize_usage(&optimized, |usage| usage.total);
    let legacy_voluntary = summarize_usage(&legacy, |usage| usage.voluntary);
    let optimized_voluntary = summarize_usage(&optimized, |usage| usage.voluntary);
    let legacy_involuntary = summarize_usage(&legacy, |usage| usage.involuntary);
    let optimized_involuntary = summarize_usage(&optimized, |usage| usage.involuntary);
    let legacy_throughput = JOBS as f64 / (legacy_elapsed.median / 1_000.0);
    let optimized_throughput = JOBS as f64 / (optimized_elapsed.median / 1_000.0);

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": "mutsuki.worker-pool.issue13.v3",
            "host": {
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "available_logical_cores": host_logical_cores,
            },
            "benchmark": {
                "logical_cores": logical_cores,
                "jobs_per_sample": JOBS,
                "samples": SAMPLES,
                "sample_order": "alternating_legacy_optimized",
                "work_iterations_per_job": WORK_ITERATIONS,
                "queue_capacity": QUEUE_CAPACITY,
            },
            "legacy": result_json(
                "3xcpu_plus_blocking",
                "std_mpsc_arc_mutex_receiver",
                legacy_threads,
                &legacy,
                legacy_elapsed,
                legacy_throughput,
                legacy_total,
                legacy_voluntary,
                legacy_involuntary,
            ),
            "optimized": result_json(
                "shared_compute_plus_bounded_blocking",
                "crossbeam_bounded_multi_consumer",
                optimized_threads,
                &optimized,
                optimized_elapsed,
                optimized_throughput,
                optimized_total,
                optimized_voluntary,
                optimized_involuntary,
            ),
            "delta": {
                "worker_thread_reduction_percent":
                    (legacy_threads - optimized_threads) as f64 / legacy_threads as f64 * 100.0,
                "median_elapsed_change_percent":
                    percent_change(legacy_elapsed.median, optimized_elapsed.median),
                "throughput_change_percent":
                    percent_change(legacy_throughput, optimized_throughput),
                "total_context_switch_change_percent":
                    percent_change_optional(legacy_total, optimized_total),
                "voluntary_context_switch_change_percent":
                    percent_change_optional(legacy_voluntary, optimized_voluntary),
                "involuntary_context_switch_change_percent":
                    percent_change_optional(legacy_involuntary, optimized_involuntary),
            }
        }))
        .expect("benchmark result must serialize")
    );
}

fn requested_logical_cores() -> Option<usize> {
    let mut args = std::env::args().skip(1);
    let arg = args.next()?;
    assert_eq!(arg, "--logical-cores", "unsupported argument {arg:?}");
    let value = args
        .next()
        .expect("--logical-cores requires a positive integer")
        .parse::<usize>()
        .expect("--logical-cores requires a positive integer");
    assert!(value > 0, "--logical-cores requires a positive integer");
    assert!(args.next().is_none(), "unexpected extra benchmark argument");
    Some(value)
}

fn interleaved_samples(
    legacy_threads: usize,
    optimized_threads: usize,
) -> (Vec<Sample>, Vec<Sample>) {
    let mut legacy = Vec::with_capacity(SAMPLES);
    let mut optimized = Vec::with_capacity(SAMPLES);
    for index in 0..SAMPLES {
        if index % 2 == 0 {
            legacy.push(run_legacy(legacy_threads));
            optimized.push(run_bounded(optimized_threads));
        } else {
            optimized.push(run_bounded(optimized_threads));
            legacy.push(run_legacy(legacy_threads));
        }
    }
    (legacy, optimized)
}

#[allow(clippy::too_many_arguments)]
fn result_json(
    topology: &str,
    queue: &str,
    worker_threads: usize,
    samples: &[Sample],
    elapsed: Summary,
    throughput: f64,
    total: Option<Summary>,
    voluntary: Option<Summary>,
    involuntary: Option<Summary>,
) -> Value {
    json!({
        "topology": topology,
        "queue": queue,
        "worker_threads": worker_threads,
        "elapsed_ms": summary_json(elapsed),
        "elapsed_ms_samples": samples
            .iter()
            .map(|sample| sample.elapsed.as_secs_f64() * 1_000.0)
            .collect::<Vec<_>>(),
        "throughput_jobs_per_second": throughput,
        "context_switches": {
            "available": total.is_some(),
            "total": total.map(summary_json),
            "total_samples": usage_samples(samples, |usage| usage.total),
            "voluntary": voluntary.map(summary_json),
            "voluntary_samples": usage_samples(samples, |usage| usage.voluntary),
            "involuntary": involuntary.map(summary_json),
            "involuntary_samples": usage_samples(samples, |usage| usage.involuntary),
        },
    })
}

fn summary_json(summary: Summary) -> Value {
    json!({
        "median": summary.median,
        "p95": summary.p95,
        "p99": summary.p99,
        "mad": summary.mad,
        "min": summary.min,
        "max": summary.max,
    })
}

fn summarize_durations(samples: &[Sample]) -> Summary {
    summarize(
        samples
            .iter()
            .map(|sample| sample.elapsed.as_secs_f64() * 1_000.0)
            .collect(),
    )
}

fn summarize_usage(samples: &[Sample], field: impl Fn(Usage) -> Option<i64>) -> Option<Summary> {
    let values = samples
        .iter()
        .map(|sample| field(sample.usage).map(|value| value as f64))
        .collect::<Option<Vec<_>>>()?;
    Some(summarize(values))
}

fn usage_samples(samples: &[Sample], field: impl Fn(Usage) -> Option<i64>) -> Option<Vec<i64>> {
    samples.iter().map(|sample| field(sample.usage)).collect()
}

fn summarize(mut values: Vec<f64>) -> Summary {
    assert!(!values.is_empty(), "benchmark summary requires samples");
    values.sort_by(f64::total_cmp);
    let median = percentile(&values, 0.5);
    let mut deviations = values
        .iter()
        .map(|value| (value - median).abs())
        .collect::<Vec<_>>();
    deviations.sort_by(f64::total_cmp);
    Summary {
        median,
        p95: percentile(&values, 0.95),
        p99: percentile(&values, 0.99),
        mad: percentile(&deviations, 0.5),
        min: values[0],
        max: values[values.len() - 1],
    }
}

fn percentile(values: &[f64], quantile: f64) -> f64 {
    let index = ((values.len() - 1) as f64 * quantile).ceil() as usize;
    values[index]
}

fn run_legacy(threads: usize) -> Sample {
    let before = usage();
    let started = Instant::now();
    let completed = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(Barrier::new(threads + 1));
    let release = Arc::new(Barrier::new(threads + 1));
    let (sender, receiver) = mpsc::channel::<usize>();
    let receiver = Arc::new(Mutex::new(receiver));
    let handles = (0..threads)
        .map(|_| {
            let receiver = receiver.clone();
            let completed = completed.clone();
            let finished = finished.clone();
            let release = release.clone();
            thread::spawn(move || {
                loop {
                    let job = receiver.lock().expect("legacy receiver poisoned").recv();
                    if job.is_err() {
                        break;
                    }
                    do_work(job.expect("checked above"));
                    completed.fetch_add(1, Ordering::Relaxed);
                }
                finished.wait();
                release.wait();
            })
        })
        .collect::<Vec<_>>();
    for job in 0..JOBS {
        sender.send(job).expect("legacy queue disconnected");
    }
    drop(sender);
    finished.wait();
    let elapsed = started.elapsed();
    let after = usage();
    release.wait();
    for handle in handles {
        handle.join().expect("legacy worker panicked");
    }
    assert_eq!(completed.load(Ordering::Relaxed), JOBS);
    Sample {
        elapsed,
        usage: usage_delta(before, after),
    }
}

fn run_bounded(threads: usize) -> Sample {
    let before = usage();
    let started = Instant::now();
    let completed = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(Barrier::new(threads + 1));
    let release = Arc::new(Barrier::new(threads + 1));
    let (sender, receiver) = bounded::<usize>(QUEUE_CAPACITY);
    let handles = (0..threads)
        .map(|_| {
            let receiver = receiver.clone();
            let completed = completed.clone();
            let finished = finished.clone();
            let release = release.clone();
            thread::spawn(move || {
                while let Ok(job) = receiver.recv() {
                    do_work(job);
                    completed.fetch_add(1, Ordering::Relaxed);
                }
                finished.wait();
                release.wait();
            })
        })
        .collect::<Vec<_>>();
    for job in 0..JOBS {
        let mut pending = job;
        loop {
            match sender.try_send(pending) {
                Ok(()) => break,
                Err(TrySendError::Full(job)) => {
                    pending = job;
                    std::hint::spin_loop();
                }
                Err(TrySendError::Disconnected(_)) => panic!("bounded queue disconnected"),
            }
        }
    }
    drop(sender);
    finished.wait();
    let elapsed = started.elapsed();
    let after = usage();
    release.wait();
    for handle in handles {
        handle.join().expect("bounded worker panicked");
    }
    assert_eq!(completed.load(Ordering::Relaxed), JOBS);
    Sample {
        elapsed,
        usage: usage_delta(before, after),
    }
}

fn percent_change(before: f64, after: f64) -> Option<f64> {
    (before != 0.0).then(|| (after / before - 1.0) * 100.0)
}

fn percent_change_optional(before: Option<Summary>, after: Option<Summary>) -> Option<f64> {
    percent_change(before?.median, after?.median)
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
        total: after
            .total
            .zip(before.total)
            .map(|(after, before)| after - before),
        voluntary: after
            .voluntary
            .zip(before.voluntary)
            .map(|(after, before)| after - before),
        involuntary: after
            .involuntary
            .zip(before.involuntary)
            .map(|(after, before)| after - before),
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
        total: Some(value.ru_nvcsw + value.ru_nivcsw),
        voluntary: Some(value.ru_nvcsw),
        involuntary: Some(value.ru_nivcsw),
    }
}

#[cfg(windows)]
fn usage() -> Usage {
    Usage {
        total: Some(windows_context_switches()),
        ..Usage::default()
    }
}

#[cfg(not(any(unix, windows)))]
fn usage() -> Usage {
    Usage::default()
}

#[cfg(windows)]
fn windows_context_switches() -> i64 {
    use std::mem::{size_of, size_of_val};
    use std::slice;

    use windows_sys::Wdk::System::SystemInformation::{
        NtQuerySystemInformation, SystemProcessInformation,
    };
    use windows_sys::Win32::System::WindowsProgramming::{
        SYSTEM_PROCESS_INFORMATION, SYSTEM_THREAD_INFORMATION,
    };

    const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xc000_0004_u32 as i32;
    let mut byte_capacity = 64 * 1024_u32;
    let process_id = std::process::id() as usize;

    loop {
        let words = (byte_capacity as usize).div_ceil(size_of::<usize>());
        let mut buffer = vec![0_usize; words];
        let mut required = 0_u32;
        // SAFETY: the word buffer is aligned for every parsed system structure, its byte size is
        // supplied exactly, and the kernel reports how much space is required on mismatch.
        let status = unsafe {
            NtQuerySystemInformation(
                SystemProcessInformation,
                buffer.as_mut_ptr().cast(),
                size_of_val(buffer.as_slice()) as u32,
                &mut required,
            )
        };
        if status == STATUS_INFO_LENGTH_MISMATCH {
            byte_capacity = required.max(byte_capacity.saturating_mul(2));
            continue;
        }
        assert!(status >= 0, "NtQuerySystemInformation failed: {status:#x}");

        let base = buffer.as_ptr().cast::<u8>();
        let mut offset = 0_usize;
        loop {
            // SAFETY: each offset is supplied by the kernel and the buffer is suitably aligned.
            let process = unsafe { &*base.add(offset).cast::<SYSTEM_PROCESS_INFORMATION>() };
            if process.UniqueProcessId as usize == process_id {
                // SYSTEM_PROCESS_INFORMATION is immediately followed by NumberOfThreads entries.
                // SAFETY: the kernel populated this record and its declared thread count.
                let threads = unsafe {
                    slice::from_raw_parts(
                        (process as *const SYSTEM_PROCESS_INFORMATION)
                            .add(1)
                            .cast::<SYSTEM_THREAD_INFORMATION>(),
                        process.NumberOfThreads as usize,
                    )
                };
                let total = threads
                    .iter()
                    // windows-sys exposes the SYSTEM_THREAD_INFORMATION ContextSwitches ABI slot
                    // as Reserved3.
                    .map(|thread| i64::from(thread.Reserved3))
                    .sum();
                return total;
            }
            assert_ne!(
                process.NextEntryOffset, 0,
                "current process missing from system snapshot"
            );
            offset += process.NextEntryOffset as usize;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Usage, percent_change, summarize, usage_delta};

    #[test]
    fn summary_retains_distribution_and_mad() {
        let summary = summarize(vec![1.0, 2.0, 3.0, 4.0, 100.0]);
        assert_eq!(summary.median, 3.0);
        assert_eq!(summary.p95, 100.0);
        assert_eq!(summary.p99, 100.0);
        assert_eq!(summary.mad, 1.0);
        assert_eq!(summary.min, 1.0);
        assert_eq!(summary.max, 100.0);
    }

    #[test]
    fn percent_change_rejects_zero_baseline() {
        assert_eq!(percent_change(10.0, 15.0), Some(50.0));
        assert_eq!(percent_change(0.0, 15.0), None);
    }

    #[test]
    fn usage_delta_tracks_total_and_split_context_switches() {
        let before = Usage {
            total: Some(10),
            voluntary: Some(3),
            involuntary: Some(7),
        };
        let after = Usage {
            total: Some(14),
            voluntary: Some(5),
            involuntary: Some(9),
        };
        let delta = usage_delta(before, after);
        assert_eq!(delta.total, Some(4));
        assert_eq!(delta.voluntary, Some(2));
        assert_eq!(delta.involuntary, Some(2));
    }

    #[cfg(windows)]
    #[test]
    fn windows_usage_exposes_total_context_switches() {
        assert!(super::usage().total.is_some());
    }
}
