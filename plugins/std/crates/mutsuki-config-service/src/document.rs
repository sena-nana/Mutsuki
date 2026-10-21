//! Helpers for owners that turn their config document back into a runtime config.
//!
//! An owner projects its runtime config into a document (`*_config_value`) and owns the
//! inverse as well; these helpers keep the field rules identical across owners. Every
//! failure is [`ConfigError::ApplyRejected`] with a console-facing reason.

use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::ConfigError;

pub type DocumentObject = Map<String, Value>;

/// The document as an object; anything else is rejected.
pub fn document_object(document: &Value) -> Result<&DocumentObject, ConfigError> {
    document
        .as_object()
        .ok_or_else(|| rejected("配置必须是对象"))
}

/// Owner config carried by a document: an explicit `runtime_config` must parse, otherwise
/// the currently applied config is the base. Malformed input is rejected; it never silently
/// falls back to the old or default config.
pub fn runtime_config_or_current<T>(
    document: &DocumentObject,
    current: Option<&Value>,
) -> Result<T, ConfigError>
where
    T: DeserializeOwned + Default,
{
    match document.get("runtime_config") {
        Some(value) => {
            T::deserialize(value).map_err(|error| rejected(format!("runtime_config 无效: {error}")))
        }
        None => current_config(current),
    }
}

/// The currently applied config; an absent or `null` config is the owner default, but an
/// unreadable stored config is an error rather than a reset.
pub fn current_config<T>(current: Option<&Value>) -> Result<T, ConfigError>
where
    T: DeserializeOwned + Default,
{
    match current {
        None | Some(Value::Null) => Ok(T::default()),
        Some(config) => T::deserialize(config)
            .map_err(|error| rejected(format!("当前插件配置无法读取: {error}"))),
    }
}

pub fn bool_field(document: &DocumentObject, field: &str) -> Result<bool, ConfigError> {
    document
        .get(field)
        .and_then(Value::as_bool)
        .ok_or_else(|| rejected(format!("`{field}` 必须是布尔值")))
}

pub fn optional_bool_field(document: &DocumentObject, field: &str) -> Option<bool> {
    document.get(field).and_then(Value::as_bool)
}

pub fn string_field(document: &DocumentObject, field: &str) -> Result<String, ConfigError> {
    document
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| rejected(format!("`{field}` 必须是字符串")))
}

/// The field as a string, or empty when it is absent or not a string.
pub fn optional_string_field(document: &DocumentObject, field: &str) -> String {
    document
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

pub fn unsigned_field<T>(document: &DocumentObject, field: &str) -> Result<T, ConfigError>
where
    T: TryFrom<u64>,
{
    document
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|value| T::try_from(value).ok())
        .ok_or_else(|| rejected(format!("`{field}` 超出允许范围")))
}

pub fn rejected(reason: impl Into<String>) -> ConfigError {
    ConfigError::ApplyRejected {
        reason: reason.into(),
    }
}
