//! Persistence sink for ConfigProvider apply — Host owns atomic write.

use std::collections::HashMap;

use crate::error::ConfigError;
use crate::scope::ConfigContext;
use crate::value::ConfigValue;

/// Prepared owner persistence. `activate` may publish a candidate runtime snapshot, `commit`
/// atomically persists it, and `rollback` restores the previous runtime/persistent state.
pub trait PreparedConfigPersist: Send {
    fn activate(&mut self) -> Result<(), ConfigError> {
        Ok(())
    }

    fn commit(&mut self) -> Result<(), ConfigError>;

    fn rollback(&mut self) -> Result<(), ConfigError> {
        Ok(())
    }
}

pub trait ConfigPersistSink: Send + Sync {
    fn prepare(
        &self,
        context: &ConfigContext,
        value: &ConfigValue,
        secrets: &HashMap<String, String>,
    ) -> Result<Box<dyn PreparedConfigPersist>, ConfigError>;
}
