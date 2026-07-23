//! Versioned ConfigDescriptor — UI-agnostic schema.

use serde::{Deserialize, Serialize};

use crate::budgets::{ConfigBudgets, DEFAULT_BUDGETS};
use crate::error::{ConfigError, LocalizedText};
use crate::expr::ConfigExpr;
use crate::scope::{ConfigProviderId, ConfigScope};
use crate::value::{ConfigKey, ConfigValue};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigDescriptor {
    pub provider_id: ConfigProviderId,
    pub schema_version: u32,
    pub value_version: u32,
    pub title: LocalizedText,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<LocalizedText>,
    pub scopes: Vec<ConfigScope>,
    pub root: ConfigNode,
    #[serde(default)]
    pub groups: Vec<ConfigGroup>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigGroup {
    pub id: String,
    pub title: LocalizedText,
    #[serde(default)]
    pub order: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigNode {
    pub key: ConfigKey,
    pub value_type: ConfigValueType,
    pub title: LocalizedText,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<LocalizedText>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<ConfigValue>,
    #[serde(default)]
    pub constraints: ConfigConstraints,
    #[serde(default)]
    pub presentation: ConfigPresentation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<ConfigExpr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_if: Option<ConfigExpr>,
    #[serde(default)]
    pub mutability: ConfigMutability,
    #[serde(default)]
    pub restart_policy: RestartPolicy,
    #[serde(default)]
    pub children: Vec<ConfigNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConfigValueType {
    Null,
    Bool,
    Integer,
    Float,
    String {
        #[serde(default)]
        multiline: bool,
    },
    Enum {
        options: Vec<EnumOption>,
        #[serde(default)]
        multi: bool,
    },
    Object,
    Array {
        item: Box<ConfigValueType>,
    },
    Map {
        key_strategy: MapKeyStrategy,
        value: Box<ConfigValueType>,
    },
    Secret,
    FileRef,
    DirectoryRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumOption {
    pub value: String,
    pub label: LocalizedText,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapKeyStrategy {
    FreeString,
    Pattern { pattern: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ConfigConstraints {
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_length: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_items: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ConfigPresentation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default)]
    pub order: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help_link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default)]
    pub secret: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConfigMutability {
    #[default]
    ReadWrite,
    ReadOnly,
    Computed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    #[default]
    None,
    Reconfigure,
    PluginReload,
    BotRestart,
    HostRestart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigApplyMode {
    HotReload,
    PersistOnly,
    RequireRestart,
}

impl ConfigDescriptor {
    pub fn validate_budgets(&self, budgets: &ConfigBudgets) -> Result<(), ConfigError> {
        let mut nodes = 0usize;
        check_node(&self.root, 0, budgets, &mut nodes)?;
        for group in &self.groups {
            if group.id.len() > budgets.max_id_bytes {
                return Err(ConfigError::BudgetExceeded {
                    reason: format!("group id exceeds max_id_bytes={}", budgets.max_id_bytes),
                });
            }
        }
        Ok(())
    }

    pub fn validate_default_budgets(&self) -> Result<(), ConfigError> {
        self.validate_budgets(&DEFAULT_BUDGETS)
    }

    pub fn supports_scope(&self, scope: ConfigScope) -> bool {
        self.scopes.contains(&scope)
    }

    pub fn cached_static(descriptor: ConfigDescriptor) -> &'static ConfigDescriptor {
        // Leak intentionally for process-lifetime schema cache (derive prefer static).
        Box::leak(Box::new(descriptor))
    }
}

fn check_node(
    node: &ConfigNode,
    depth: usize,
    budgets: &ConfigBudgets,
    nodes: &mut usize,
) -> Result<(), ConfigError> {
    if depth > budgets.max_schema_depth {
        return Err(ConfigError::BudgetExceeded {
            reason: format!("schema depth > {}", budgets.max_schema_depth),
        });
    }
    *nodes += 1;
    if *nodes > budgets.max_schema_nodes {
        return Err(ConfigError::BudgetExceeded {
            reason: format!("schema nodes > {}", budgets.max_schema_nodes),
        });
    }
    if let Some(expr) = &node.visibility {
        expr.validate_budgets(budgets)?;
    }
    if let Some(expr) = &node.enabled_if {
        expr.validate_budgets(budgets)?;
    }
    for child in &node.children {
        check_node(child, depth + 1, budgets, nodes)?;
    }
    Ok(())
}

/// Trait implemented by `#[derive(MutsukiConfig)]`.
pub trait MutsukiConfigSchema {
    fn schema() -> ConfigDescriptor;
}
