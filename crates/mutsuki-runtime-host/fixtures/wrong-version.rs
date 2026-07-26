//! Minimal library that exports the right symbol with an unsupported transport version.

use std::ffi::c_void;

#[repr(C)]
pub struct AbiHostV2 {
    context: *mut c_void,
    request: Option<unsafe extern "C" fn(*mut c_void, *const u8, usize) -> AbiCallResult>,
    release: Option<unsafe extern "C" fn(AbiBuffer)>,
}

#[repr(C)]
pub struct AbiBuffer {
    ptr: *mut u8,
    len: usize,
}

#[repr(C)]
pub struct AbiCallResult {
    status: i32,
    payload: AbiBuffer,
}

#[repr(C)]
pub struct AbiPluginV2 {
    transport_version: u32,
    context: *mut c_void,
    request: Option<unsafe extern "C" fn(*mut c_void, *const u8, usize) -> AbiCallResult>,
    release: Option<unsafe extern "C" fn(AbiBuffer)>,
    close: Option<unsafe extern "C" fn(*mut c_void)>,
}

#[unsafe(no_mangle)]
pub extern "C" fn mutsuki_plugin_abi_v2(_host: AbiHostV2) -> AbiPluginV2 {
    AbiPluginV2 {
        transport_version: 99,
        context: std::ptr::null_mut(),
        request: None,
        release: None,
        close: None,
    }
}
