pub mod adapter;
pub mod api;
pub mod config;
pub mod gateway;
pub mod tasks;

pub use api::*;
pub use config::{DEFAULT_QQBOT_INTENTS, QqBotConfig, QqConfigError, validate_gateway_url};
pub use gateway::*;
pub use tasks::{
    QQBOT_ADAPTER_PLUGIN_ID, QqGatewayMapRunner, QqOpenApiRunner, openapi_descriptor,
    qqbot_adapter_manifest, qqbot_runners,
};

#[cfg(test)]
mod tests;
