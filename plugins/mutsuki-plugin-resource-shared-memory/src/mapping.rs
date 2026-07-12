use shared_memory::{Shmem, ShmemConf};

use mutsuki_runtime_contracts::{
    ERR_RESOURCE_NOT_FOUND, ERR_RESOURCE_UNSUPPORTED, ERR_RUNTIME_HOST_FAILED,
};
use mutsuki_runtime_core::RuntimeResult;

use crate::constants::ROUTE_CREATE;
use crate::error::detailed_failure;

pub(crate) struct OwnedMapping {
    _mapping: Shmem,
}

// `Shmem` contains a raw mapping pointer, so auto traits cannot prove that moving it is safe.
// The provider only accesses mappings while holding its mutex and exposes bytes through copies.
unsafe impl Send for OwnedMapping {}

pub(crate) fn create_mapping(name: &str, bytes: &[u8]) -> RuntimeResult<OwnedMapping> {
    let map_size = bytes.len().max(1);
    let mut mapping = ShmemConf::new()
        .os_id(name)
        .size(map_size)
        .create()
        .map_err(|error| {
            detailed_failure(ERR_RUNTIME_HOST_FAILED, ROUTE_CREATE, error.to_string())
        })?;
    if !bytes.is_empty() {
        // SAFETY: the mapping was just created by this provider and is not shared with callers yet.
        unsafe {
            mapping.as_slice_mut()[..bytes.len()].copy_from_slice(bytes);
        }
    }
    Ok(OwnedMapping { _mapping: mapping })
}

fn open_mapping(name: &str, route: &str) -> RuntimeResult<Shmem> {
    ShmemConf::new()
        .os_id(name)
        .open()
        .map_err(|error| detailed_failure(ERR_RESOURCE_NOT_FOUND, route, error.to_string()))
}

pub(crate) fn read_mapping(
    name: &str,
    offset: u64,
    len: u64,
    route: &str,
) -> RuntimeResult<Vec<u8>> {
    let mapping = open_mapping(name, route)?;
    let offset = offset as usize;
    let len = len as usize;
    if offset
        .checked_add(len)
        .is_none_or(|end| end > mapping.len())
    {
        return Err(detailed_failure(
            ERR_RESOURCE_UNSUPPORTED,
            route,
            "shared-memory descriptor range is outside mapping".to_string(),
        ));
    }
    // SAFETY: bytes are copied out immediately; no borrowed slice crosses the provider boundary.
    Ok(unsafe { mapping.as_slice()[offset..offset + len].to_vec() })
}
