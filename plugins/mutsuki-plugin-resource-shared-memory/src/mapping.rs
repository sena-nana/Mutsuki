use std::fmt;
use std::sync::Arc;

use mutsuki_runtime_contracts::{
    ERR_RESOURCE_NOT_FOUND, ERR_RESOURCE_UNSUPPORTED, ERR_RUNTIME_HOST_FAILED, ResourceAccess,
    ResourceRef,
};
use mutsuki_runtime_core::RuntimeResult;

use crate::constants::{ROUTE_CREATE, ROUTE_READ};
use crate::error::detailed_failure;

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
use shared_memory::{Shmem, ShmemConf};

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(crate) struct OwnedMapping {
    mapping: Shmem,
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub(crate) struct OwnedMapping;

// `Shmem` contains a raw mapping pointer. Mutsuki mappings are initialized before publication and
// never mutated in place: a logical write creates a new mapping generation. The wrapper exposes no
// mutable access, so moving or sharing the immutable generation between provider/view owners is safe.
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
unsafe impl Send for OwnedMapping {}
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
unsafe impl Sync for OwnedMapping {}

/// An owning, process-local view of a shared-memory resource.
///
/// The mapping stays alive for at least as long as this handle. `bytes` borrows from the handle, so
/// no slice or pointer is placed in `ResourceRef` or allowed to outlive the OS mapping.
pub struct SharedMemoryView {
    descriptor: ResourceRef,
    mapping: Arc<OwnedMapping>,
    offset: usize,
    len: usize,
}

impl SharedMemoryView {
    /// Opens the shared mapping named by `resource` without copying its bytes.
    ///
    /// Only descriptors that declare a readonly shared-memory access are accepted. Writable
    /// resources must publish an immutable generation before they can be viewed safely.
    pub fn open(resource: &ResourceRef) -> RuntimeResult<Self> {
        let (name, offset, len, readonly) = access_range(resource, ROUTE_READ)?;
        if !readonly {
            return Err(detailed_failure(
                ERR_RESOURCE_UNSUPPORTED,
                ROUTE_READ,
                "mapped views require a readonly shared-memory generation".to_string(),
            ));
        }
        let mapping = Arc::new(open_mapping(name, ROUTE_READ)?);
        Self::from_mapping(resource.clone(), mapping, offset, len, ROUTE_READ)
    }

    pub(crate) fn from_mapping(
        descriptor: ResourceRef,
        mapping: Arc<OwnedMapping>,
        offset: u64,
        len: u64,
        route: &str,
    ) -> RuntimeResult<Self> {
        let offset = usize::try_from(offset).map_err(|_| range_failure(route))?;
        let len = usize::try_from(len).map_err(|_| range_failure(route))?;
        if offset
            .checked_add(len)
            .is_none_or(|end| end > mapping.len())
        {
            return Err(range_failure(route));
        }
        Ok(Self {
            descriptor,
            mapping,
            offset,
            len,
        })
    }

    /// Returns bytes borrowed from this owning view. No `Vec<u8>` is allocated.
    pub fn bytes(&self) -> &[u8] {
        &self.mapping.bytes()[self.offset..self.offset + self.len]
    }

    pub fn descriptor(&self) -> &ResourceRef {
        &self.descriptor
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) fn mapping(&self) -> Arc<OwnedMapping> {
        Arc::clone(&self.mapping)
    }
}

impl AsRef<[u8]> for SharedMemoryView {
    fn as_ref(&self) -> &[u8] {
        self.bytes()
    }
}

impl fmt::Debug for SharedMemoryView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SharedMemoryView")
            .field("ref_id", &self.descriptor.ref_id)
            .field("generation", &self.descriptor.generation)
            .field("offset", &self.offset)
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

pub(crate) fn create_mapping(name: &str, bytes: &[u8]) -> RuntimeResult<Arc<OwnedMapping>> {
    create_mapping_impl(name, bytes).map(Arc::new)
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn create_mapping_impl(name: &str, bytes: &[u8]) -> RuntimeResult<OwnedMapping> {
    let map_size = bytes.len().max(1);
    let mut mapping = ShmemConf::new()
        .os_id(name)
        .size(map_size)
        .create()
        .map_err(|error| {
            detailed_failure(ERR_RUNTIME_HOST_FAILED, ROUTE_CREATE, error.to_string())
        })?;
    if !bytes.is_empty() {
        // SAFETY: this provider exclusively owns the newly created mapping. Initialization ends
        // before the mapping is wrapped in `Arc` and published as an immutable generation.
        unsafe {
            mapping.as_slice_mut()[..bytes.len()].copy_from_slice(bytes);
        }
    }
    Ok(OwnedMapping { mapping })
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn create_mapping_impl(_name: &str, _bytes: &[u8]) -> RuntimeResult<OwnedMapping> {
    Err(unsupported_platform(ROUTE_CREATE))
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn open_mapping(name: &str, route: &str) -> RuntimeResult<OwnedMapping> {
    ShmemConf::new()
        .os_id(name)
        .open()
        .map(|mapping| OwnedMapping { mapping })
        .map_err(|error| detailed_failure(ERR_RESOURCE_NOT_FOUND, route, error.to_string()))
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn open_mapping(_name: &str, route: &str) -> RuntimeResult<OwnedMapping> {
    Err(unsupported_platform(route))
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
impl OwnedMapping {
    fn len(&self) -> usize {
        self.mapping.len()
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: `OwnedMapping` only publishes immutable generations and exposes no mutable API.
        unsafe { self.mapping.as_slice() }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
impl OwnedMapping {
    fn len(&self) -> usize {
        0
    }

    fn bytes(&self) -> &[u8] {
        &[]
    }
}

fn access_range<'a>(
    resource: &'a ResourceRef,
    route: &str,
) -> RuntimeResult<(&'a str, u64, u64, bool)> {
    match &resource.access {
        ResourceAccess::SharedMemory {
            name,
            offset,
            len,
            readonly,
        } => Ok((name, *offset, *len, *readonly)),
        _ => Err(detailed_failure(
            ERR_RESOURCE_UNSUPPORTED,
            route,
            "resource does not expose shared-memory access".to_string(),
        )),
    }
}

fn range_failure(route: &str) -> mutsuki_runtime_core::RuntimeFailure {
    detailed_failure(
        ERR_RESOURCE_UNSUPPORTED,
        route,
        "shared-memory descriptor range is outside mapping".to_string(),
    )
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn unsupported_platform(route: &str) -> mutsuki_runtime_core::RuntimeFailure {
    detailed_failure(
        ERR_RESOURCE_UNSUPPORTED,
        route,
        "shared mappings are unavailable on this platform; use a provider RPC deployment"
            .to_string(),
    )
}
