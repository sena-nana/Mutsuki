//! Host-neutral configuration contracts.
//!
//! This crate owns the wire-visible schema, value, validation, revision and
//! compare-and-set documents. Storage and activation live in separate crates.

pub mod budgets;
pub mod document;
pub mod error;
pub mod expr;
pub mod schema;
pub mod scope;
pub mod secret;
pub mod value;

pub use budgets::{ConfigBudgets, DEFAULT_BUDGETS};
pub use document::{
    ConfigAction, ConfigApplyRequest, ConfigApplyResult, ConfigCompareAndSetRequest,
    ConfigDocumentKey, ConfigDocumentSnapshot, ConfigRevision, ConfigSnapshot, ConfigSource,
};
pub use error::{
    ConfigError, FieldDiff, LocalizedText, ValidationCode, ValidationIssue, ValidationResult,
    ValidationSeverity, capability,
};
pub use expr::ConfigExpr;
pub use schema::{
    ConfigApplyMode, ConfigConstraints, ConfigDescriptor, ConfigGroup, ConfigMutability,
    ConfigNode, ConfigPresentation, ConfigValueType, EnumOption, MapKeyStrategy,
    MutsukiConfigSchema, RestartPolicy,
};
pub use scope::{ConfigContext, ConfigProviderId, ConfigScope};
pub use secret::{SecretState, SecretUpdate, SecretValue};
pub use value::{ConfigKey, ConfigPath, ConfigValue};

use serde_json::{Value, json};

pub const VERSION: &str = "0.2.0";
pub const ABI_CODEC: &str = "serde-json";
pub const CONFIG_DESCRIBE: &str = "mutsuki.config.describe";
pub const PERMISSION_CHECK: &str = "mutsuki.permission.check";
pub const PROTOCOL_IDS: &[&str] = &[CONFIG_DESCRIBE, PERMISSION_CHECK];

pub fn input_schema(protocol_id: &str) -> Option<Value> {
    PROTOCOL_IDS
        .contains(&protocol_id)
        .then(|| json!({"type": "object"}))
}

pub fn output_schema(protocol_id: &str) -> Option<Value> {
    input_schema(protocol_id)
}

pub fn error_schema(protocol_id: &str) -> Option<Value> {
    input_schema(protocol_id)
}
