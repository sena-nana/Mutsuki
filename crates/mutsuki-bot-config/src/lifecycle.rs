//! Apply-time lifecycle hooks (plugin reload / restart) executed by ConfigService.

use crate::error::ConfigError;
use crate::provider::ConfigAction;
use crate::schema::RestartPolicy;

/// Executes restart/reload side effects after a successful durable apply.
///
/// Providers only report `restart_policy` and pending actions. Real plugin_reload
/// must go through an implementation of this trait (typically ServiceHost control).
pub trait ConfigLifecycle: Send + Sync {
    /// Consume pending lifecycle actions that this hook can fulfill.
    ///
    /// Returns actions that were actually completed (moved out of pending).
    fn execute(
        &self,
        provider_id: &str,
        policy: RestartPolicy,
        pending: &[ConfigAction],
    ) -> Result<Vec<ConfigAction>, ConfigError>;
}

/// No-op lifecycle — pending reload/restart actions remain pending.
pub struct NoopConfigLifecycle;

impl ConfigLifecycle for NoopConfigLifecycle {
    fn execute(
        &self,
        _provider_id: &str,
        _policy: RestartPolicy,
        _pending: &[ConfigAction],
    ) -> Result<Vec<ConfigAction>, ConfigError> {
        Ok(Vec::new())
    }
}
