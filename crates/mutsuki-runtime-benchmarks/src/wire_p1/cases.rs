use std::collections::BTreeMap;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use mutsuki_runtime_host::JsonlTransport;
use mutsuki_runtime_wire::{
    CancelRunnerRequest, DEFAULT_WIRE_LIMITS, DisposeRunnerRequest, RunBatchRequest,
};

use crate::allocator::TrackingAllocator;
use crate::report::{BenchmarkMode, CaseResult};

use super::fixtures::run_batch_request;
use super::server::{ServerMode, spawn};

pub(super) fn run(
    mode: BenchmarkMode,
    allocator: &TrackingAllocator,
) -> Result<Vec<CaseResult>, String> {
    let mut cases = vec![cancel_case(mode, allocator)?];
    for concurrency in [1, 16, 56] {
        cases.push(concurrent_case(mode, allocator, concurrency)?);
    }
    Ok(cases)
}

fn cancel_case(mode: BenchmarkMode, allocator: &TrackingAllocator) -> Result<CaseResult, String> {
    let iterations = mode.select(10, 100);
    let (reader, writer, server) = spawn(ServerMode::Cancel);
    let transport = JsonlTransport::new(reader, writer);
    transport
        .request(&DisposeRunnerRequest {
            runner_id: "benchmark.runner".into(),
        })
        .map_err(|error| error.to_string())?;
    let run = run_batch_request(1, 32);
    let cancel = CancelRunnerRequest {
        runner_id: "benchmark.runner".into(),
        invocation_id: run.ctx.invocation_id.clone(),
    };
    let measurement = allocator.measurement();
    let mut latencies = Vec::with_capacity(iterations as usize);
    for iteration in 0..iterations {
        std::thread::scope(|scope| -> Result<(), String> {
            let run_transport = transport.clone();
            let run = run.clone();
            let running = scope.spawn(move || run_transport.request(&run));
            server.wait_for_runs(iteration as usize + 1)?;
            let started = Instant::now();
            transport
                .request(&cancel)
                .map_err(|error| error.to_string())?;
            latencies.push(started.elapsed().as_nanos() as u64);
            running
                .join()
                .map_err(|_| "run request panicked".to_string())?
                .map_err(|error| error.to_string())?;
            Ok(())
        })?;
    }
    let (elapsed_ns, allocations) = measurement.finish(allocator);
    let mut sorted = latencies.clone();
    sorted.sort_unstable();
    let p50 = percentile(&sorted, 50);
    let p95 = percentile(&sorted, 95);
    let max = sorted.last().copied().unwrap_or_default();
    close_transport(transport, server)?;
    Ok(CaseResult::measured(
        "wire/p1/jsonl/cancel-during-run_batch",
        "wire_p1",
        BTreeMap::from([
            ("phase".into(), "p1".into()),
            ("operation".into(), "cancel".into()),
            ("in_flight".into(), "2".into()),
        ]),
        iterations,
        iterations,
        elapsed_ns,
        allocations,
        BTreeMap::from([
            ("p50_ns".into(), p50 as i128),
            ("p95_ns".into(), p95 as i128),
            ("max_ns".into(), max as i128),
        ]),
    ))
}

fn concurrent_case(
    mode: BenchmarkMode,
    allocator: &TrackingAllocator,
    concurrency: usize,
) -> Result<CaseResult, String> {
    let iterations = match (mode, concurrency) {
        (BenchmarkMode::Smoke, 1) => 10,
        (BenchmarkMode::Smoke, 16) => 4,
        (BenchmarkMode::Smoke, _) => 2,
        (BenchmarkMode::Full, 1) => 5_000,
        (BenchmarkMode::Full, 16) => 313,
        (BenchmarkMode::Full, _) => 90,
    };
    let (reader, writer, server) = spawn(ServerMode::Concurrent {
        group_size: concurrency,
    });
    let limits = DEFAULT_WIRE_LIMITS;
    let transport = JsonlTransport::with_limits(reader, writer, limits, Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    transport
        .request(&DisposeRunnerRequest {
            runner_id: "benchmark.runner".into(),
        })
        .map_err(|error| error.to_string())?;
    let request = run_batch_request(1, 32);
    let measurement = allocator.measurement();
    let mut elapsed_ns = 0_u64;
    for _ in 0..iterations {
        let barrier = Arc::new(Barrier::new(concurrency + 1));
        std::thread::scope(|scope| -> Result<(), String> {
            let mut requests = Vec::with_capacity(concurrency);
            for _ in 0..concurrency {
                let transport = transport.clone();
                let request: RunBatchRequest = request.clone();
                let barrier = barrier.clone();
                requests.push(scope.spawn(move || {
                    barrier.wait();
                    transport.request(&request)
                }));
            }
            let started = Instant::now();
            barrier.wait();
            for request in requests {
                request
                    .join()
                    .map_err(|_| "concurrent request panicked".to_string())?
                    .map_err(|error| error.to_string())?;
            }
            elapsed_ns = elapsed_ns.saturating_add(started.elapsed().as_nanos() as u64);
            Ok(())
        })?;
    }
    let (_, allocations) = measurement.finish(allocator);
    close_transport(transport, server)?;
    let units = iterations * concurrency as u64;
    Ok(CaseResult::measured(
        format!("wire/p1/jsonl/concurrent/in-flight-{concurrency}"),
        "wire_p1",
        BTreeMap::from([
            ("phase".into(), "p1".into()),
            ("operation".into(), "run_batch".into()),
            ("in_flight".into(), concurrency.to_string()),
            ("response_order".into(), "reverse".into()),
        ]),
        iterations,
        units,
        elapsed_ns,
        allocations,
        BTreeMap::new(),
    ))
}

fn close_transport<R, W>(
    transport: JsonlTransport<R, W>,
    server: super::server::ServerHandle,
) -> Result<(), String>
where
    R: std::io::BufRead + Send + 'static,
    W: std::io::Write + Send + 'static,
{
    let (reader, writer) = transport.into_inner();
    drop(writer);
    drop(reader);
    server.join()
}

fn percentile(sorted: &[u64], percentile: usize) -> u64 {
    let index = sorted
        .len()
        .saturating_mul(percentile)
        .div_ceil(100)
        .saturating_sub(1);
    sorted.get(index).copied().unwrap_or_default()
}
