use std::fs;
use std::path::PathBuf;

use mutsuki_runtime_contracts::{ResourceAccess, ResourceRef};

use crate::RuntimeResult;

use super::io_failure;

#[derive(Clone, Debug)]
pub(super) struct LocalResourceBackend {
    root: PathBuf,
}

impl LocalResourceBackend {
    pub(super) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(super) fn mmap_access(&self, ref_id: &str, bytes: &[u8]) -> RuntimeResult<ResourceAccess> {
        fs::create_dir_all(&self.root).map_err(io_failure)?;
        let path = self.root.join(format!("{ref_id}.bin"));
        fs::write(&path, bytes).map_err(io_failure)?;
        Ok(ResourceAccess::MmapFile {
            path: path.to_string_lossy().to_string(),
            offset: 0,
            len: bytes.len() as u64,
            readonly: true,
        })
    }

    pub(super) fn read(
        &self,
        descriptor: &ResourceRef,
        stored_bytes: &[u8],
    ) -> RuntimeResult<Vec<u8>> {
        match &descriptor.access {
            ResourceAccess::MmapFile { path, .. } => fs::read(path).map_err(io_failure),
            _ => Ok(stored_bytes.to_vec()),
        }
    }

    pub(super) fn write(&self, descriptor: &mut ResourceRef, bytes: &[u8]) -> RuntimeResult<()> {
        if let ResourceAccess::MmapFile { path, len, .. } = &mut descriptor.access {
            fs::write(path, bytes).map_err(io_failure)?;
            *len = bytes.len() as u64;
        }
        Ok(())
    }
}
