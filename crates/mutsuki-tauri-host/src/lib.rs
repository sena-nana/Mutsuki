mod approval;
mod builder;
mod config;
#[cfg(test)]
mod echo;
mod error;
mod health;
mod host;
mod plugin_runner;

pub use approval::{ApprovalBridge, PendingApproval};
pub use builder::MutsukiTauriHostBuilder;
pub use config::{HostMode, MutsukiTauriConfig, PathsConfig, SecurityConfig};
pub use error::{HostError, HostResult};
pub use host::{MAX_RESOURCE_INVOKE_BYTES, MutsukiTauriHost};

#[cfg(test)]
mod tests;
