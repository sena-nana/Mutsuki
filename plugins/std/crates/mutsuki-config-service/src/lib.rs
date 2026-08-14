//! Host-neutral configuration coordination.
//!
//! Repositories own durable documents. Providers own validation and reversible
//! domain activation. `ConfigService` is the only component that orders both.

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
