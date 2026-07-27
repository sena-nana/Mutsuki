//! Restricted, verifiable expression AST for visibility/enabled conditions.
//! No arbitrary JS, env, network, or function calls.

use serde::{Deserialize, Serialize};

use crate::budgets::ConfigBudgets;
use crate::error::ConfigError;
use crate::value::{ConfigKey, ConfigValue};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ConfigExpr {
    Field {
        key: ConfigKey,
    },
    Literal {
        value: ConfigValue,
    },
    Eq {
        left: Box<ConfigExpr>,
        right: Box<ConfigExpr>,
    },
    Ne {
        left: Box<ConfigExpr>,
        right: Box<ConfigExpr>,
    },
    And {
        items: Vec<ConfigExpr>,
    },
    Or {
        items: Vec<ConfigExpr>,
    },
    Not {
        expr: Box<ConfigExpr>,
    },
    IsSet {
        key: ConfigKey,
    },
}

impl ConfigExpr {
    pub fn validate_budgets(&self, budgets: &ConfigBudgets) -> Result<(), ConfigError> {
        let mut nodes = 0usize;
        self.walk(0, budgets, &mut nodes)
    }

    fn walk(
        &self,
        depth: usize,
        budgets: &ConfigBudgets,
        nodes: &mut usize,
    ) -> Result<(), ConfigError> {
        if depth > budgets.max_expr_depth {
            return Err(ConfigError::BudgetExceeded {
                reason: format!("expr depth > {}", budgets.max_expr_depth),
            });
        }
        *nodes += 1;
        if *nodes > budgets.max_expr_nodes {
            return Err(ConfigError::BudgetExceeded {
                reason: format!("expr nodes > {}", budgets.max_expr_nodes),
            });
        }
        match self {
            Self::Field { .. } | Self::Literal { .. } | Self::IsSet { .. } => Ok(()),
            Self::Eq { left, right } | Self::Ne { left, right } => {
                left.walk(depth + 1, budgets, nodes)?;
                right.walk(depth + 1, budgets, nodes)
            }
            Self::And { items } | Self::Or { items } => {
                for item in items {
                    item.walk(depth + 1, budgets, nodes)?;
                }
                Ok(())
            }
            Self::Not { expr } => expr.walk(depth + 1, budgets, nodes),
        }
    }

    /// Evaluate against an object-shaped draft value. Field keys are root-level.
    pub fn eval(&self, root: &ConfigValue) -> Result<bool, ConfigError> {
        match self {
            Self::Field { key } => Ok(truthy(lookup(root, key.as_str())?)),
            Self::Literal { value } => Ok(truthy(value)),
            Self::Eq { left, right } => Ok(eval_value(left, root)? == eval_value(right, root)?),
            Self::Ne { left, right } => Ok(eval_value(left, root)? != eval_value(right, root)?),
            Self::And { items } => {
                for item in items {
                    if !item.eval(root)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Self::Or { items } => {
                for item in items {
                    if item.eval(root)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Self::Not { expr } => Ok(!expr.eval(root)?),
            Self::IsSet { key } => {
                let value = lookup(root, key.as_str())?;
                Ok(!matches!(value, ConfigValue::Null))
            }
        }
    }

    /// Parse a tiny subset used by derive `visible_if = "field == true"`.
    pub fn parse_simple(input: &str) -> Result<Self, ConfigError> {
        let trimmed = input.trim();
        if let Some((left, right)) = trimmed.split_once("==") {
            return Ok(Self::Eq {
                left: Box::new(parse_atom(left.trim())?),
                right: Box::new(parse_atom(right.trim())?),
            });
        }
        if let Some((left, right)) = trimmed.split_once("!=") {
            return Ok(Self::Ne {
                left: Box::new(parse_atom(left.trim())?),
                right: Box::new(parse_atom(right.trim())?),
            });
        }
        Ok(parse_atom(trimmed)?)
    }
}

fn parse_atom(input: &str) -> Result<ConfigExpr, ConfigError> {
    if input == "true" {
        return Ok(ConfigExpr::Literal {
            value: ConfigValue::Bool(true),
        });
    }
    if input == "false" {
        return Ok(ConfigExpr::Literal {
            value: ConfigValue::Bool(false),
        });
    }
    if let Ok(n) = input.parse::<i64>() {
        return Ok(ConfigExpr::Literal {
            value: ConfigValue::Integer(n),
        });
    }
    if (input.starts_with('"') && input.ends_with('"'))
        || (input.starts_with('\'') && input.ends_with('\''))
    {
        let inner = &input[1..input.len() - 1];
        return Ok(ConfigExpr::Literal {
            value: ConfigValue::String(inner.to_string()),
        });
    }
    if input.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Ok(ConfigExpr::Field {
            key: ConfigKey::new(input),
        });
    }
    Err(ConfigError::ApplyRejected {
        reason: format!("unsupported expression atom: {input}"),
    })
}

fn lookup<'a>(root: &'a ConfigValue, key: &str) -> Result<&'a ConfigValue, ConfigError> {
    root.as_object()
        .and_then(|map| map.get(key))
        .ok_or_else(|| ConfigError::ApplyRejected {
            reason: format!("unknown field in expression: {key}"),
        })
}

fn eval_value(expr: &ConfigExpr, root: &ConfigValue) -> Result<ConfigValue, ConfigError> {
    match expr {
        ConfigExpr::Field { key } => Ok(lookup(root, key.as_str())?.clone()),
        ConfigExpr::Literal { value } => Ok(value.clone()),
        other => Ok(ConfigValue::Bool(other.eval(root)?)),
    }
}

fn truthy(value: &ConfigValue) -> bool {
    match value {
        ConfigValue::Null => false,
        ConfigValue::Bool(v) => *v,
        ConfigValue::Integer(v) => *v != 0,
        ConfigValue::Float(v) => *v != 0.0,
        ConfigValue::String(v) => !v.is_empty(),
        ConfigValue::Secret(_) => true,
        ConfigValue::Array(v) => !v.is_empty(),
        ConfigValue::Object(v) => !v.is_empty(),
    }
}
