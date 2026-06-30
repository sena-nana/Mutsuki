pub mod adapter;
pub mod api;
pub mod config;
pub mod gateway;
pub mod tasks;

pub use api::{QqBotClients, QqHttpClient, QqHttpRequest, QqHttpResponse, QqIdSource};
pub use config::QqBotConfig;
pub use gateway::{GatewayAction, GatewayFrame, QqGatewayPump};
pub use tasks::{QQBOT_ADAPTER_PLUGIN_ID, qqbot_runners};

#[cfg(test)]
mod tests;
