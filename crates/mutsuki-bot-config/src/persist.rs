//! Persistence sink for ConfigProvider apply — Host stores own atomic write.

use std::collections::HashMap;

use crate::error::ConfigError;
use crate::scope::ConfigContext;
use crate::value::ConfigValue;

/// Host/product-backed sink invoked before in-memory revision commit.
///
/// Implementations must fail loud (`PersistenceFailed`) and leave the previous
/// durable state untouched on error. Providers must not advance revision when
/// this returns `Err`.
pub trait ConfigPersistSink: Send + Sync {
    fn persist(
        &self,
        context: &ConfigContext,
        value: &ConfigValue,
        secrets: &HashMap<String, String>,
    ) -> Result<(), ConfigError>;
}
