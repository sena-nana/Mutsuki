pub mod adapter;
pub mod api;
pub mod config;
pub mod gateway;
pub mod inbound_media;
pub mod tasks;

pub use api::*;
pub use config::{
    DEFAULT_QQBOT_INTENTS, QQ_CLIENT_SECRET_FIELD, QQ_CLIENT_SECRET_KEY, QQ_INTENT_GROUP_AND_C2C,
    QQ_INTENT_PUBLIC_GUILD, QQ_RECEIVE_GUILD_FIELD, QQ_RECEIVE_PRIVATE_AND_GROUP_FIELD,
    QqBotConfig, QqConfigError, apply_receive_intents, qq_config_descriptor, qq_config_value,
    receive_guild, receive_private_and_group, validate_gateway_url,
};
pub use gateway::*;
pub use inbound_media::*;
pub use tasks::{
    QQBOT_ADAPTER_PLUGIN_ID, QqGatewayMapRunner, QqOpenApiRunner, openapi_descriptor,
    qqbot_adapter_manifest, qqbot_runners,
};

#[cfg(test)]
mod tests;
