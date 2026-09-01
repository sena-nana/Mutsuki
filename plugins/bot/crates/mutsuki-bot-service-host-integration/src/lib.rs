// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::cast_possible_truncation,
    clippy::default_trait_access,
    clippy::doc_markdown,
    clippy::field_reassign_with_default,
    clippy::ignored_unit_patterns,
    clippy::implicit_clone,
    clippy::manual_let_else,
    clippy::map_unwrap_or,
    clippy::match_same_arms,
    clippy::match_wildcard_for_single_variants,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::return_self_not_must_use,
    clippy::too_many_lines,
    clippy::trivially_copy_pass_by_ref,
    clippy::type_complexity,
    clippy::unchecked_time_subtraction,
    clippy::unnecessary_literal_bound
)]

mod ai_bundle;
mod bilibili;
mod bundle;
mod configured;
mod console_bridge;
mod delivery_recovery;
mod event_source;
mod orchestrated_flow;
mod sandbox_intercept;

pub use ai_bundle::QqAiBotPluginBundle;
pub use bilibili::{BilibiliPollingCredentials, BilibiliPollingEventSource};
pub use bundle::QqBotPluginBundle;
pub use configured::*;
pub use console_bridge::{
    AgentConnectionConsoleBridge, AgentConnectionRegistryConsoleBridge,
    BILIBILI_MANAGEMENT_SERVICE_ID, BOT_STATE_DB_SERVICE_ID, BilibiliConsoleBridge,
    BotAgentConfigConsoleBridge, BotDatabaseConsoleBridge, BotFlowConsoleBridge,
    LocalAgentConsoleBridge, QQ_MANAGEMENT_SERVICE_ID, QqConsoleBridge, SANDBOX_SERVICE_ID,
    SandboxConsoleBridge, bot_state_db_host_service,
};
pub use delivery_recovery::{
    BOT_REPLY_DELIVERY_RECOVERY_SOURCE_ID, BotReplyDeliveryRecoveryEventSource,
};
pub use event_source::{
    QQBOT_GATEWAY_SOURCE_ID, QqGatewayControlHandle, QqGatewayEventSource, QqGatewayHealthHandle,
    QqGatewayHealthSnapshot, QqInboundObserveHandle,
};
pub use mutsuki_plugin_bot_delivery::{
    bot_delivery_manifest, bot_reply_delivery_manifest, bot_reply_delivery_manifest_for,
    bot_scheduled_delivery_manifest,
};
pub use mutsuki_plugin_bot_interaction::bot_interaction_manifest;
pub use orchestrated_flow::{
    QQ_AI_PRESENTATION_FAILURE_TEXT, bilibili_push_flow, qq_ai_orchestrated_flow,
    qq_ai_orchestrated_flow_with_source, qq_full_business_flow, qq_link_resolve_flow,
};
