use std::collections::BTreeMap;
use std::sync::{Arc, Barrier};
use std::time::Duration;

use mutsuki_runtime_host::{BinaryTransport, JsonlTransport, TypedRequestTransport};

use crate::allocator::TrackingAllocator;
use crate::report::{BenchmarkMode, CaseResult};

use super::fixtures::run_batch_request;
use super::server::{Codec, ServerHandle, spawn};

pub(super) fn run(
    mode: BenchmarkMode,
    allocator: &TrackingAllocator,
) -> Result<Vec<CaseResult>, String> {
    Ok(vec![jsonl(mode, allocator)?, binary(mode, allocator)?])
}

fn jsonl(mode: BenchmarkMode, allocator: &TrackingAllocator) -> Result<CaseResult, String> {
    let concurrency = 16;
    let (reader, writer, server) = spawn(Codec::Jsonl, concurrency);
    let transport = JsonlTransport::with_limits(
        reader,
        writer,
        mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
        Duration::from_secs(5),
    )
    .map_err(|error| error.to_string())?;
    let case = measure(mode, allocator, "jsonl", &transport, concurrency)?;
    let (reader, writer) = transport.into_inner();
    close(reader, writer, server)?;
    Ok(case)
}

fn binary(mode: BenchmarkMode, allocator: &TrackingAllocator) -> Result<CaseResult, String> {
    let concurrency = 16;
    let (reader, writer, server) = spawn(Codec::Binary, concurrency);
    let transport = BinaryTransport::with_limits(
        reader,
        writer,
        mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
        Duration::from_secs(5),
    )
    .map_err(|error| error.to_string())?;
    let case = measure(mode, allocator, "binary", &transport, concurrency)?;
    let (reader, writer) = transport.into_inner();
    close(reader, writer, server)?;
    Ok(case)
}

fn measure<T: TypedRequestTransport + Clone>(
    mode: BenchmarkMode,
    allocator: &TrackingAllocator,
    codec: &str,
    transport: &T,
    concurrency: usize,
) -> Result<CaseResult, String> {
    let iterations = mode.select(3, 30);
    let request = run_batch_request(1, 32);
    let measurement = allocator.measurement();
    let mut elapsed_ns = 0_u64;
    for _ in 0..iterations {
        let barrier = Arc::new(Barrier::new(concurrency + 1));
        std::thread::scope(|scope| -> Result<(), String> {
            let mut pending = Vec::with_capacity(concurrency);
            for _ in 0..concurrency {
                let transport = transport.clone();
                let request = request.clone();
                let barrier = barrier.clone();
                pending.push(scope.spawn(move || {
                    barrier.wait();
                    transport.request(&request)
                }));
            }
            let started = std::time::Instant::now();
            barrier.wait();
            for response in pending {
                response
                    .join()
                    .map_err(|_| "P2 transport request panicked".to_string())?
                    .map_err(|error| error.to_string())?;
            }
            elapsed_ns += started.elapsed().as_nanos() as u64;
            Ok(())
        })?;
    }
    let (_, allocations) = measurement.finish(allocator);
    let units = iterations * concurrency as u64;
    Ok(CaseResult::measured(
        format!("wire/p2/stdio/{codec}"),
        "wire_p2_stdio",
        BTreeMap::from([
            ("phase".into(), "p2".into()),
            ("surface".into(), "stdio".into()),
            ("codec".into(), codec.into()),
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

fn close<R, W>(reader: R, writer: W, server: ServerHandle) -> Result<(), String> {
    drop(writer);
    drop(reader);
    server.join()
}
