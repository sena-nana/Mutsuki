//! Schema-backed provider with reversible in-process activation.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use std::sync::Mutex;

use crate::{
    ConfigActivation, ConfigApplyMode, ConfigContext, ConfigDescriptor, ConfigError,
    ConfigPersistSink, ConfigPersistTransaction, ConfigProvider, ConfigRevision,
    ConfigSecretMutation, ConfigSnapshot, ConfigValue, PreparedConfigActivation, SecretState,
    ValidationResult, validate_structure,
};

#[derive(Clone)]
struct ActiveValue {
    secrets: HashMap<String, String>,
}

type NormalizedCandidate = (
    ConfigValue,
    HashMap<String, String>,
    BTreeMap<String, ConfigSecretMutation>,
);

struct MemoryActivation {
    active: Arc<Mutex<HashMap<String, ActiveValue>>>,
    key: String,
    before: Option<ActiveValue>,
    after: ActiveValue,
    persistence: Option<Box<dyn ConfigPersistTransaction>>,
    activated: bool,
    committed: bool,
    finished: bool,
}

impl ConfigActivation for MemoryActivation {
    fn activate(&mut self) -> Result<(), ConfigError> {
        if let Some(persistence) = &mut self.persistence {
            persistence.activate()?;
        }
        self.active
            .lock()
            .unwrap()
            .insert(self.key.clone(), self.after.clone());
        self.activated = true;
        Ok(())
    }

    fn commit_marker(&self) -> Option<&std::path::Path> {
        self.persistence
            .as_ref()
            .and_then(|transaction| transaction.commit_marker())
    }

    fn commit(&mut self) -> Result<(), ConfigError> {
        if let Some(persistence) = &mut self.persistence {
            persistence.commit()?;
        }
        self.committed = true;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), ConfigError> {
        if let Some(persistence) = &mut self.persistence {
            persistence.finish()?;
        }
        self.persistence = None;
        self.finished = true;
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), ConfigError> {
        if self.activated {
            let mut active = self.active.lock().unwrap();
            if let Some(before) = self.before.clone() {
                active.insert(self.key.clone(), before);
            } else {
                active.remove(&self.key);
            }
        }
        if let Some(persistence) = &mut self.persistence {
            persistence.rollback()?;
        }
        self.finished = true;
        Ok(())
    }
}

impl Drop for MemoryActivation {
    fn drop(&mut self) {
        if !self.finished {
            if self.committed {
                let _ = self.finish();
            } else {
                let _ = self.rollback();
            }
        }
    }
}

pub struct MemoryConfigProvider {
    descriptor: ConfigDescriptor,
    #[allow(dead_code)]
    apply_mode: ConfigApplyMode,
    defaults: ConfigValue,
    initial_secrets: HashMap<String, String>,
    persist: Option<Arc<dyn ConfigPersistSink>>,
    active: Arc<Mutex<HashMap<String, ActiveValue>>>,
}

