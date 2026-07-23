//! Provider registry with schema cache and metrics hooks.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::budgets::{ConfigBudgets, DEFAULT_BUDGETS};
use crate::error::ConfigError;
use crate::metrics::ConfigMetrics;
use crate::provider::ConfigProvider;
use crate::schema::ConfigDescriptor;
use crate::scope::{ConfigProviderId, ConfigScope};

#[derive(Clone)]
pub struct ProviderEntry {
    pub provider: Arc<dyn ConfigProvider>,
    pub cached_schema: Arc<ConfigDescriptor>,
}

pub struct ConfigProviderRegistry {
    budgets: ConfigBudgets,
    providers: RwLock<HashMap<String, ProviderEntry>>,
    metrics: ConfigMetrics,
}

impl Default for ConfigProviderRegistry {
    fn default() -> Self {
        Self::new(DEFAULT_BUDGETS)
    }
}

impl ConfigProviderRegistry {
    pub fn new(budgets: ConfigBudgets) -> Self {
        Self {
            budgets,
            providers: RwLock::new(HashMap::new()),
            metrics: ConfigMetrics::default(),
        }
    }

    pub fn metrics(&self) -> ConfigMetrics {
        self.metrics.clone()
    }

    pub fn budgets(&self) -> ConfigBudgets {
        self.budgets
    }

    pub fn register(&self, provider: Arc<dyn ConfigProvider>) -> Result<(), ConfigError> {
        let descriptor = provider.descriptor();
        descriptor.validate_budgets(&self.budgets)?;
        let id = descriptor.provider_id.0.clone();
        if id.len() > self.budgets.max_id_bytes {
            return Err(ConfigError::BudgetExceeded {
                reason: format!("provider id exceeds {}", self.budgets.max_id_bytes),
            });
        }
        let mut guard = self.providers.write();
        if guard.len() >= self.budgets.max_providers && !guard.contains_key(&id) {
            return Err(ConfigError::BudgetExceeded {
                reason: format!("max_providers={}", self.budgets.max_providers),
            });
        }
        guard.insert(
            id,
            ProviderEntry {
                provider,
                cached_schema: Arc::new(descriptor),
            },
        );
        self.metrics.set_provider_count(guard.len() as u64);
        Ok(())
    }

    pub fn unregister(&self, provider_id: &str) -> bool {
        let mut guard = self.providers.write();
        let removed = guard.remove(provider_id).is_some();
        self.metrics.set_provider_count(guard.len() as u64);
        removed
    }

    pub fn list(&self) -> Vec<ConfigProviderId> {
        self.providers
            .read()
            .keys()
            .cloned()
            .map(ConfigProviderId::new)
            .collect()
    }

    pub fn get(&self, provider_id: &str) -> Result<ProviderEntry, ConfigError> {
        self.providers
            .read()
            .get(provider_id)
            .cloned()
            .ok_or(ConfigError::ProviderUnavailable)
    }

    pub fn schema(&self, provider_id: &str) -> Result<Arc<ConfigDescriptor>, ConfigError> {
        let entry = self.get(provider_id)?;
        self.metrics.inc_schema_cache_hit();
        Ok(entry.cached_schema)
    }

    pub fn ensure_scope(
        &self,
        provider_id: &str,
        scope: ConfigScope,
    ) -> Result<ProviderEntry, ConfigError> {
        let entry = self.get(provider_id)?;
        if !entry.cached_schema.supports_scope(scope) {
            return Err(ConfigError::ScopeUnsupported {
                reason: format!("provider `{provider_id}` does not support scope {scope:?}"),
            });
        }
        Ok(entry)
    }
}
