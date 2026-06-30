use mutsuki_runtime_contracts::{
    ERR_CAPABILITY_EXHAUSTED, ERR_RESOURCE_GENERATION_MISMATCH, ResourceAccess, ResourceId,
    ResourceLifetime, ResourceRef, ResourceSealState, ResourceSemantic,
};

use crate::{IdSource, RuntimeResult};

use super::{ResourceManager, hub::ResourceEntry, resource_not_found};

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

    pub fn register_resource_descriptor(
        &mut self,
        descriptor: ResourceRef,
    ) -> RuntimeResult<ResourceRef> {
        if descriptor.resource_id.generation != descriptor.generation
            || descriptor.resource_id.version != descriptor.version
        {
            return Err(crate::runtime_failure(
                ERR_RESOURCE_GENERATION_MISMATCH,
                "runtime.resource_manager",
                format!("resource.register.{}", descriptor.ref_id),
            ));
        }
        if self.hub.get(&descriptor.ref_id).is_some() {
            return Err(crate::runtime_failure(
                ERR_CAPABILITY_EXHAUSTED,
                "runtime.resource_manager",
                format!("resource.register.{}", descriptor.ref_id),
            ));
        }
        self.hub.insert(ResourceEntry::new(descriptor.clone()));
        Ok(descriptor)
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
            resource_id: ResourceId {
                kind_id: stream_id.into(),
                slot_id: ref_id.clone(),
                generation: 1,
                version: 1,
            },
            ref_id,
            semantic: ResourceSemantic::StreamResource,
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
        self.hub.insert(ResourceEntry::new(descriptor.clone()));
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

    pub fn resource_store_name(&self, ref_id: &str) -> Option<&'static str> {
        self.hub.store_name(ref_id)
    }
}
