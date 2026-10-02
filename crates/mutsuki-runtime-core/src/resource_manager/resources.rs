use mutsuki_runtime_contracts::{
    ERR_CAPABILITY_EXHAUSTED, PlanReceipt, ResourceAccess, ResourceId, ResourceLifetime,
    ResourceRef, ResourceSealState, ResourceSemantic,
};

use crate::{IdSource, RuntimeResult};

use super::{
    ResourceManager, hub::ResourceEntry, receipt_descriptors, resource_generation_mismatch,
    resource_not_found,
};

/// Validate the identity fields that make a descriptor safe to put in the
/// hub.  A zero generation/version cannot be addressed by the provider
/// lifecycle contracts (invalidations reject generation zero), so accepting
/// one during restore would create a permanently unreachable descriptor.
fn validate_descriptor_identity(descriptor: &ResourceRef, route: &str) -> RuntimeResult<()> {
    if descriptor.generation == 0
        || descriptor.version == 0
        || descriptor.resource_id.generation != descriptor.generation
        || descriptor.resource_id.version != descriptor.version
    {
        return Err(resource_generation_mismatch(format!(
            "{route}.{}",
            descriptor.ref_id
        )));
    }
    Ok(())
}

impl ResourceManager {
    /// Validate the complete removal set before changing the registry.
    pub fn invalidate_resource_descriptors(
        &mut self,
        provider_id: &str,
        invalidations: &[mutsuki_runtime_contracts::ResourceDescriptorInvalidation],
    ) -> RuntimeResult<()> {
        for removed in invalidations {
            if removed.provider_id != provider_id || removed.generation == 0 {
                return Err(resource_generation_mismatch(format!(
                    "resource.invalidate.{}",
                    removed.ref_id
                )));
            }
            if let Some(entry) = self.hub.get(&removed.ref_id)
                && (entry.descriptor.provider_id != provider_id
                    || entry.descriptor.generation != removed.generation)
            {
                return Err(resource_generation_mismatch(format!(
                    "resource.invalidate.{}",
                    removed.ref_id
                )));
            }
        }
        for removed in invalidations {
            self.hub.remove(&removed.ref_id);
        }
        Ok(())
    }

    pub fn open_resource(&self, ref_id: impl AsRef<str>) -> RuntimeResult<ResourceRef> {
        let ref_id = ref_id.as_ref();
        self.hub
            .get(ref_id)
            .map(|entry| entry.descriptor.clone())
            .ok_or_else(|| resource_not_found(format!("resource.open.{ref_id}")))
    }

    /// Read-only inventory of currently registered resource descriptors.
    pub fn list_descriptors(&self) -> Vec<ResourceRef> {
        let mut descriptors: Vec<_> = self
            .hub
            .entries()
            .map(|entry| entry.descriptor.clone())
            .collect();
        descriptors.sort_by(|left, right| left.ref_id.cmp(&right.ref_id));
        descriptors
    }

    pub fn register_resource_descriptor(
        &mut self,
        descriptor: ResourceRef,
    ) -> RuntimeResult<ResourceRef> {
        validate_descriptor_identity(&descriptor, "resource.register")?;
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

    /// Drops a hub descriptor. Missing ids succeed: retention and delete both
    /// need to be safe if the hub never observed the row.
    pub fn unregister_resource(&mut self, ref_id: impl AsRef<str>) -> RuntimeResult<()> {
        self.hub.remove(ref_id.as_ref());
        Ok(())
    }

    pub fn sync_resource_descriptor(
        &mut self,
        descriptor: ResourceRef,
    ) -> RuntimeResult<ResourceRef> {
        validate_descriptor_identity(&descriptor, "resource.sync")?;

        if let Some(existing) = self.hub.get(&descriptor.ref_id).cloned() {
            if descriptor.generation < existing.descriptor.generation
                || (descriptor.generation == existing.descriptor.generation
                    && descriptor.version < existing.descriptor.version)
            {
                return Err(resource_generation_mismatch(format!(
                    "resource.sync.{}",
                    descriptor.ref_id
                )));
            }
            if existing
                .writer
                .as_ref()
                .is_some_and(|writer| writer.generation != descriptor.generation)
            {
                return Err(resource_generation_mismatch(format!(
                    "resource.sync.{}",
                    descriptor.ref_id
                )));
            }
            let writer = existing.writer;
            self.hub.remove(&descriptor.ref_id);
            let mut entry = ResourceEntry::new(descriptor.clone());
            entry.writer = writer;
            self.hub.insert(entry);
            return Ok(descriptor);
        }

        self.hub.insert(ResourceEntry::new(descriptor.clone()));
        Ok(descriptor)
    }

    pub fn sync_plan_receipt(&mut self, receipt: &PlanReceipt) -> RuntimeResult<Vec<ResourceRef>> {
        let synced = receipt_descriptors(receipt)
            .into_iter()
            .map(|descriptor| self.sync_resource_descriptor(descriptor))
            .collect::<RuntimeResult<Vec<_>>>()?;
        for ref_id in &receipt.descriptor_removals {
            self.unregister_resource(ref_id)?;
        }
        Ok(synced)
    }

    pub fn create_stream_resource(
        &mut self,
        stream_id: &str,
        schema: &str,
        provider_id: &str,
        endpoint: &str,
    ) -> ResourceRef {
        let ref_id = mutsuki_runtime_contracts::RefId::from(self.id_source.next_id("resource"));
        let descriptor = ResourceRef {
            resource_id: ResourceId {
                kind_id: stream_id.into(),
                slot_id: ref_id.to_string(),
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

    pub fn close_stream_resource(&mut self, ref_id: impl AsRef<str>) -> RuntimeResult<ResourceRef> {
        let ref_id = ref_id.as_ref();
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

    pub fn resource_store_name(&self, ref_id: impl AsRef<str>) -> Option<&'static str> {
        self.hub.store_name(ref_id)
    }
}
