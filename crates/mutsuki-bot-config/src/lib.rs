//! Schema-first Bot/plugin configuration protocol.
//!
//! Rust config types derive a stable, versioned, UI-agnostic Config Schema.
//! CLI / Tauri / Web renderers consume the same descriptor via ConfigProvider.

mod budgets;
mod error;
mod expr;
mod json_schema;
mod memory;
mod metrics;
mod migrate;
mod provider;
mod registry;
mod schema;
mod scope;
mod secret;
mod service;
mod validate;
mod value;

pub use budgets::{ConfigBudgets, DEFAULT_BUDGETS};
pub use error::{
    ConfigError, FieldDiff, LocalizedText, ValidationCode, ValidationIssue, ValidationResult,
    ValidationSeverity, capability,
};
pub use expr::ConfigExpr;
pub use json_schema::to_json_schema;
pub use memory::{MemoryConfigProvider, SharedMemoryProvider};
pub use metrics::{ConfigMetrics, ConfigMetricsSnapshot};
pub use migrate::{MigrationDryRun, MigrationPlan, MigrationStep, migrate, require_migration};
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

/// Re-export derive macro.
pub use mutsuki_bot_config_derive::MutsukiConfig;
