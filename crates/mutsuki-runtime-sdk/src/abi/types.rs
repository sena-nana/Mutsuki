use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Mutex;

pub const ABI_TRANSPORT_VERSION: u32 = 1;
pub const ABI_ENTRY_SYMBOL: &[u8] = b"mutsuki_plugin_abi_v1\0";
pub const ABI_CODEC_ID: &str = "mutsuki.codec.typed-jsonl.v1";
pub const ABI_BRIDGE_ID: &str = "mutsuki.bridge.abi.jsonl.v1";

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AbiBuffer {
    pub ptr: *mut u8,
    pub len: usize,
}

impl AbiBuffer {
    pub const fn empty() -> Self {
        Self {
            ptr: ptr::null_mut(),
            len: 0,
        }
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        if bytes.is_empty() {
            return Self::empty();
        }
        let mut boxed = bytes.into_boxed_slice();
        let buffer = Self {
            ptr: boxed.as_mut_ptr(),
            len: boxed.len(),
        };
        std::mem::forget(boxed);
        buffer
    }

    /// # Safety
    ///
    /// The paired owner must keep the buffer alive until its release callback is invoked.
    pub unsafe fn as_slice<'a>(&self) -> &'a [u8] {
        if self.len == 0 {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(self.ptr.cast_const(), self.len) }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AbiCallResult {
    pub status: i32,
    pub payload: AbiBuffer,
}

impl AbiCallResult {
    pub fn ok(bytes: Vec<u8>) -> Self {
        Self {
            status: 0,
            payload: AbiBuffer::from_bytes(bytes),
        }
    }

    pub fn failed(bytes: Vec<u8>) -> Self {
        Self {
            status: 1,
            payload: AbiBuffer::from_bytes(bytes),
        }
    }
}

pub type AbiRequestFn = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> AbiCallResult;
pub type AbiReleaseFn = unsafe extern "C" fn(AbiBuffer);
pub type AbiCloseFn = unsafe extern "C" fn(*mut c_void);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AbiHostV1 {
    pub context: *mut c_void,
    pub request: Option<AbiRequestFn>,
    pub release: Option<AbiReleaseFn>,
}

unsafe impl Send for AbiHostV1 {}
unsafe impl Sync for AbiHostV1 {}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AbiPluginV1 {
    pub transport_version: u32,
    pub context: *mut c_void,
    pub request: Option<AbiRequestFn>,
    pub release: Option<AbiReleaseFn>,
    pub close: Option<AbiCloseFn>,
}

pub type AbiEntryV1 = unsafe extern "C" fn(AbiHostV1) -> AbiPluginV1;

pub trait AbiGuest: Send {
    fn request(&mut self, request: &[u8]) -> Vec<u8>;
}

pub fn plugin_api_from_guest(guest: Box<dyn AbiGuest>) -> AbiPluginV1 {
    let context = Box::into_raw(Box::new(Mutex::new(guest))).cast::<c_void>();
    AbiPluginV1 {
        transport_version: ABI_TRANSPORT_VERSION,
        context,
        request: Some(guest_request),
        release: Some(release_buffer),
        close: Some(close_guest),
    }
}

unsafe extern "C" fn guest_request(
    context: *mut c_void,
    request: *const u8,
    request_len: usize,
) -> AbiCallResult {
    if context.is_null() || (request.is_null() && request_len != 0) {
        return AbiCallResult::failed(b"invalid ABI request pointers".to_vec());
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let request = unsafe { std::slice::from_raw_parts(request, request_len) };
        let guest = unsafe { &*(context.cast::<Mutex<Box<dyn AbiGuest>>>()) };
        guest
            .lock()
            .expect("ABI guest mutex poisoned")
            .request(request)
    }));
    match result {
        Ok(response) => AbiCallResult::ok(response),
        Err(_) => AbiCallResult::failed(b"ABI guest panicked".to_vec()),
    }
}

pub(crate) unsafe extern "C" fn release_buffer(buffer: AbiBuffer) {
    if buffer.ptr.is_null() || buffer.len == 0 {
        return;
    }
    let slice = ptr::slice_from_raw_parts_mut(buffer.ptr, buffer.len);
    unsafe { drop(Box::from_raw(slice)) };
}

unsafe extern "C" fn close_guest(context: *mut c_void) {
    if !context.is_null() {
        unsafe { drop(Box::from_raw(context.cast::<Mutex<Box<dyn AbiGuest>>>())) };
    }
}
