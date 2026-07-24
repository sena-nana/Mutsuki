//! Schema-first Bot/plugin configuration protocol.
//!
//! Rust config types derive a stable, versioned, UI-agnostic Config Schema.
//! CLI / Tauri / Web renderers consume the same descriptor via ConfigProvider.

mod budgets;
mod error;
mod expr;
mod lifecycle;
mod memory;
mod metrics;
mod migrate;
mod persist;
mod provider;
mod registry;
mod schema;
mod scope;
mod secret;
mod service;
mod validate;
mod value;
mod watch;

pub use budgets::{ConfigBudgets, DEFAULT_BUDGETS};
pub use error::{
    ConfigError, FieldDiff, LocalizedText, ValidationCode, ValidationIssue, ValidationResult,
    ValidationSeverity, capability,
};
pub use expr::ConfigExpr;
pub use lifecycle::ConfigLifecycle;
pub use memory::MemoryConfigProvider;
pub use metrics::{ConfigMetrics, ConfigMetricsSnapshot};
pub use migrate::{MigrationPlan, MigrationStep, migrate, require_migration};
pub use persist::ConfigPersistSink;
pub use provider::{
    ConfigAction, ConfigApplyRequest, ConfigApplyResult, ConfigProvider, ConfigRegistration,
    ConfigRevision, ConfigSnapshot, ConfigSource,
};
pub use registry::{ConfigProviderRegistry, ProviderEntry};
pub use schema::{
    ConfigApplyMode, ConfigConstraints, ConfigDescriptor, ConfigGroup, ConfigMutability,
    ConfigNode, ConfigPresentation, ConfigValueType, EnumOption, MapKeyStrategy,
    MutsukiConfigSchema, RestartPolicy,
};
pub use scope::{
    AccountId, BotId, ChannelId, ConfigContext, ConfigProviderId, ConfigScope, GuildId, HostId,
    PluginInstanceId,
};
pub use secret::{SecretState, SecretUpdate, SecretValue};
pub use service::ConfigService;
pub use validate::{validate_structure, validate_structure_with_budgets};
pub use value::{ConfigKey, ConfigPath, ConfigValue};
pub use watch::{RevisionChangedEvent, RevisionChangedListener};

/// Re-export derive macro.
pub use mutsuki_bot_config_derive::MutsukiConfig;
