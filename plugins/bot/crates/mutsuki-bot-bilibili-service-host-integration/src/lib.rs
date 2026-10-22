//! `ServiceHost` assembly of the Bilibili owner: the configured plugin factory, the polling
//! `EventSource` wrapper and the credential/config stores backed by Host services. Polling and
//! management logic stay in `mutsuki-plugin-bot-bilibili`.

// Pedantic lints inherited from the integration code this crate was split out of; listed so the
// remaining debt stays auditable.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::too_many_lines
)]

mod configured;
mod polling;

pub use configured::{BilibiliConfiguredPlugin, configured_bilibili_plugin_catalog};
pub use polling::{BilibiliPollingCredentials, BilibiliPollingEventSource};
