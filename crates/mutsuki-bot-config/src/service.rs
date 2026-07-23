//! Facade service used by CLI/Web/Tauri with capability checks and metrics.

use std::sync::Arc;
use std::time::Instant;

use crate::error::{ConfigError, capability};
use crate::metrics::ConfigMetricsSnapshot;
use crate::provider::{ConfigApplyRequest, ConfigApplyResult, ConfigSnapshot};
use crate::registry::ConfigProviderRegistry;
use crate::schema::ConfigDescriptor;
use crate::scope::{ConfigContext, ConfigProviderId};
use crate::value::ConfigValue;

#[derive(Clone)]
pub struct ConfigService {
    registry: Arc<ConfigProviderRegistry>,
}

impl ConfigService {
    pub fn new(registry: Arc<ConfigProviderRegistry>) -> Self {
        Self { registry }
    }

    pub fn registry(&self) -> &ConfigProviderRegistry {
        &self.registry
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
        let entry = self.registry.ensure_scope(provider_id, context.scope)?;
        let started = Instant::now();
        let result = entry.provider.apply(request, context).await;
        self.registry
            .metrics()
            .observe_apply(started.elapsed().as_millis() as u64);
        match &result {
            Err(ConfigError::RevisionConflict { .. }) => {
                self.registry.metrics().inc_revision_conflict();
                self.registry.metrics().inc_apply_failed();
            }
            Err(_) => self.registry.metrics().inc_apply_failed(),
            Ok(ok) if !ok.pending_actions.is_empty() => {
                self.registry.metrics().inc_reload_required();
            }
            Ok(_) => {}
        }
        result
    }

    pub fn metrics_snapshot(&self) -> ConfigMetricsSnapshot {
        self.registry.metrics().snapshot()
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
