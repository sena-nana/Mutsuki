//! Backward-compatible ABI exports backed by the independent plugin API crate.

use std::ptr;

use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};

pub use mutsuki_plugin_api::{
    AbiBuffer, AbiCallResult, AbiCloseFn, AbiEntryV1, AbiEntryV2, AbiGuest, AbiHostV1, AbiHostV2,
    AbiPluginV1, AbiPluginV2, AbiReleaseFn, AbiRequestFn, plugin_api_from_guest,
    plugin_api_v2_from_guest,
};

pub const ABI_TRANSPORT_VERSION: u32 = mutsuki_plugin_api::ABI_V1_TRANSPORT_VERSION;
pub const ABI_ENTRY_SYMBOL: &[u8] = mutsuki_plugin_api::ABI_V1_ENTRY_SYMBOL;
pub const ABI_CODEC_ID: &str = mutsuki_plugin_api::ABI_V1_CODEC_ID;
pub const ABI_BRIDGE_ID: &str = mutsuki_plugin_api::ABI_V1_BRIDGE_ID;
pub const ABI_V2_TRANSPORT_VERSION: u32 = mutsuki_plugin_api::ABI_V2_TRANSPORT_VERSION;
pub const ABI_V2_ENTRY_SYMBOL: &[u8] = mutsuki_plugin_api::ABI_V2_ENTRY_SYMBOL;
pub const ABI_V2_CODEC_ID: &str = mutsuki_plugin_api::ABI_V2_CODEC_ID;
pub const ABI_V2_BRIDGE_ID: &str = mutsuki_plugin_api::ABI_V2_BRIDGE_ID;

pub(crate) fn consume_call_result(
    result: AbiCallResult,
    release: Option<AbiReleaseFn>,
    route: &'static str,
) -> RuntimeResult<(bool, Vec<u8>)> {
    let valid_buffer = (result.payload.len == 0 && result.payload.ptr.is_null())
        || (result.payload.len > 0 && !result.payload.ptr.is_null());
    if !valid_buffer {
        return Err(abi_contract_failure(
            route,
            "invalid payload pointer/length pair",
        ));
    }
    if result.payload.len > 0 && release.is_none() {
        return Err(abi_contract_failure(
            route,
            "non-empty payload is missing its release callback",
        ));
    }
    // SAFETY: the ABI callback owns the payload until the paired release callback below.
    let bytes = unsafe { result.payload.as_slice() }.to_vec();
    if let Some(release) = release.filter(|_| result.payload.len > 0) {
        // SAFETY: the plugin returned a validated non-empty payload and a release callback.
        unsafe { release(result.payload) };
    }
    match result.status {
        0 => Ok((true, bytes)),
        1 => Ok((false, bytes)),
        _ => Err(abi_contract_failure(route, "invalid ABI callback status")),
    }
}

fn abi_contract_failure(route: &'static str, detail: &'static str) -> RuntimeFailure {
    RuntimeFailure::new(mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        detail,
        route,
    ))
}

#[allow(dead_code)]
pub(crate) unsafe extern "C" fn release_buffer(buffer: AbiBuffer) {
    if buffer.ptr.is_null() || buffer.len == 0 {
        return;
    }
    let slice = ptr::slice_from_raw_parts_mut(buffer.ptr, buffer.len);
    // SAFETY: buffers passed here were allocated by the independent plugin API allocator.
    unsafe { drop(Box::from_raw(slice)) };
}
