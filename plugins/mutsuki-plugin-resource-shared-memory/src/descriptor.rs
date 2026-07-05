use mutsuki_runtime_contracts::{
    ERR_RESOURCE_GENERATION_MISMATCH, ResourceAccess, ResourceId, ResourceLifetime,
    ResourceProviderCompatibility, ResourceProviderReloadPolicy, ResourceRef, ResourceSealState,
    ResourceSemantic, ResourceTypeDescriptor,
};
use mutsuki_runtime_core::RuntimeResult;

use crate::constants::PROVIDER_ID;
use crate::error::{runtime_failure, unsupported};

pub(crate) fn resource_type(
    kind_id: &str,
    semantic: ResourceSemantic,
    schema: &str,
    operations: &[&str],
) -> ResourceTypeDescriptor {
    ResourceTypeDescriptor {
        kind_id: kind_id.into(),
        semantic,
        schema: schema.into(),
        provider_id: PROVIDER_ID.into(),
        operations: operations
            .iter()
            .map(|operation| (*operation).into())
            .collect(),
        reload_policy: ResourceProviderReloadPolicy::CompatibleWithoutLeases,
        compatibility: ResourceProviderCompatibility {
            schema_version: "1.0.0".into(),
            required_operations: operations
                .iter()
                .map(|operation| (*operation).into())
                .collect(),
            preserves_resource_type_id: true,
            accepts_older_generations: false,
            lease_drain_required: false,
        },
    }
}

pub(crate) fn resource_ref(
    ref_id: &str,
    kind_id: &str,
    semantic: ResourceSemantic,
    schema: &str,
    version: u64,
    mapping_name: &str,
    len: u64,
    readonly: bool,
) -> ResourceRef {
    ResourceRef {
        ref_id: ref_id.into(),
        resource_id: ResourceId {
            kind_id: kind_id.into(),
            slot_id: ref_id.into(),
            generation: 1,
            version,
        },
        semantic,
        provider_id: PROVIDER_ID.into(),
        resource_kind: kind_id.into(),
        schema: schema.into(),
        version,
        generation: 1,
        access: ResourceAccess::SharedMemory {
            name: mapping_name.into(),
            offset: 0,
            len,
            readonly,
        },
        size_hint: Some(len),
        content_hash: None,
        lifetime: ResourceLifetime::Persistent,
        lease: None,
        seal_state: if readonly {
            ResourceSealState::Sealed
        } else {
            ResourceSealState::Writable
        },
    }
}

pub(crate) fn shared_memory_access<'a>(
    resource: &'a ResourceRef,
    route: &str,
) -> RuntimeResult<(&'a str, u64, u64)> {
    match &resource.access {
        ResourceAccess::SharedMemory {
            name, offset, len, ..
        } => Ok((name, *offset, *len)),
        _ => Err(unsupported(route, "non_shared_memory_resource")),
    }
}

pub(crate) fn ensure_provider(resource: &ResourceRef, route: &str) -> RuntimeResult<()> {
    if resource.provider_id != PROVIDER_ID {
        return Err(unsupported(route, &resource.provider_id));
    }
    Ok(())
}

pub(crate) fn ensure_descriptor_self_consistent(
    resource: &ResourceRef,
    route: &str,
) -> RuntimeResult<()> {
    if resource.resource_id.generation != resource.generation
        || resource.resource_id.version != resource.version
    {
        return Err(runtime_failure(
            ERR_RESOURCE_GENERATION_MISMATCH,
            format!("{route}.{}", resource.ref_id),
        ));
    }
    Ok(())
}

pub(crate) fn ensure_descriptor_current(
    requested: &ResourceRef,
    current: &ResourceRef,
    route: &str,
) -> RuntimeResult<()> {
    if requested.generation != current.generation
        || requested.version != current.version
        || requested.resource_id.generation != requested.generation
        || requested.resource_id.version != requested.version
    {
        return Err(runtime_failure(
            ERR_RESOURCE_GENERATION_MISMATCH,
            format!("{route}.{}", requested.ref_id),
        ));
    }
    Ok(())
}
