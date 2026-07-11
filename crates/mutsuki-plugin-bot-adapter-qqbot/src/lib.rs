pub mod adapter;
pub mod api;
pub mod bundle;
pub mod config;
pub mod gateway;
pub mod tasks;

pub use api::*;
pub use bundle::QqBotPluginBundle;
pub use config::{DEFAULT_QQBOT_INTENTS, QqBotConfig, QqConfigError};
pub use gateway::*;
pub use tasks::{QQBOT_ADAPTER_PLUGIN_ID, qqbot_adapter_manifest, qqbot_runners};

#[cfg(test)]
mod tests;
