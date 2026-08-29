//! WebExtension host registry.
//!
//! Plugin authors depend on `mutsuki-web-extension-api`. This crate owns Host
//! load/budget/reactor dispatch.
#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::must_use_candidate
)]

mod registry;

pub use mutsuki_web_extension_api::*;
pub use registry::{
    ExtensionLoadReport, ExtensionRecord, ExtensionRegistry, WebExtensionContext, WebServiceContext,
};
