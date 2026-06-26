use std::fs;

use mutsuki_runtime_contracts::{
    ERR_RESOURCE_GENERATION_MISMATCH, ERR_RESOURCE_NOT_FOUND, ResourceAccess, ResourceLifetime,
    ResourceRef, ResourceSealState, RuntimeError,
};

use crate::{IdSource, RuntimeFailure, RuntimeResult};

use super::{ResourceEntry, ResourceManager, io_failure, resource_not_found, simple_hash};

impl ResourceManager {
    pub fn create_mmap_resource(
        &mut self,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        fs::create_dir_all(&self.root).map_err(io_failure)?;
        let ref_id = self.id_source.next_id("resource");
        let path = self.root.join(format!("{ref_id}.bin"));
        fs::write(&path, &bytes).map_err(io_failure)?;
        let descriptor = ResourceRef {
            ref_id: ref_id.clone(),
            provider_id: "resource.local".into(),
            resource_kind: "bytes".into(),
            schema: schema.into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::MmapFile {
                path: path.to_string_lossy().to_string(),
                offset: 0,
                len: bytes.len() as u64,
                readonly: true,
            },
            size_hint: Some(bytes.len() as u64),
            content_hash: Some(simple_hash(&bytes)),
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        };
        self.resources
            .insert(ref_id, ResourceEntry::new(descriptor.clone(), bytes));
        Ok(descriptor)
    }

    pub fn create_blob_resource(&mut self, schema: &str, bytes: Vec<u8>) -> ResourceRef {
        let ref_id = self.id_source.next_id("resource");
        let descriptor = ResourceRef {
            ref_id: ref_id.clone(),
            provider_id: "resource.local".into(),
            resource_kind: "blob".into(),
            schema: schema.into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::Blob {
                store_id: "resource.local.blob".into(),
                key: ref_id.clone(),
            },
            size_hint: Some(bytes.len() as u64),
            content_hash: Some(simple_hash(&bytes)),
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        };
        self.resources
            .insert(ref_id, ResourceEntry::new(descriptor.clone(), bytes));
        descriptor
    }

    pub fn create_stream_resource(
        &mut self,
        stream_id: &str,
        schema: &str,
        provider_id: &str,
        endpoint: &str,
    ) -> ResourceRef {
        let ref_id = self.id_source.next_id("resource");
        let descriptor = ResourceRef {
            ref_id: ref_id.clone(),
            provider_id: provider_id.into(),
            resource_kind: stream_id.into(),
            schema: schema.into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::Stream {
                endpoint: endpoint.into(),
            },
            size_hint: None,
            content_hash: None,
            lifetime: ResourceLifetime::ExternalManaged,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        };
        self.resources
            .insert(ref_id, ResourceEntry::new(descriptor.clone(), Vec::new()));
        descriptor
    }

    pub fn close_stream_resource(&mut self, ref_id: &str) -> RuntimeResult<ResourceRef> {
        match self.resources.get(ref_id) {
            Some(entry) if matches!(&entry.descriptor.access, ResourceAccess::Stream { .. }) => {
                Ok(self
                    .resources
                    .remove(ref_id)
                    .expect("stream resource was checked before remove")
                    .descriptor)
            }
            _ => Err(resource_not_found(format!(
                "resource.stream.close.{ref_id}"
            ))),
        }
    }

    pub fn read_resource(&self, resource_ref: &ResourceRef) -> RuntimeResult<Vec<u8>> {
        let entry = self.resources.get(&resource_ref.ref_id).ok_or_else(|| {
            RuntimeFailure::new(RuntimeError::new(
                ERR_RESOURCE_NOT_FOUND,
                "runtime.resource_manager",
                format!("resource.{}", resource_ref.ref_id),
            ))
        })?;
        if entry.descriptor.generation != resource_ref.generation {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_RESOURCE_GENERATION_MISMATCH,
                "runtime.resource_manager",
                format!("resource.{}", resource_ref.ref_id),
            )));
        }
        match &entry.descriptor.access {
            ResourceAccess::MmapFile { path, .. } => fs::read(path).map_err(io_failure),
            _ => Ok(entry.bytes.clone()),
        }
    }

    pub fn copy_on_write(
        &mut self,
        base_ref: &ResourceRef,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        self.read_resource(base_ref)?;
        self.create_mmap_resource(&base_ref.schema, bytes)
    }
}
