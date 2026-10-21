//! ABI v2 exports backed by the independent plugin API crate.

use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};

pub use mutsuki_plugin_api::{
    AbiBuffer, AbiCallResult, AbiCloseFn, AbiEntryV2, AbiGuest, AbiHostV2, AbiPluginV2,
    AbiReleaseFn, AbiRequestFn, plugin_api_from_guest,
};

pub const ABI_V2_TRANSPORT_VERSION: u32 = mutsuki_plugin_api::ABI_V2_TRANSPORT_VERSION;
pub const ABI_V2_ENTRY_SYMBOL: &[u8] = mutsuki_plugin_api::ABI_V2_ENTRY_SYMBOL;
pub const ABI_V2_CODEC_ID: &str = mutsuki_plugin_api::ABI_V2_CODEC_ID;
pub const ABI_V2_BRIDGE_ID: &str = mutsuki_plugin_api::ABI_V2_BRIDGE_ID;

/// The plugin API owns the ABI result contract; the SDK only lifts its error into a
/// `RuntimeFailure`.
pub(crate) fn consume_call_result(
    result: AbiCallResult,
    release: Option<AbiReleaseFn>,
    route: &'static str,
) -> RuntimeResult<(bool, Vec<u8>)> {
    mutsuki_plugin_api::consume_call_result(result, release, route)
        .map_err(|error| RuntimeFailure::new(error.error))
}

// Only tests release plugin-allocated buffers directly; production frees
// through the plugin API allocator vtable.
#[cfg(test)]
pub(crate) unsafe extern "C" fn release_buffer(buffer: AbiBuffer) {
    if buffer.ptr.is_null() || buffer.len == 0 {
        return;
    }
    let slice = std::ptr::slice_from_raw_parts_mut(buffer.ptr, buffer.len);
    // SAFETY: buffers passed here were allocated by the independent plugin API allocator.
    unsafe { drop(Box::from_raw(slice)) };
}
