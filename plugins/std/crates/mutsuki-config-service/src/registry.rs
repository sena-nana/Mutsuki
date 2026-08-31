//! Provider registry with schema cache and metrics hooks.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use std::sync::RwLock;

use crate::metrics::ConfigMetrics;
use crate::provider::ConfigProvider;
use crate::{
    ConfigBudgets, ConfigDescriptor, ConfigError, ConfigProviderId, ConfigScope, DEFAULT_BUDGETS,
};

#[derive(Clone)]
pub struct ProviderEntry {
    pub provider: Arc<dyn ConfigProvider>,
    pub cached_schema: Arc<ConfigDescriptor>,
    registration_id: u64,
}

pub struct ConfigProviderRegistry {
    budgets: ConfigBudgets,
    providers: RwLock<HashMap<String, Vec<ProviderEntry>>>,
    metrics: ConfigMetrics,
    next_registration_id: AtomicU64,
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
            next_registration_id: AtomicU64::new(1),
        }
    }

    pub fn metrics(&self) -> ConfigMetrics {
        self.metrics.clone()
    }

    pub fn budgets(&self) -> ConfigBudgets {
        self.budgets
    }

    pub fn register(&self, provider: Arc<dyn ConfigProvider>) -> Result<(), ConfigError> {
        self.register_entry(provider, false, false).map(|_| ())
    }

    pub fn register_owned(
        self: &Arc<Self>,
        provider: Arc<dyn ConfigProvider>,
    ) -> Result<ConfigProviderRegistration, ConfigError> {
        let (provider_id, registration_id) = self.register_entry(provider, true, false)?;
        Ok(ConfigProviderRegistration {
            registry: self.clone(),
            provider_id,
            registration_id,
            disposed: false,
        })
    }

    pub fn register_owned_staged(
        self: &Arc<Self>,
        provider: Arc<dyn ConfigProvider>,
    ) -> Result<ConfigProviderRegistration, ConfigError> {
        let (provider_id, registration_id) = self.register_entry(provider, true, true)?;
        Ok(ConfigProviderRegistration {
            registry: self.clone(),
            provider_id,
            registration_id,
            disposed: false,
        })
    }

    fn register_entry(
        &self,
        provider: Arc<dyn ConfigProvider>,
        owned: bool,
        staged: bool,
    ) -> Result<(String, u64), ConfigError> {
        let (id, descriptor) = self.validate_provider(&provider)?;
        let mut guard = self.providers.write().unwrap();
        if guard.len() >= self.budgets.max_providers && !guard.contains_key(&id) {
            return Err(ConfigError::BudgetExceeded {
                reason: format!("max_providers={}", self.budgets.max_providers),
            });
        }
        let registration_id = self.next_registration_id.fetch_add(1, Ordering::Relaxed);
        let entry = ProviderEntry {
            provider,
            cached_schema: descriptor,
            registration_id,
        };
        if owned && staged {
            let entries = guard.entry(id.clone()).or_default();
            let before_active = entries.len().saturating_sub(1);
            entries.insert(before_active, entry);
        } else if owned {
            guard.entry(id.clone()).or_default().push(entry);
        } else {
            guard.insert(id.clone(), vec![entry]);
        }
        self.metrics.set_provider_count(guard.len() as u64);
        Ok((id, registration_id))
    }

    pub(crate) fn candidate_entry(
        &self,
        provider: Arc<dyn ConfigProvider>,
        scope: &ConfigScope,
    ) -> Result<(String, ProviderEntry), ConfigError> {
        let (id, descriptor) = self.validate_provider(&provider)?;
        if !descriptor.supports_scope(scope) {
            return Err(ConfigError::ScopeUnsupported {
                reason: format!("provider `{id}` does not support scope {scope:?}"),
            });
        }
        let guard = self.providers.read().unwrap();
        if guard.len() >= self.budgets.max_providers && !guard.contains_key(&id) {
            return Err(ConfigError::BudgetExceeded {
                reason: format!("max_providers={}", self.budgets.max_providers),
            });
        }
        Ok((
            id,
            ProviderEntry {
                provider,
                cached_schema: descriptor,
                registration_id: 0,
            },
        ))
    }

    fn validate_provider(
        &self,
        provider: &Arc<dyn ConfigProvider>,
    ) -> Result<(String, Arc<ConfigDescriptor>), ConfigError> {
        let descriptor = provider.descriptor();
        descriptor.validate_budgets(&self.budgets)?;
        let id = descriptor.provider_id.0.clone();
        if id.len() > self.budgets.max_id_bytes {
            return Err(ConfigError::BudgetExceeded {
                reason: format!("provider id exceeds {}", self.budgets.max_id_bytes),
            });
        }
        Ok((id, Arc::new(descriptor)))
    }

    pub fn unregister(&self, provider_id: &str) -> bool {
        let mut guard = self.providers.write().unwrap();
        let removed = guard.remove(provider_id).is_some();
        self.metrics.set_provider_count(guard.len() as u64);
        removed
    }

    fn unregister_registration(&self, provider_id: &str, registration_id: u64) -> bool {
        let mut guard = self.providers.write().unwrap();
        let mut removed = false;
        let mut empty = false;
        if let Some(entries) = guard.get_mut(provider_id) {
            let previous_len = entries.len();
            entries.retain(|entry| entry.registration_id != registration_id);
            removed = entries.len() != previous_len;
            empty = entries.is_empty();
        }
        if empty {
            guard.remove(provider_id);
        }
        self.metrics.set_provider_count(guard.len() as u64);
        removed
    }

    pub fn list(&self) -> Vec<ConfigProviderId> {
        self.providers
            .read()
            .unwrap()
            .keys()
            .cloned()
            .map(ConfigProviderId::new)
            .collect()
    }

    pub fn get(&self, provider_id: &str) -> Result<ProviderEntry, ConfigError> {
        self.providers
            .read()
            .unwrap()
            .get(provider_id)
            .and_then(|entries| entries.last().cloned())
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
        scope: &ConfigScope,
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

pub struct ConfigProviderRegistration {
    registry: Arc<ConfigProviderRegistry>,
    provider_id: String,
    registration_id: u64,
    disposed: bool,
}

impl ConfigProviderRegistration {
    pub fn dispose(&mut self) -> bool {
        if self.disposed {
            return false;
        }
        self.disposed = true;
        self.registry
            .unregister_registration(&self.provider_id, self.registration_id)
    }
}

impl Drop for ConfigProviderRegistration {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}
