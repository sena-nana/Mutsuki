//! MutsukiLink control bridge for in-process and library consumers.
//!
//! When IPC is enabled, ServiceRuntime exposes a stable `mutsuki.servicehost`
//! local app endpoint via [`LinkControlServer`]. This crate is the explicit
//! integration point between the ServiceHost control plane and the local Link
//! transport; remote (QUIC) control is not part of the product Host.
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::default_trait_access,
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value
)]

mod client;
mod protocol;
mod server;
mod transport;

pub use client::{
    LinkControlHandler, STANDALONE_LINK_CONNECT_FAILED, STANDALONE_LINK_PROTOCOL_ERROR,
    STANDALONE_LINK_REJECTED,
};
pub use protocol::{
    LinkControlClientFrame, LinkControlRejectCode, LinkControlServerFrame, SERVICE_LINK_APP_ID,
};
pub use server::{LinkControlServer, LinkControlServerError};

#[cfg(test)]
pub(crate) static LINK_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
