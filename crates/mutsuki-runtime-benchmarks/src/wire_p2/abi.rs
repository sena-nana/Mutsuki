use std::collections::BTreeMap;
use std::ffi::c_void;
use std::hint::black_box;

use mutsuki_runtime_sdk::abi::{
    AbiCallResult, AbiGuest, AbiReleaseFn, plugin_api_from_guest, plugin_api_v2_from_guest,
};
use mutsuki_runtime_wire::{DisposeRunnerRequest, encode_binary_request, encode_jsonl_request};

use crate::allocator::TrackingAllocator;
use crate::report::{BenchmarkMode, CaseResult};

struct EchoGuest;

impl AbiGuest for EchoGuest {
    fn request(&mut self, request: &[u8]) -> Vec<u8> {
        request.to_vec()
    }
}

pub(super) fn run(
    mode: BenchmarkMode,
    allocator: &TrackingAllocator,
) -> Result<Vec<CaseResult>, String> {
    let iterations = mode.select(1_000, 20_000);
    let request = DisposeRunnerRequest {
        runner_id: "benchmark.runner".into(),
    };
    let json = encode_jsonl_request(1, &request, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS)
        .map_err(|error| error.to_string())?;
    let binary = encode_binary_request(1, &request, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS)
        .map_err(|error| error.to_string())?;
    let v1 = plugin_api_from_guest(Box::new(EchoGuest));
    let v2 = plugin_api_v2_from_guest(Box::new(EchoGuest));
    let json_case = measure(
        allocator,
        iterations,
        "jsonl",
        &json,
        v1.context,
        v1.request.expect("v1 request"),
        v1.release.expect("v1 release"),
    );
    let binary_case = measure(
        allocator,
        iterations,
        "binary",
        &binary,
        v2.context,
        v2.request.expect("v2 request"),
        v2.release.expect("v2 release"),
    );
    unsafe { v1.close.expect("v1 close")(v1.context) };
    unsafe { v2.close.expect("v2 close")(v2.context) };
    Ok(vec![json_case, binary_case])
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
        let response = unsafe { callback(context, request.as_ptr(), request.len()) };
        black_box(unsafe { response.payload.as_slice() });
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
