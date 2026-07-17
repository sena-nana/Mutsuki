use std::collections::BTreeMap;
#[cfg(unix)]
use std::mem::MaybeUninit;

use crate::report::{CaseMetrics, CaseReport, Correctness, Distribution};

pub fn process_case() -> CaseReport {
    let metrics = process_metrics();
    CaseReport {
        case_id: "core.system.process".into(),
        measurement_mode: "system".into(),
        dimensions: BTreeMap::from([(
            "scope".into(),
            "entire-benchmark-process-including-fixture-construction".into(),
        )]),
        metrics: CaseMetrics {
            cpu_time_ns: Some(Distribution::from_samples(
                [metrics.cpu_time_ns as f64],
                "ns/process",
            )),
            peak_rss_bytes: Some(metrics.peak_rss_bytes as f64),
            context_switches: Some(metrics.context_switches as f64),
            ..CaseMetrics::default()
        },
        correctness: Correctness {
            passed: true,
            counters: BTreeMap::new(),
            output_hash: None,
        },
        stage_breakdown: BTreeMap::new(),
    }
}

struct ProcessMetrics {
    cpu_time_ns: u64,
    peak_rss_bytes: u64,
    context_switches: u64,
}

#[cfg(unix)]
fn process_metrics() -> ProcessMetrics {
    let mut usage = MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage initializes the provided rusage pointer on success.
    let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if status != 0 {
        return ProcessMetrics {
            cpu_time_ns: 0,
            peak_rss_bytes: 0,
            context_switches: 0,
        };
    }
    // SAFETY: status == 0 means getrusage initialized the structure.
    let usage = unsafe { usage.assume_init() };
    let cpu_time_ns = timeval_ns(usage.ru_utime).saturating_add(timeval_ns(usage.ru_stime));
    #[cfg(target_os = "macos")]
    let peak_rss_bytes = usage.ru_maxrss.max(0) as u64;
    #[cfg(not(target_os = "macos"))]
    let peak_rss_bytes = (usage.ru_maxrss.max(0) as u64).saturating_mul(1024);
    ProcessMetrics {
        cpu_time_ns,
        peak_rss_bytes,
        context_switches: (usage.ru_nvcsw.max(0) as u64)
            .saturating_add(usage.ru_nivcsw.max(0) as u64),
    }
}

#[cfg(unix)]
fn timeval_ns(value: libc::timeval) -> u64 {
    (value.tv_sec.max(0) as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add((value.tv_usec.max(0) as u64).saturating_mul(1_000))
}

#[cfg(not(unix))]
fn process_metrics() -> ProcessMetrics {
    ProcessMetrics {
        cpu_time_ns: 0,
        peak_rss_bytes: 0,
        context_switches: 0,
    }
}
