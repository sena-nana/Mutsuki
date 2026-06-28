use mutsuki_runtime_contracts::{
    ERR_RESOURCE_GENERATION_MISMATCH, ERR_RESOURCE_NOT_FOUND, ResourceAccess, ResourceId,
    ResourceLifetime, ResourceRef, ResourceSealState, ResourceSemantic, RuntimeError,
};
use serde_json::Value;

use crate::{IdSource, RuntimeFailure, RuntimeResult};

use super::{ResourceManager, hub::ResourceEntry, resource_not_found, simple_hash};

impl ResourceManager {
    pub fn open_resource(&self, ref_id: &str) -> RuntimeResult<ResourceRef> {
        self.hub
            .get(ref_id)
            .map(|entry| entry.descriptor.clone())
            .ok_or_else(|| resource_not_found(format!("resource.open.{ref_id}")))
    }

    pub fn map_resource(&self, ref_id: &str) -> RuntimeResult<ResourceRef> {
        self.open_resource(ref_id)
    }

    pub fn create_mmap_resource(
        &mut self,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        let ref_id = self.id_source.next_id("resource");
        let access = self.backend.mmap_access(&ref_id, &bytes)?;
        let descriptor = resource_descriptor(
            ref_id,
            "bytes",
            ResourceSemantic::FrozenValue,
            schema,
            access,
            Some(bytes.len() as u64),
            Some(simple_hash(&bytes)),
            ResourceLifetime::Persistent,
        );
        self.hub
            .insert(ResourceEntry::new(descriptor.clone(), bytes));
        Ok(descriptor)
    }

    pub fn create_blob_resource(&mut self, schema: &str, bytes: Vec<u8>) -> ResourceRef {
        let ref_id = self.id_source.next_id("resource");
        let access = ResourceAccess::Blob {
            store_id: "resource.local.blob".into(),
            key: ref_id.clone(),
        };
        let descriptor = resource_descriptor(
            ref_id,
            "blob",
            ResourceSemantic::FrozenValue,
            schema,
            access,
            Some(bytes.len() as u64),
            Some(simple_hash(&bytes)),
            ResourceLifetime::Persistent,
        );
        self.hub
            .insert(ResourceEntry::new(descriptor.clone(), bytes));
        descriptor
    }

    pub fn create_cow_state_resource(
        &mut self,
        kind_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        let ref_id = self.id_source.next_id("resource");
        let access = self.backend.mmap_access(&ref_id, &bytes)?;
        let descriptor = resource_descriptor(
            ref_id,
            kind_id,
            ResourceSemantic::CowVersionedState,
            schema,
            access,
            Some(bytes.len() as u64),
            Some(simple_hash(&bytes)),
            ResourceLifetime::Persistent,
        );
        self.hub
            .insert(ResourceEntry::new(descriptor.clone(), bytes));
        Ok(descriptor)
    }

    pub fn create_snapshot_resource(
        &mut self,
        kind_id: &str,
        schema: &str,
        source: &ResourceRef,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        self.read_resource(source)?;
        let ref_id = self.id_source.next_id("resource");
        let access = self.backend.mmap_access(&ref_id, &bytes)?;
        let descriptor = resource_descriptor(
            ref_id,
            kind_id,
            ResourceSemantic::VersionedSnapshot,
            schema,
            access,
            Some(bytes.len() as u64),
            Some(simple_hash(&bytes)),
            ResourceLifetime::Persistent,
        );
        self.hub
            .insert(ResourceEntry::new(descriptor.clone(), bytes));
        Ok(descriptor)
    }

    pub fn create_fact_resource(
        &mut self,
        kind_id: &str,
        schema: &str,
        value: Value,
    ) -> RuntimeResult<ResourceRef> {
        let bytes = serde_json::to_vec(&value).map_err(|err| {
            RuntimeFailure::new(RuntimeError::new(
                "resource.encode_failed",
                "runtime.resource_manager",
                err.to_string(),
            ))
        })?;
        let ref_id = self.id_source.next_id("resource");
        let descriptor = resource_descriptor(
            ref_id,
            kind_id,
            ResourceSemantic::ReadOnlyFact,
            schema,
            ResourceAccess::ProviderRpc {
                provider_id: "resource.local".into(),
                method: "fact.read".into(),
            },
            Some(bytes.len() as u64),
            Some(simple_hash(&bytes)),
            ResourceLifetime::Persistent,
        );
        self.hub
            .insert(ResourceEntry::new(descriptor.clone(), bytes));
        Ok(descriptor)
    }

    pub fn create_capability_resource(&mut self, kind_id: &str, schema: &str) -> ResourceRef {
        let ref_id = self.id_source.next_id("resource");
        let descriptor = resource_descriptor(
            ref_id,
            kind_id,
            ResourceSemantic::CapabilityResource,
            schema,
            ResourceAccess::ProviderRpc {
                provider_id: "resource.local".into(),
                method: "capability.command".into(),
            },
            None,
            None,
            ResourceLifetime::ExternalManaged,
        );
        self.hub
            .insert(ResourceEntry::new(descriptor.clone(), Vec::new()));
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
        let mut descriptor = resource_descriptor(
            ref_id,
            stream_id,
            ResourceSemantic::StreamResource,
            schema,
            ResourceAccess::Stream {
                endpoint: endpoint.into(),
            },
            None,
            None,
            ResourceLifetime::ExternalManaged,
        );
        descriptor.provider_id = provider_id.into();
        self.hub
            .insert(ResourceEntry::new(descriptor.clone(), Vec::new()));
        descriptor
    }

    pub fn close_stream_resource(&mut self, ref_id: &str) -> RuntimeResult<ResourceRef> {
        match self.hub.get(ref_id) {
            Some(entry) if matches!(&entry.descriptor.access, ResourceAccess::Stream { .. }) => {
                Ok(self
                    .hub
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
        let entry = self.hub.get(&resource_ref.ref_id).ok_or_else(|| {
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
        if entry.descriptor.resource_id.generation != resource_ref.resource_id.generation
            || entry.descriptor.resource_id.version != resource_ref.resource_id.version
        {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_RESOURCE_GENERATION_MISMATCH,
                "runtime.resource_manager",
                format!("resource.{}", resource_ref.ref_id),
            )));
        }
        self.backend.read(&entry.descriptor, &entry.bytes)
    }

    pub fn read_resource_by_id(&self, ref_id: &str) -> RuntimeResult<Vec<u8>> {
        let descriptor = self.map_resource(ref_id)?;
        self.read_resource(&descriptor)
    }

    pub fn copy_on_write(
        &mut self,
        base_ref: &ResourceRef,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        self.read_resource(base_ref)?;
        self.create_cow_state_resource(&base_ref.resource_kind, &base_ref.schema, bytes)
    }

    pub fn resource_store_name(&self, ref_id: &str) -> Option<&'static str> {
        self.hub.store_name(ref_id)
    }
}

fn resource_descriptor(
    ref_id: String,
    resource_kind: &str,
    semantic: ResourceSemantic,
    schema: &str,
    access: ResourceAccess,
    size_hint: Option<u64>,
    content_hash: Option<String>,
    lifetime: ResourceLifetime,
) -> ResourceRef {
    ResourceRef {
        resource_id: ResourceId {
            kind_id: resource_kind.into(),
            slot_id: ref_id.clone(),
            generation: 1,
            version: 1,
        },
        ref_id,
        semantic,
        provider_id: "resource.local".into(),
        resource_kind: resource_kind.into(),
        schema: schema.into(),
        version: 1,
        generation: 1,
        access,
        size_hint,
        content_hash,
        lifetime,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}
