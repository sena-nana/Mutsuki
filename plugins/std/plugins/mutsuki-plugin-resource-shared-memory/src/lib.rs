// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::similar_names,
    clippy::unused_self
)]

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
