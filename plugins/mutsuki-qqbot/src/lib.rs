pub mod auth;
pub mod config;
pub mod gateway;
pub mod manifest;
pub mod media;
pub mod openapi;
pub mod payload;
pub mod redaction;
pub mod runner;

pub use config::QqBotConfig;
pub use gateway::{GatewayAction, GatewayFrame, QqGatewayPump};
pub use manifest::{
    EFFECT_INTERACTION_ACK, EFFECT_MEDIA_UPLOAD, EFFECT_MESSAGE_RECALL, EFFECT_MESSAGE_SEND,
    EFFECT_RUNNER_ID, EFFECT_USER_SHARE_LINK, GATEWAY_NORMALIZER_RUNNER_ID, PLUGIN_ID,
    RAW_GATEWAY_TASK_KIND, qqbot_manifest,
};
pub use openapi::{QqBotClients, QqHttpClient, QqHttpRequest, QqHttpResponse, QqIdSource};
pub use runner::qqbot_runners;

#[cfg(test)]
mod tests;
