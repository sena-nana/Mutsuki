mod constants;
mod descriptor;
mod error;
mod mapping;
mod plugin;
mod provider;

pub use constants::{PLUGIN_ID, PROVIDER_ID};
pub use plugin::{loaded_plugin, plugin};
pub use provider::SharedMemoryResourceProvider;
