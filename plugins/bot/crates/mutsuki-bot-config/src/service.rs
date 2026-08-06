//! Facade service used by CLI/Web/Tauri with capability checks and metrics.

use std::sync::Arc;
use std::time::Instant;

use crate::error::{ConfigError, capability};
use crate::lifecycle::ConfigLifecycle;
use crate::metrics::ConfigMetricsSnapshot;
use crate::provider::{ConfigApplyRequest, ConfigApplyResult, ConfigSnapshot};
use crate::registry::ConfigProviderRegistry;
use crate::schema::ConfigDescriptor;
use crate::scope::{ConfigContext, ConfigProviderId};
use crate::value::ConfigValue;
use crate::watch::{ConfigWatchHub, RevisionChangedEvent, RevisionChangedListener};

#[derive(Clone)]
pub struct ConfigService {
    registry: Arc<ConfigProviderRegistry>,
    lifecycle: Option<Arc<dyn ConfigLifecycle>>,
    watch: Arc<ConfigWatchHub>,
}

impl ConfigService {
    pub fn new(registry: Arc<ConfigProviderRegistry>) -> Self {
        Self {
            registry,
            lifecycle: None,
            watch: Arc::new(ConfigWatchHub::default()),
        }
    }

    pub fn with_lifecycle(mut self, lifecycle: Arc<dyn ConfigLifecycle>) -> Self {
        self.lifecycle = Some(lifecycle);
        self
    }

    pub fn registry(&self) -> &ConfigProviderRegistry {
        &self.registry
    }

    pub fn subscribe_revision_changed(&self, listener: RevisionChangedListener) {
        self.watch.subscribe(listener);
    }

    pub fn list_providers(&self, caps: &[String]) -> Result<Vec<ConfigProviderId>, ConfigError> {
        require_cap(caps, capability::SCHEMA_READ)?;
        Ok(self.registry.list())
    }

    pub fn get_schema(
        &self,
        provider_id: &str,
        caps: &[String],
    ) -> Result<ConfigDescriptor, ConfigError> {
        require_cap(caps, capability::SCHEMA_READ)?;
        Ok((*self.registry.schema(provider_id)?).clone())
    }

    pub async fn read(
        &self,
        provider_id: &str,
        context: ConfigContext,
        caps: &[String],
    ) -> Result<ConfigSnapshot, ConfigError> {
        require_cap(caps, capability::VALUE_READ)?;
        let entry = self.registry.ensure_scope(provider_id, context.scope)?;
        let started = Instant::now();
        let result = entry.provider.read(context).await;
        self.registry
            .metrics()
            .observe_read(started.elapsed().as_millis() as u64);
        result
    }

    pub async fn validate(
        &self,
        provider_id: &str,
        candidate: ConfigValue,
        context: ConfigContext,
        caps: &[String],
    ) -> Result<crate::error::ValidationResult, ConfigError> {
        // validate must not become a permission oracle for secrets/values.
        require_cap(caps, capability::VALUE_WRITE)?;
        let entry = self.registry.ensure_scope(provider_id, context.scope)?;
        let started = Instant::now();
        let result = entry.provider.validate(candidate, context).await;
        self.registry
            .metrics()
            .observe_validate(started.elapsed().as_millis() as u64);
        result
    }

