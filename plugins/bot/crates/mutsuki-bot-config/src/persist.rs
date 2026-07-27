//! Persistence sink for ConfigProvider apply — Host owns atomic write.

use std::collections::HashMap;

use crate::error::ConfigError;
use crate::scope::ConfigContext;
use crate::value::ConfigValue;

/// Invoked before in-memory revision commit. Fail loud; do not advance revision on `Err`.
pub trait ConfigPersistSink: Send + Sync {
    fn persist(
        &self,
        context: &ConfigContext,
        value: &ConfigValue,
        secrets: &HashMap<String, String>,
    ) -> Result<(), ConfigError>;
}
