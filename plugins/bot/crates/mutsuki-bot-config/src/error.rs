//! Structured configuration errors — no string-prefix protocol.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::value::{ConfigPath, ConfigValue};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationCode {
    Required,
    TypeMismatch,
    OutOfRange,
    PatternMismatch,
    LengthInvalid,
    UnknownField,
    ConstraintFailed,
    BusinessRule,
    PermissionDenied,
    ExpressionInvalid,
    BudgetExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalizedText {
    pub default: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zh_cn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub en: Option<String>,
}

impl LocalizedText {
    pub fn new(default: impl Into<String>) -> Self {
        Self {
            default: default.into(),
            zh_cn: None,
            en: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub path: ConfigPath,
    pub code: ValidationCode,
    pub severity: ValidationSeverity,
    pub message: LocalizedText,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationResult {
    pub ok: bool,
    pub issues: Vec<ValidationIssue>,
}

impl ValidationResult {
    pub fn success() -> Self {
        Self {
            ok: true,
            issues: Vec::new(),
        }
    }

    pub fn from_issues(issues: Vec<ValidationIssue>) -> Self {
        let ok = !issues
            .iter()
            .any(|issue| matches!(issue.severity, ValidationSeverity::Error));
        Self { ok, issues }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConfigError {
    #[error("provider unavailable")]
    ProviderUnavailable,
    #[error("scope unsupported: {reason}")]
    ScopeUnsupported { reason: String },
    #[error("schema incompatible: expected {expected}, got {actual}")]
    SchemaIncompatible { expected: u32, actual: u32 },
    #[error("value migration required: from {from_version} to {to_version}")]
    ValueMigrationRequired { from_version: u32, to_version: u32 },
    #[error("validation failed")]
    ValidationFailed { result: ValidationResult },
    #[error("revision conflict: expected {expected}, current {current}")]
    RevisionConflict {
        expected: u64,
        current: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff: Option<Vec<FieldDiff>>,
    },
    #[error("permission denied: {capability}")]
    PermissionDenied { capability: String },
    #[error("apply rejected: {reason}")]
    ApplyRejected { reason: String },
    #[error("persistence failed: {reason}")]
    PersistenceFailed { reason: String },
    #[error("reload failed: {reason}")]
    ReloadFailed { reason: String },
    #[error("secret unavailable")]
    SecretUnavailable,
    #[error("budget exceeded: {reason}")]
    BudgetExceeded { reason: String },
    #[error("cancelled")]
    Cancelled,
    #[error("timeout")]
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldDiff {
    pub path: ConfigPath,
    pub before: Option<ConfigValue>,
    pub after: Option<ConfigValue>,
}

/// Capability constants for config operations.
pub mod capability {
    pub const SCHEMA_READ: &str = "config.schema.read";
    pub const VALUE_READ: &str = "config.value.read";
    pub const VALUE_WRITE: &str = "config.value.write";
    pub const SECRET_WRITE: &str = "config.secret.write";
    pub const APPLY: &str = "config.apply";
    pub const RELOAD: &str = "config.reload";
}
