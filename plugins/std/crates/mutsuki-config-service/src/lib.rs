//! Host-neutral configuration coordination.
//!
//! Repositories own durable documents. Providers own validation and reversible
//! domain activation. `ConfigService` is the only component that orders both.
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::assigning_clones,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::doc_markdown,
    clippy::match_same_arms,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::semicolon_if_nothing_returned,
    clippy::too_many_lines,
    clippy::unused_async
)]

mod lifecycle;
mod memory;
mod metrics;
mod migrate;
mod persist;
mod provider;
mod registry;
mod repository;
mod service;
mod validate;
mod watch;

pub use mutsuki_protocol_config::*;

pub use lifecycle::ConfigLifecycle;
pub use memory::MemoryConfigProvider;
pub use metrics::{ConfigMetrics, ConfigMetricsSnapshot};
pub use migrate::{MigrationPlan, MigrationStep, migrate, require_migration};
pub use persist::{ConfigPersistSink, ConfigPersistTransaction, ConfigSecretMutation};
pub use provider::{
    ConfigActivation, ConfigProvider, ConfigRegistration, PreparedConfigActivation,
};
pub use registry::{ConfigProviderRegistration, ConfigProviderRegistry, ProviderEntry};
pub use repository::{ConfigRepository, InMemoryConfigRepository, PreparedConfigWrite};
pub use service::ConfigService;
pub use validate::{validate_structure, validate_structure_with_budgets};
pub use watch::{ConfigWatchSubscription, RevisionChangedEvent, RevisionChangedListener};

pub use mutsuki_config_derive::MutsukiConfig;
