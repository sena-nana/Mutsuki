//! Plugin configuration helpers.

use serde::Deserialize;
use serde_json::Value;

/// Configuration of a plugin that accepts no options; unknown fields are rejected.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyPluginConfig {}

/// Rejects any field in a configuration that must stay empty.
pub fn decode_empty(plugin_id: &str, config: &Value) -> Result<(), String> {
    serde_json::from_value::<EmptyPluginConfig>(config.clone())
        .map(|_| ())
        .map_err(|error| format!("invalid {plugin_id} configuration: {error}"))
}
