//! Apply-time lifecycle hooks executed by ConfigService after durable apply.

use crate::{ConfigAction, ConfigError, ConfigValue, RestartPolicy};

/// Real plugin_reload / restart side effects. Providers only report policy + pending.
pub trait ConfigLifecycle: Send + Sync {
    fn execute(
        &self,
        provider_id: &str,
        current: &ConfigValue,
        candidate: &ConfigValue,
        policy: RestartPolicy,
        pending: &[ConfigAction],
    ) -> Result<Vec<ConfigAction>, ConfigError>;

    fn rollback(
        &self,
        _provider_id: &str,
        _current: &ConfigValue,
        _policy: RestartPolicy,
        _completed: &[ConfigAction],
    ) -> Result<(), ConfigError> {
        Ok(())
    }
}
