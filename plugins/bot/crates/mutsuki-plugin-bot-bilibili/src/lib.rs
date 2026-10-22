// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::assigning_clones,
    clippy::cast_lossless,
    clippy::cast_possible_wrap,
    clippy::default_trait_access,
    clippy::if_not_else,
    clippy::manual_string_new,
    clippy::map_unwrap_or,
    clippy::match_same_arms,
    clippy::missing_errors_doc,
    clippy::missing_fields_in_debug,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::needless_update,
    clippy::redundant_closure_for_method_calls,
    clippy::return_self_not_must_use,
    clippy::similar_names,
    clippy::too_many_lines,
    clippy::trivially_copy_pass_by_ref,
    clippy::unnecessary_wraps,
    clippy::unreadable_literal,
    clippy::useless_vec
)]

use mutsuki_bot_link_parser::MAX_LINK_CARD_MEDIA_BYTES;

mod commands;
mod config;
mod error;
mod management;
mod manifest;
mod model;
mod parse;
mod poller;
mod render;
mod repository;
mod runner;
mod transport;

#[cfg(test)]
mod tests;

pub use config::{
    BILIBILI_COOKIE_FIELD, BILIBILI_COOKIE_KEY, bilibili_config_descriptor,
    bilibili_config_from_value, bilibili_config_value,
};
pub use error::BilibiliError;
pub use management::{BilibiliManagementService, BilibiliQrRenderer, BilibiliSecretPresence};
pub use manifest::{manifest, manifest_for_config, manifest_with_surfaces};
pub use model::{
    BilibiliBackendConfig, BilibiliConfig, BilibiliConfigStore, BilibiliCredentialStore,
    BilibiliItem, BilibiliManagementConfig, BilibiliNotification, BilibiliPollKind,
    BilibiliProfile, BilibiliQrCode, BilibiliQrStatus, BilibiliRiskControlBackend,
    BilibiliRiskControlConfig, BilibiliSubscription, LinkResolveRequest, LinkResolverConfig,
    PollRequest, RetryConfig, SharedBilibiliConfig,
};
pub use poller::{BilibiliPollerHealth, run_bilibili_polling};
pub use render::RuntimeBilibiliQrRenderer;
pub use repository::SqliteBilibiliRepository;
pub use runner::BilibiliRunner;
pub use transport::{
    BilibiliHttp, BilibiliTransport, COOKIE_DOMAIN, HTTP_DOMAINS, HttpBilibiliTransport,
    RuntimeBilibiliHttp, sign_wbi_query,
};

pub const PLUGIN_ID: &str = "mutsuki.bot.bilibili";
pub const RUNNER_ID: &str = "mutsuki.bot.bilibili.runner";
pub const POLL_LIVE: &str = "mutsuki.bot.bilibili.poll/live@1";
pub const POLL_DYNAMIC: &str = "mutsuki.bot.bilibili.poll/dynamic@1";
pub const POLL_VIDEO: &str = "mutsuki.bot.bilibili.poll/video@1";
pub const NOTIFY_CARD: &str = "mutsuki.bot.bilibili.card/render@1";
pub const LINK_RESOLVE: &str = "mutsuki.bot.bilibili.link/resolve@1";
pub const MANAGEMENT_COMMAND: &str = "mutsuki.bot.bilibili.management/command@1";
pub const RISK_CONTROL_STATUS_EVENT: &str = "mutsuki.bot.bilibili.risk_control/status@1";
pub const BILIBILI_EVENT_TYPE: &str = "mutsuki.bot.event.bilibili";
pub const BILIBILI_NOTIFICATION_NODE_TYPE: &str = "mutsuki.bot.bilibili.notification";
pub const BILIBILI_CARD_NODE_TYPE: &str = "mutsuki.bot.bilibili.card";
pub const MAX_MEDIA_BYTES: usize = MAX_LINK_CARD_MEDIA_BYTES;
