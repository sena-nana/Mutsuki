//! Domain-neutral configuration scope and context.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{ConfigBudgets, ConfigError};

fn validate_id(name: &str, value: &str, budgets: &ConfigBudgets) -> Result<(), ConfigError> {
    if value.trim().is_empty() {
        return Err(ConfigError::ScopeUnsupported {
            reason: format!("{name} must be non-empty"),
        });
    }
    if value.len() > budgets.max_id_bytes || value.chars().any(char::is_control) {
        return Err(ConfigError::ScopeUnsupported {
            reason: format!("{name} is not a valid bounded identifier"),
        });
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConfigProviderId(pub String);

impl ConfigProviderId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Qualified, owner-defined scope. The framework does not enumerate product domains.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConfigScope(pub String);

impl ConfigScope {
    pub const GLOBAL: &'static str = "mutsuki.global";
    pub const PLUGIN_INSTANCE: &'static str = "mutsuki.plugin.instance";

    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn global() -> Self {
        Self::new(Self::GLOBAL)
    }

    #[must_use]
    pub fn plugin_instance() -> Self {
        Self::new(Self::PLUGIN_INSTANCE)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Context qualifiers are interpreted only by the provider that owns the scope.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConfigContext {
    pub scope: ConfigScope,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub qualifiers: BTreeMap<String, String>,
}

impl ConfigContext {
    #[must_use]
    pub fn global() -> Self {
        Self {
            scope: ConfigScope::global(),
            qualifiers: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn plugin_instance(plugin_instance_id: impl Into<String>) -> Self {
        Self {
            scope: ConfigScope::plugin_instance(),
            qualifiers: BTreeMap::from([("plugin_instance_id".into(), plugin_instance_id.into())]),
        }
    }

    #[must_use]
    pub fn qualified(scope: impl Into<String>, qualifiers: BTreeMap<String, String>) -> Self {
        Self {
            scope: ConfigScope::new(scope),
            qualifiers,
        }
    }

    #[must_use]
    pub fn storage_key(&self) -> String {
        let mut parts = vec![self.scope.0.clone()];
        parts.extend(
            self.qualifiers
                .iter()
                .map(|(key, value)| format!("{key}={value}")),
        );
        parts.join("|")
    }

    pub fn validate(&self, budgets: &ConfigBudgets) -> Result<(), ConfigError> {
        validate_id("scope", self.scope.as_str(), budgets)?;
        for (key, value) in &self.qualifiers {
            validate_id("context qualifier key", key, budgets)?;
            validate_id("context qualifier value", value, budgets)?;
        }
        Ok(())
    }
}
