// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::assigning_clones,
    clippy::cast_possible_truncation,
    clippy::default_trait_access,
    clippy::doc_markdown,
    clippy::duration_suboptimal_units,
    clippy::format_push_string,
    clippy::if_not_else,
    clippy::ignored_unit_patterns,
    clippy::items_after_statements,
    clippy::manual_let_else,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::no_effect_underscore_binding,
    clippy::redundant_closure_for_method_calls,
    clippy::return_self_not_must_use,
    clippy::semicolon_if_nothing_returned,
    clippy::too_many_lines,
    clippy::unnecessary_literal_bound,
    clippy::unreadable_literal,
    clippy::used_underscore_binding
)]

mod app_delivery;
mod approval;
mod builder;
mod config;
#[cfg(test)]
mod echo;
mod error;
mod health;
mod host;
mod plugin_abi;
mod plugin_package;
mod plugin_path;
mod plugin_runner;

pub use app_delivery::{
    ActivationError, ActivationReceipt, AppCapabilityEndpoint, AppDeliveryError,
    AppDeliveryOptions, AppDeliveryService, AppDescriptor, AppId, AppIdentity, AppLinkSession,
    AppLinkTransport, CapabilityStatus, DeliveryDraft, DeliveryDraftStore, DeliveryPhase,
    EndpointDescriptor, HOST_PROTOCOL_VERSION, InMemoryAppLinkTransport, LinkLocalAppTransport,
    OperationHistoryPolicy, OperationHistoryStats, ProcessAppActivator, TauriAppActivator,
    desktop_receipt_retention,
};
pub use approval::{ApprovalBridge, PendingApproval};
pub use builder::MutsukiTauriHostBuilder;
pub use config::{HostMode, MutsukiTauriConfig, PathsConfig, PluginSelection, SecurityConfig};
pub use error::{HostError, HostResult};
pub use host::{MAX_RESOURCE_INVOKE_BYTES, MutsukiTauriHost};
pub use mutsuki_runtime_contracts::{CapabilityDescriptor, DeliveryReceipt};
pub use plugin_package::{PluginPackageRecord, StagedCompanionArtifact};

#[cfg(test)]
mod tests;
