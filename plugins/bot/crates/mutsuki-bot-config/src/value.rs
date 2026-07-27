//! Typed configuration values. Secrets are never plain strings in snapshots.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::secret::SecretState;

/// Stable field key within a schema.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConfigKey(pub String);

impl ConfigKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ConfigKey {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for ConfigKey {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// Dot-separated path into a nested config value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConfigPath(pub Vec<String>);

impl ConfigPath {
    pub fn root() -> Self {
        Self(Vec::new())
    }

    pub fn join(&self, segment: impl Into<String>) -> Self {
        let mut parts = self.0.clone();
        parts.push(segment.into());
        Self(parts)
    }

    pub fn display(&self) -> String {
        self.0.join(".")
    }
}

/// Versioned typed config value model (not a free-form JSON blob as the sole API).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ConfigValue {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Secret(SecretState),
    Array(Vec<ConfigValue>),
    Object(BTreeMap<String, ConfigValue>),
}

impl ConfigValue {
    pub fn object_empty() -> Self {
        Self::Object(BTreeMap::new())
    }

    pub fn as_object(&self) -> Option<&BTreeMap<String, ConfigValue>> {
        match self {
            Self::Object(map) => Some(map),
            _ => None,
        }
    }

    pub fn as_object_mut(&mut self) -> Option<&mut BTreeMap<String, ConfigValue>> {
        match self {
            Self::Object(map) => Some(map),
            _ => None,
        }
    }

    pub fn get_path(&self, path: &ConfigPath) -> Option<&ConfigValue> {
        let mut current = self;
        for segment in &path.0 {
            current = current.as_object()?.get(segment)?;
        }
        Some(current)
    }

    /// Convert to JSON for wire transport. Secrets stay redacted/sentinel, never plaintext.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Null => serde_json::Value::Null,
            Self::Bool(v) => serde_json::Value::Bool(*v),
            Self::Integer(v) => serde_json::json!(*v),
            Self::Float(v) => serde_json::json!(*v),
            Self::String(v) => serde_json::Value::String(v.clone()),
            Self::Secret(state) => serde_json::to_value(state).unwrap_or(serde_json::Value::Null),
            Self::Array(items) => {
                serde_json::Value::Array(items.iter().map(Self::to_json).collect())
            }
            Self::Object(map) => {
                let mut out = serde_json::Map::new();
                for (k, v) in map {
                    out.insert(k.clone(), v.to_json());
                }
                serde_json::Value::Object(out)
            }
        }
    }

    pub fn from_json(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(v) => Self::Bool(*v),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Self::Integer(i)
                } else if let Some(f) = n.as_f64() {
                    Self::Float(f)
                } else {
                    Self::Null
                }
            }
            serde_json::Value::String(s) => Self::String(s.clone()),
            serde_json::Value::Array(items) => {
                Self::Array(items.iter().map(Self::from_json).collect())
            }
            serde_json::Value::Object(map) => {
                // Secret wire shape: { "state": "configured"|"absent"|"unavailable"|"keep"|"set"|"clear", ... }
                if let Some(state) = map.get("state").and_then(|v| v.as_str()) {
                    if matches!(
                        state,
                        "configured" | "absent" | "unavailable" | "keep" | "set" | "clear"
                    ) {
                        if let Ok(secret) = serde_json::from_value::<SecretState>(value.clone()) {
                            return Self::Secret(secret);
                        }
                    }
                }
                let mut out = BTreeMap::new();
                for (k, v) in map {
                    out.insert(k.clone(), Self::from_json(v));
                }
                Self::Object(out)
            }
        }
    }
}