impl MemoryConfigProvider {
    #[must_use]
    pub fn new(
        descriptor: ConfigDescriptor,
        defaults: ConfigValue,
        apply_mode: ConfigApplyMode,
    ) -> Self {
        Self {
            descriptor,
            apply_mode,
            defaults,
            initial_secrets: HashMap::new(),
            persist: None,
            active: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[must_use]
    pub fn with_initial_secret(mut self, field: impl Into<String>, value: String) -> Self {
        self.initial_secrets.insert(field.into(), value);
        self
    }

    #[must_use]
    pub fn with_persist(mut self, sink: Arc<dyn ConfigPersistSink>) -> Self {
        self.persist = Some(sink);
        self
    }

    #[must_use]
    pub fn from_schema<T: crate::MutsukiConfigSchema>(
        defaults: ConfigValue,
        apply_mode: ConfigApplyMode,
    ) -> Self {
        Self::new(T::schema(), defaults, apply_mode)
    }

    fn is_secret_field(&self, key: &str) -> bool {
        self.descriptor.root.children.iter().any(|node| {
            node.key.as_str() == key
                && (node.presentation.secret
                    || matches!(node.value_type, crate::ConfigValueType::Secret))
        })
    }

    fn normalize_candidate(
        &self,
        candidate: &ConfigValue,
        previous_secrets: &HashMap<String, String>,
    ) -> Result<NormalizedCandidate, ConfigError> {
        let Some(map) = candidate.as_object() else {
            return Err(ConfigError::ApplyRejected {
                reason: "candidate must be object".into(),
            });
        };
        let mut normalized = BTreeMap::new();
        let mut secrets = previous_secrets.clone();
        let mut mutations = BTreeMap::new();
        for (key, value) in map {
            if !self.is_secret_field(key) {
                normalized.insert(key.clone(), value.clone());
                continue;
            }
            match value {
                ConfigValue::Secret(SecretState::Set { value }) => {
                    let value = value.expose().to_owned();
                    secrets.insert(key.clone(), value.clone());
                    mutations.insert(key.clone(), ConfigSecretMutation::Set(value));
                }
                ConfigValue::Secret(SecretState::Clear) => {
                    secrets.remove(key);
                    mutations.insert(key.clone(), ConfigSecretMutation::Clear);
                }
                ConfigValue::Secret(
                    SecretState::Keep
                    | SecretState::Configured
                    | SecretState::Absent
                    | SecretState::Unavailable,
                ) => {}
                ConfigValue::String(_) => {
                    return Err(ConfigError::ApplyRejected {
                        reason: format!("secret field `{key}` requires keep/set/clear"),
                    });
                }
                _ => {
                    return Err(ConfigError::ApplyRejected {
                        reason: format!("invalid secret update for `{key}`"),
                    });
                }
            }
            normalized.insert(
                key.clone(),
                ConfigValue::Secret(SecretState::for_read(secrets.contains_key(key))),
            );
        }
        Ok((ConfigValue::Object(normalized), secrets, mutations))
    }

    fn redact_defaults(&self) -> ConfigValue {
        let Some(defaults) = self.defaults.as_object() else {
            return self.defaults.clone();
        };
        ConfigValue::Object(
            defaults
                .iter()
                .map(|(key, value)| {
                    let value = if self.is_secret_field(key) {
                        ConfigValue::Secret(SecretState::for_read(
                            self.initial_secrets.contains_key(key),
                        ))
                    } else {
                        value.clone()
                    };
                    (key.clone(), value)
                })
                .collect(),
        )
    }
}

#[async_trait]
impl ConfigProvider for MemoryConfigProvider {
    fn descriptor(&self) -> ConfigDescriptor {
        self.descriptor.clone()
    }

    fn default_value(&self, context: &ConfigContext) -> Result<ConfigValue, ConfigError> {
        context.validate(&crate::DEFAULT_BUDGETS)?;
        Ok(self.redact_defaults())
    }

    async fn validate(
        &self,
        candidate: ConfigValue,
        context: ConfigContext,
    ) -> Result<ValidationResult, ConfigError> {
        context.validate(&crate::DEFAULT_BUDGETS)?;
        Ok(validate_structure(&self.descriptor, &candidate))
    }

    async fn prepare_activation(
        &self,
        candidate: ConfigValue,
        _current: ConfigSnapshot,
        _next_revision: ConfigRevision,
        context: ConfigContext,
    ) -> Result<PreparedConfigActivation, ConfigError> {
        context.validate(&crate::DEFAULT_BUDGETS)?;
        let key = context.storage_key();
        let before = self.active.lock().unwrap().get(&key).cloned();
        let previous_secrets = before.as_ref().map_or_else(
            || self.initial_secrets.clone(),
            |value| value.secrets.clone(),
        );
        let (value, secrets, mutations) =
            self.normalize_candidate(&candidate, &previous_secrets)?;
        let persistence = self
            .persist
            .as_ref()
            .map(|sink| sink.prepare(&value, mutations))
            .transpose()?;
        Ok(PreparedConfigActivation::new(
            value.clone(),
            Box::new(MemoryActivation {
                active: self.active.clone(),
                key,
                before,
                after: ActiveValue { secrets },
                persistence,
                activated: false,
                committed: false,
                finished: false,
            }),
        ))
    }
}
