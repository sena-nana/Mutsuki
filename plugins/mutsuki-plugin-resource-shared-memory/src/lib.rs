mod constants;
mod descriptor;
mod error;
mod mapping;
mod plugin;
mod provider;

pub use constants::{PLUGIN_ID, PROVIDER_ID};
pub use mapping::SharedMemoryView;
pub use plugin::{loaded_plugin, plugin};
pub use provider::{
    DEFAULT_MAX_COLLECT_BYTES, SharedMemoryCopyMetrics, SharedMemoryProviderConfig,
    SharedMemoryResourceProvider,
};
