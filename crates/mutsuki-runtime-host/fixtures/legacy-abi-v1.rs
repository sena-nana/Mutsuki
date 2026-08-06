//! Frozen legacy ABI v1 symbol fixture. The current loader must detect this symbol without
//! invoking it and return `abi.unsupported_version`.

use std::ffi::c_void;

#[repr(C)]
pub struct LegacyAbiHostV1 {
    pub context: *mut c_void,
    pub request: Option<unsafe extern "C" fn()>,
    pub release: Option<unsafe extern "C" fn()>,
}

#[repr(C)]
pub struct LegacyAbiPluginV1 {
    pub transport_version: u32,
    pub context: *mut c_void,
    pub request: Option<unsafe extern "C" fn()>,
    pub release: Option<unsafe extern "C" fn()>,
    pub close: Option<unsafe extern "C" fn()>,
}

#[unsafe(no_mangle)]
pub extern "C" fn mutsuki_plugin_abi_v1(_host: LegacyAbiHostV1) -> LegacyAbiPluginV1 {
    panic!("the retired ABI v1 entry must never be invoked")
}
