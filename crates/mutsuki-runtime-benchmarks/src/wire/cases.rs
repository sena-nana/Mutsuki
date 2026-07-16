use std::collections::BTreeMap;
use std::hint::black_box;

use mutsuki_runtime_wire::{
    AnyWireRequest, RunBatchRequest, WireRequest, decode_jsonl_any_request, encode_jsonl_request,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::allocator::TrackingAllocator;
use crate::report::{BenchmarkMode, CaseResult};

use super::fixtures::{dispose_request, run_batch_request};

pub fn run(mode: BenchmarkMode, allocator: &TrackingAllocator) -> Result<Vec<CaseResult>, String> {
    let mut cases = Vec::new();
    let empty_iterations = mode.select(500, 5_000);
    cases.push(encode_case(
        allocator,
        "dispose",
        1,
        0,
        empty_iterations,
        &dispose_request(),
        Codec::Legacy,
    )?);
    cases.push(encode_case(
        allocator,
        "dispose",
        1,
        0,
        empty_iterations,
        &dispose_request(),
        Codec::Typed,
    )?);

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
        for codec in [Codec::Legacy, Codec::Typed] {
            cases.push(encode_case(
                allocator,
                "run_batch",
                entries,
                32,
                iterations,
                &request,
                codec,
            )?);
            cases.push(decode_case(
                allocator, entries, 32, iterations, &request, codec,
            )?);
        }
    }
    Ok(cases)
}

#[derive(Clone, Copy)]
enum Codec {
    Legacy,
    Typed,
}

impl Codec {
    fn label(self) -> &'static str {
        match self {
            Self::Legacy => "legacy_json_rpc",
            Self::Typed => "typed_jsonl",
        }
    }
}

fn encode_case<R: WireRequest + Serialize>(
    allocator: &TrackingAllocator,
    operation: &str,
    entries: usize,
    payload_bytes: usize,
    iterations: u64,
    request: &R,
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
        format!(
            "wire/p0/{}/{operation}/encode/entries-{entries}",
            codec.label()
        ),
        "wire_p0",
        dimensions(codec, operation, "encode", entries, payload_bytes),
        iterations,
        iterations.saturating_mul(entries as u64),
        elapsed_ns,
        allocations,
        BTreeMap::from([("frame_bytes".into(), frame_bytes as i128)]),
    ))
}

fn decode_case(
    allocator: &TrackingAllocator,
    entries: usize,
    payload_bytes: usize,
    iterations: u64,
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
        format!(
            "wire/p0/{}/run_batch/decode/entries-{entries}",
            codec.label()
        ),
        "wire_p0",
        dimensions(codec, "run_batch", "decode", entries, payload_bytes),
        iterations,
        iterations.saturating_mul(entries as u64),
        elapsed_ns,
        allocations,
        BTreeMap::from([("frame_bytes".into(), encoded.len() as i128)]),
    ))
}

fn encode<R: WireRequest + Serialize>(codec: Codec, request: &R) -> Result<Vec<u8>, String> {
    match codec {
        Codec::Legacy => {
            let params = serde_json::to_value(request).map_err(|error| error.to_string())?;
            serde_json::to_vec(&json!({
                "id": "req-1",
                "method": R::OPCODE.method(),
                "params": params,
            }))
            .map_err(|error| error.to_string())
        }
        Codec::Typed => encode_jsonl_request(1, request, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS)
            .map_err(|error| error.to_string()),
    }
}

fn decode(codec: Codec, encoded: &[u8]) -> Result<RunBatchRequest, String> {
    match codec {
        Codec::Legacy => {
            let envelope: Value =
                serde_json::from_slice(encoded).map_err(|error| error.to_string())?;
            decode_value(
                envelope
                    .get("params")
                    .cloned()
                    .ok_or_else(|| "legacy params missing".to_string())?,
            )
        }
        Codec::Typed => {
            let decoded =
                decode_jsonl_any_request(encoded, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS)
                    .map_err(|error| error.to_string())?;
            match decoded.request {
                AnyWireRequest::RunBatch(request) => Ok(*request),
                other => Err(format!("unexpected opcode {:#06x}", other.opcode() as u16)),
            }
        }
    }
}

fn decode_value<T: DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| error.to_string())
}

fn dimensions(
    codec: Codec,
    operation: &str,
    direction: &str,
    entries: usize,
    payload_bytes: usize,
) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("phase".into(), "p0".into()),
        ("codec".into(), codec.label().into()),
        ("operation".into(), operation.into()),
        ("direction".into(), direction.into()),
        ("entries".into(), entries.to_string()),
        ("payload_bytes_per_entry".into(), payload_bytes.to_string()),
    ])
}
