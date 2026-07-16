use std::collections::BTreeMap;
use std::hint::black_box;

use mutsuki_runtime_wire::{
    AnyWireRequest, RunBatchRequest, decode_binary_any_request, decode_jsonl_any_request,
    encode_binary_request, encode_jsonl_request,
};

use crate::allocator::TrackingAllocator;
use crate::report::{BenchmarkMode, CaseResult};

use super::fixtures::run_batch_request;

#[derive(Clone, Copy)]
enum Codec {
    Jsonl,
    Binary,
}

impl Codec {
    fn label(self) -> &'static str {
        match self {
            Self::Jsonl => "jsonl",
            Self::Binary => "binary",
        }
    }
}

pub(super) fn run(
    mode: BenchmarkMode,
    allocator: &TrackingAllocator,
) -> Result<Vec<CaseResult>, String> {
    let mut cases = Vec::new();
    for entries in [1, 16, 256, 4_096] {
        let iterations = match (mode, entries) {
            (BenchmarkMode::Smoke, 1) => 100,
            (BenchmarkMode::Smoke, 16) => 40,
            (BenchmarkMode::Smoke, 256) => 4,
            (BenchmarkMode::Smoke, _) => 1,
            (BenchmarkMode::Full, 1) => 1_000,
            (BenchmarkMode::Full, 16) => 300,
            (BenchmarkMode::Full, 256) => 30,
            (BenchmarkMode::Full, _) => 3,
        };
        let request = run_batch_request(entries, 32);
        for codec in [Codec::Jsonl, Codec::Binary] {
            cases.push(encode_case(
                allocator, iterations, entries, &request, codec,
            )?);
            cases.push(decode_case(
                allocator, iterations, entries, &request, codec,
            )?);
        }
    }
    Ok(cases)
}

fn encode_case(
    allocator: &TrackingAllocator,
    iterations: u64,
    entries: usize,
    request: &RunBatchRequest,
    codec: Codec,
) -> Result<CaseResult, String> {
    for _ in 0..3 {
        black_box(encode(codec, request)?);
    }
    let measurement = allocator.measurement();
    let mut frame_bytes = 0;
    for _ in 0..iterations {
        let encoded = encode(codec, black_box(request))?;
        frame_bytes = encoded.len();
        black_box(encoded);
    }
    let (elapsed_ns, allocations) = measurement.finish(allocator);
    Ok(CaseResult::measured(
        format!("wire/p2/{}/encode/entries-{entries}", codec.label()),
        "wire_p2_codec",
        dimensions(codec, "encode", entries),
        iterations,
        iterations * entries as u64,
        elapsed_ns,
        allocations,
        BTreeMap::from([("frame_bytes".into(), frame_bytes as i128)]),
    ))
}

fn decode_case(
    allocator: &TrackingAllocator,
    iterations: u64,
    entries: usize,
    request: &RunBatchRequest,
    codec: Codec,
) -> Result<CaseResult, String> {
    let encoded = encode(codec, request)?;
    for _ in 0..3 {
        black_box(decode(codec, &encoded)?);
    }
    let measurement = allocator.measurement();
    for _ in 0..iterations {
        black_box(decode(codec, black_box(&encoded))?);
    }
    let (elapsed_ns, allocations) = measurement.finish(allocator);
    Ok(CaseResult::measured(
        format!("wire/p2/{}/decode/entries-{entries}", codec.label()),
        "wire_p2_codec",
        dimensions(codec, "decode", entries),
        iterations,
        iterations * entries as u64,
        elapsed_ns,
        allocations,
        BTreeMap::from([("frame_bytes".into(), encoded.len() as i128)]),
    ))
}

fn encode(codec: Codec, request: &RunBatchRequest) -> Result<Vec<u8>, String> {
    match codec {
        Codec::Jsonl => encode_jsonl_request(1, request, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS),
        Codec::Binary => {
            encode_binary_request(1, request, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS)
        }
    }
    .map_err(|error| error.to_string())
}

fn decode(codec: Codec, encoded: &[u8]) -> Result<RunBatchRequest, String> {
    let decoded = match codec {
        Codec::Jsonl => {
            decode_jsonl_any_request(encoded, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS)
        }
        Codec::Binary => {
            decode_binary_any_request(encoded, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS)
        }
    }
    .map_err(|error| error.to_string())?;
    match decoded.request {
        AnyWireRequest::RunBatch(request) => Ok(*request),
        other => Err(format!("unexpected opcode {:#06x}", other.opcode() as u16)),
    }
}

fn dimensions(codec: Codec, direction: &str, entries: usize) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("phase".into(), "p2".into()),
        ("surface".into(), "codec".into()),
        ("codec".into(), codec.label().into()),
        ("direction".into(), direction.into()),
        ("entries".into(), entries.to_string()),
    ])
}
