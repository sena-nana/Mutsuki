//! ServiceHost assembly of the Bilibili owner: the configured plugin factory, the polling
//! EventSource wrapper and the credential/config stores backed by Host services. Polling and
//! management logic stay in `mutsuki-plugin-bot-bilibili`.

#![allow(clippy::missing_errors_doc, clippy::too_many_lines)]

mod configured;
mod polling;

pub use configured::{BilibiliConfiguredPlugin, configured_bilibili_plugin_catalog};
pub use polling::{BilibiliPollingCredentials, BilibiliPollingEventSource};
