use std::sync::Arc;

use mutsuki_runtime_contracts::ResourceSemantic;
use mutsuki_runtime_sdk::{LoadedPlugin, Plugin, PluginBuilder};

use crate::constants::{BLOB_KIND_ID, PLUGIN_ID, PROVIDER_ID, SNAPSHOT_KIND_ID};
use crate::descriptor::resource_type;
use crate::provider::SharedMemoryResourceProvider;

pub fn loaded_plugin() -> LoadedPlugin {
    plugin_builder().build()
}

pub fn plugin() -> impl Plugin {
    plugin_builder()
}

fn plugin_builder() -> PluginBuilder {
    PluginBuilder::new(PLUGIN_ID)
        .resource_provider_gateway(PROVIDER_ID, Arc::new(SharedMemoryResourceProvider::new()))
        .resource_type_descriptor(resource_type(
            BLOB_KIND_ID,
            ResourceSemantic::FrozenValue,
            "mutsuki.resource.shared_memory.blob.v1",
            &["collect", "get", "snapshot", "export"],
        ))
        .resource_type_descriptor(resource_type(
            SNAPSHOT_KIND_ID,
            ResourceSemantic::VersionedSnapshot,
            "mutsuki.resource.shared_memory.snapshot.v1",
            &["collect", "get", "export"],
        ))
}
