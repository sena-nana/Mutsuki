use std::collections::BTreeMap;
use std::hint::black_box;
use std::io::Cursor;

use mutsuki_runtime_wire::{
    DEFAULT_WIRE_LIMITS, DisposeRunnerRequest, decode_binary_any_request, encode_binary_request,
    read_binary_frame,
};

use crate::allocator::TrackingAllocator;
use crate::report::{BenchmarkMode, CaseResult};

pub(super) fn run(
    mode: BenchmarkMode,
    allocator: &TrackingAllocator,
) -> Result<Vec<CaseResult>, String> {
    let valid = encode_binary_request(
        1,
        &DisposeRunnerRequest {
            runner_id: "benchmark.runner".into(),
        },
        DEFAULT_WIRE_LIMITS,
    )
    .map_err(|error| error.to_string())?;
    let mut malformed = valid.clone();
    *malformed.last_mut().expect("dispose frame payload") = 0xc1;
    let truncated = valid[..valid.len() - 1].to_vec();
    let oversized = (DEFAULT_WIRE_LIMITS.max_frame_bytes as u32 + 1)
        .to_be_bytes()
        .to_vec();
    let iterations = mode.select(1_000, 100_000);

    Ok(vec![
        measure(
            allocator,
            iterations,
            "malformed-messagepack",
            malformed.len(),
            || decode_binary_any_request(black_box(&malformed), DEFAULT_WIRE_LIMITS).is_err(),
        ),
        measure(
            allocator,
            iterations,
            "truncated-frame",
            truncated.len(),
            || decode_binary_any_request(black_box(&truncated), DEFAULT_WIRE_LIMITS).is_err(),
        ),
        measure(
            allocator,
            iterations,
            "oversized-prefix",
            oversized.len(),
            || {
                read_binary_frame(&mut Cursor::new(black_box(&oversized)), DEFAULT_WIRE_LIMITS)
                    .is_err()
            },
        ),
    ])
}

fn measure(
    allocator: &TrackingAllocator,
    iterations: u64,
    kind: &str,
    input_bytes: usize,
    mut reject: impl FnMut() -> bool,
) -> CaseResult {
    for _ in 0..10 {
        assert!(reject(), "rejection benchmark input must be invalid");
    }
    let measurement = allocator.measurement();
    let mut rejected = 0_i128;
    for _ in 0..iterations {
        rejected += i128::from(reject());
    }
    let (elapsed_ns, allocations) = measurement.finish(allocator);
    CaseResult::measured(
        format!("wire/p3/binary/reject/{kind}"),
        "wire_p3_rejection",
        BTreeMap::from([
            ("phase".into(), "p3".into()),
            ("codec".into(), "binary".into()),
            ("failure".into(), kind.into()),
        ]),
        iterations,
        iterations,
        elapsed_ns,
        allocations,
        BTreeMap::from([
            ("input_bytes".into(), input_bytes as i128),
            ("rejected".into(), rejected),
        ]),
    )
}