    pub async fn apply(
        &self,
        provider_id: &str,
        request: ConfigApplyRequest,
        context: ConfigContext,
        caps: &[String],
    ) -> Result<ConfigApplyResult, ConfigError> {
        require_cap(caps, capability::APPLY)?;
        require_cap(caps, capability::VALUE_WRITE)?;
        if candidate_writes_secret(&request.candidate) {
            require_cap(caps, capability::SECRET_WRITE)?;
        }
        let dry_run = request.dry_run;
        let entry = self.registry.ensure_scope(provider_id, context.scope)?;
        let started = Instant::now();
        let prepared = entry.provider.prepare(request, context.clone()).await;
        match &prepared {
            Err(ConfigError::RevisionConflict { .. }) => {
                self.registry.metrics().inc_revision_conflict();
                self.registry.metrics().inc_apply_failed();
            }
            Err(_) => self.registry.metrics().inc_apply_failed(),
            Ok(change) if !change.result().pending_actions.is_empty() => {
                self.registry.metrics().inc_reload_required();
            }
            Ok(_) => {}
        }
        let mut prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.registry
                    .metrics()
                    .observe_apply(started.elapsed().as_millis() as u64);
                return Err(error);
            }
        };
        if !dry_run && prepared.result().applied {
            if let Err(error) = prepared.activate() {
                let rollback = prepared.rollback();
                self.registry.metrics().inc_apply_failed();
                self.registry
                    .metrics()
                    .observe_apply(started.elapsed().as_millis() as u64);
                return Err(transaction_error(error, [rollback]));
            }
            let policy = prepared.result().restart_policy;
            let pending = prepared.result().pending_actions.clone();
            let completed = if let Some(lifecycle) = &self.lifecycle {
                match lifecycle.execute(provider_id, policy, &pending) {
                    Ok(completed) => completed,
                    Err(error) => {
                        let rollback = prepared.rollback();
                        let lifecycle_rollback = lifecycle.rollback(provider_id, policy, &[]);
                        self.registry.metrics().inc_apply_failed();
                        self.registry
                            .metrics()
                            .observe_apply(started.elapsed().as_millis() as u64);
                        return Err(transaction_error(error, [rollback, lifecycle_rollback]));
                    }
                }
            } else {
                Vec::new()
            };
            if let Err(error) = prepared.commit() {
                let rollback = prepared.rollback();
                let lifecycle_rollback = self.lifecycle.as_ref().map_or(Ok(()), |lifecycle| {
                    lifecycle.rollback(provider_id, policy, &completed)
                });
                self.registry.metrics().inc_apply_failed();
                self.registry
                    .metrics()
                    .observe_apply(started.elapsed().as_millis() as u64);
                return Err(transaction_error(error, [rollback, lifecycle_rollback]));
            }
            let result = prepared.result_mut();
            for action in completed {
                result.pending_actions.retain(|pending| pending != &action);
                if !result.actions.contains(&action) {
                    result.actions.push(action);
                }
            }
            self.watch.notify(RevisionChangedEvent {
                provider_id: ConfigProviderId::new(provider_id),
                revision: prepared.result().revision,
                context,
            });
        }
        self.registry
            .metrics()
            .observe_apply(started.elapsed().as_millis() as u64);
        Ok(prepared.into_result())
    }

    pub fn metrics_snapshot(&self) -> ConfigMetricsSnapshot {
        self.registry.metrics().snapshot()
    }
}

fn transaction_error<const N: usize>(
    cause: ConfigError,
    rollbacks: [Result<(), ConfigError>; N],
) -> ConfigError {
    let rollback = rollbacks
        .into_iter()
        .filter_map(Result::err)
        .map(|error| error.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    if rollback.is_empty() {
        cause
    } else {
        ConfigError::RollbackFailed {
            cause: cause.to_string(),
            rollback,
        }
    }
}

fn require_cap(caps: &[String], needed: &str) -> Result<(), ConfigError> {
    if caps.iter().any(|c| c == "*" || c == needed) {
        Ok(())
    } else {
        Err(ConfigError::PermissionDenied {
            capability: needed.to_string(),
        })
    }
}

fn candidate_writes_secret(value: &ConfigValue) -> bool {
    match value {
        ConfigValue::Secret(state) => matches!(
            state,
            crate::secret::SecretState::Set { .. } | crate::secret::SecretState::Clear
        ),
        ConfigValue::Object(map) => map.values().any(candidate_writes_secret),
        ConfigValue::Array(items) => items.iter().any(candidate_writes_secret),
        _ => false,
    }
}
