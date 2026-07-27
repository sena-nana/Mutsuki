//! Constructors for provider-backed Agent resource handles.

use mutsuki_agent_protocol::{ResourceCellRef, ResourceRef};
use mutsuki_runtime_sdk::contracts::{
    ResourceAccess, ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic,
};

fn resource_ref(
    kind: &str,
    provider_id: impl Into<String>,
    slot_id: impl Into<String>,
    semantic: ResourceSemantic,
    lifetime: ResourceLifetime,
    seal_state: ResourceSealState,
    access: ResourceAccess,
) -> ResourceRef {
    let slot_id = slot_id.into();
    let ref_id = format!("{kind}:{slot_id}");
    ResourceRef {
        ref_id: ref_id.clone(),
        resource_id: ResourceId {
            kind_id: kind.into(),
            slot_id,
            generation: 1,
            version: 1,
        },
        semantic,
        provider_id: provider_id.into(),
        resource_kind: kind.into(),
        schema: format!("{kind}.v1"),
        version: 1,
        generation: 1,
        access,
        size_hint: None,
        content_hash: None,
        lifetime,
        lease: None,
        seal_state,
    }
}

pub fn memory_resource_ref(
    provider_id: impl Into<String>,
    memory_id: impl Into<String>,
) -> ResourceRef {
    let provider_id = provider_id.into();
    resource_ref(
        "mutsuki.agent.memory",
        provider_id.clone(),
        memory_id,
        ResourceSemantic::CowVersionedState,
        ResourceLifetime::Persistent,
        ResourceSealState::Writable,
        ResourceAccess::ProviderRpc {
            provider_id,
            method: "memory.read".into(),
        },
    )
}

pub fn memory_cell_ref(
    owner_plugin_id: impl Into<String>,
    memory_id: impl Into<String>,
) -> ResourceCellRef {
    let memory_id = memory_id.into();
    ResourceCellRef {
        cell_id: format!("agent-memory-cell:{memory_id}"),
        resource_kind: "mutsuki.agent.memory".into(),
        owner_plugin_id: owner_plugin_id.into(),
        schema: "mutsuki.agent.memory.v1".into(),
        generation: 1,
        health: "ready".into(),
        reload_policy: "compatible_without_leases".into(),
    }
}

pub fn stream_resource_ref(
    provider_id: impl Into<String>,
    stream_id: impl Into<String>,
) -> ResourceRef {
    let provider_id = provider_id.into();
    resource_ref(
        "mutsuki.agent.stream",
        provider_id.clone(),
        stream_id,
        ResourceSemantic::StreamResource,
        ResourceLifetime::BorrowedUntilTaskEnd,
        ResourceSealState::Writable,
        ResourceAccess::ProviderRpc {
            provider_id,
            method: "stream.read".into(),
        },
    )
}

pub fn session_resource_ref(
    provider_id: impl Into<String>,
    session_id: impl Into<String>,
) -> ResourceRef {
    let provider_id = provider_id.into();
    resource_ref(
        "mutsuki.agent.session",
        provider_id.clone(),
        session_id,
        ResourceSemantic::CowVersionedState,
        ResourceLifetime::ExternalManaged,
        ResourceSealState::Writable,
        ResourceAccess::ProviderRpc {
            provider_id,
            method: "session.read".into(),
        },
    )
}

pub fn session_cell_ref(
    owner_plugin_id: impl Into<String>,
    session_id: impl Into<String>,
) -> ResourceCellRef {
    let session_id = session_id.into();
    ResourceCellRef {
        cell_id: format!("agent-session-cell:{session_id}"),
        resource_kind: "mutsuki.agent.session".into(),
        owner_plugin_id: owner_plugin_id.into(),
        schema: "mutsuki.agent.session.v1".into(),
        generation: 1,
        health: "ready".into(),
        reload_policy: "compatible_without_leases".into(),
    }
}
