// This file is on the workspace `unsafe_code` exception list.
// This lane drives the `extern "C"` ABI v2 surface directly so the benchmark measures the real
// call path rather than a safe wrapper around it.
#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::hint::black_box;

use mutsuki_runtime_sdk::abi::{AbiCallResult, AbiGuest, AbiReleaseFn, plugin_api_from_guest};
use mutsuki_runtime_wire::{DisposeRunnerRequest, encode_binary_request};

use crate::allocator::TrackingAllocator;
use crate::report::{BenchmarkMode, CaseResult};

struct EchoGuest;

impl AbiGuest for EchoGuest {
    fn request(&self, request: &[u8]) -> Vec<u8> {
        request.to_vec()
    }
}

pub(super) fn run(
    mode: BenchmarkMode,
    allocator: &TrackingAllocator,
) -> Result<Vec<CaseResult>, String> {
    let iterations = mode.select(100_000, 2_000_000);
    let request = DisposeRunnerRequest {
        runner_id: "benchmark.runner".into(),
    };
    let binary = encode_binary_request(1, &request, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS)
        .map_err(|error| error.to_string())?;
    let v2 = plugin_api_from_guest(Box::new(EchoGuest));
    let binary_case = measure(
        allocator,
        iterations,
        "binary",
        &binary,
        v2.context,
        v2.request.expect("v2 request"),
        v2.release.expect("v2 release"),
    );
    // SAFETY: the guest was built locally by `plugin_api_from_guest`, so the context matches the
    // callback, and the measurement loop above has already returned every borrowed buffer.
    unsafe { v2.close.expect("v2 close")(v2.context) };
    Ok(vec![binary_case])
}

fn measure(
    allocator: &TrackingAllocator,
    iterations: u64,
    codec: &str,
    request: &[u8],
    context: *mut c_void,
    callback: unsafe extern "C" fn(*mut c_void, *const u8, usize) -> AbiCallResult,
    release: AbiReleaseFn,
) -> CaseResult {
    let measurement = allocator.measurement();
    for _ in 0..iterations {
        // SAFETY: `context`, `callback` and `release` come from one locally constructed
        // `AbiPluginV2` and are used together; `request` stays borrowed for the call.
        let response = unsafe { callback(context, request.as_ptr(), request.len()) };
        // SAFETY: the payload is owned by the guest until the paired release below.
        black_box(unsafe { response.payload.as_slice() });
        // SAFETY: single release for the payload returned by the call above.
        unsafe { release(response.payload) };
    }
    let (elapsed_ns, allocations) = measurement.finish(allocator);
    CaseResult::measured(
        format!("wire/p2/native_abi/{codec}"),
        "wire_p2_native_abi",
        BTreeMap::from([
            ("phase".into(), "p2".into()),
            ("surface".into(), "native_abi".into()),
            ("codec".into(), codec.into()),
        ]),
        iterations,
        iterations,
        elapsed_ns,
        allocations,
        BTreeMap::from([("frame_bytes".into(), request.len() as i128)]),
    )
}
