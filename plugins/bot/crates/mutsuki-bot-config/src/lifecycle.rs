//! Apply-time lifecycle hooks executed by ConfigService after durable apply.

use crate::error::ConfigError;
use crate::provider::ConfigAction;
use crate::schema::RestartPolicy;

/// Real plugin_reload / restart side effects. Providers only report policy + pending.
pub trait ConfigLifecycle: Send + Sync {
    fn execute(
        &self,
        provider_id: &str,
        policy: RestartPolicy,
        pending: &[ConfigAction],
    ) -> Result<Vec<ConfigAction>, ConfigError>;
}
