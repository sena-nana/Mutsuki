//! In-memory ConfigProvider used by tests and MVP demo plugins.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;

use crate::error::{ConfigError, FieldDiff, ValidationResult};
use crate::persist::ConfigPersistSink;
use crate::provider::{
    ConfigAction, ConfigApplyRequest, ConfigApplyResult, ConfigProvider, ConfigRevision,
    ConfigSnapshot, ConfigSource,
};
use crate::schema::{ConfigApplyMode, ConfigDescriptor, RestartPolicy};
use crate::scope::ConfigContext;
use crate::secret::SecretState;
use crate::validate::validate_structure;
use crate::value::{ConfigPath, ConfigValue};

#[derive(Clone)]
struct Stored {
    value: ConfigValue,
    secrets: HashMap<String, String>,
    revision: ConfigRevision,
    value_version: u32,
    persisted: bool,
}

pub struct MemoryConfigProvider {
    descriptor: ConfigDescriptor,
    apply_mode: ConfigApplyMode,
    store: Mutex<HashMap<String, Stored>>,
    defaults: ConfigValue,
    persist: Option<Arc<dyn ConfigPersistSink>>,
}

impl MemoryConfigProvider {
    pub fn new(
        descriptor: ConfigDescriptor,
        defaults: ConfigValue,
        apply_mode: ConfigApplyMode,
    ) -> Self {
        Self {
            descriptor,
            apply_mode,
            store: Mutex::new(HashMap::new()),
            defaults,
            persist: None,
        }
    }

    pub fn with_persist(mut self, sink: Arc<dyn ConfigPersistSink>) -> Self {
        self.persist = Some(sink);
        self
    }

    pub fn from_schema<T: crate::schema::MutsukiConfigSchema>(
        defaults: ConfigValue,
        apply_mode: ConfigApplyMode,
    ) -> Self {
        Self::new(T::schema(), defaults, apply_mode)
    }

    fn redact_for_read(
        &self,
        value: &ConfigValue,
        secrets: &HashMap<String, String>,
    ) -> ConfigValue {
        match value {
            ConfigValue::Object(map) => {
                let mut out = std::collections::BTreeMap::new();
                for (key, child) in map {
                    if self.is_secret_field(key) {
                        out.insert(
                            key.clone(),
                            ConfigValue::Secret(SecretState::for_read(secrets.contains_key(key))),
                        );
                    } else {
                        out.insert(key.clone(), self.redact_for_read(child, secrets));
                    }
                }
                ConfigValue::Object(out)
            }
            other => other.clone(),
        }
    }

    fn is_secret_field(&self, key: &str) -> bool {
        self.descriptor.root.children.iter().any(|node| {
            node.key.as_str() == key
                && (node.presentation.secret
                    || matches!(node.value_type, crate::schema::ConfigValueType::Secret))
        })
    }

    fn merge_secrets(
        &self,
        candidate: &ConfigValue,
        previous_secrets: &HashMap<String, String>,
    ) -> Result<(ConfigValue, HashMap<String, String>), ConfigError> {
        let Some(map) = candidate.as_object() else {
            return Err(ConfigError::ApplyRejected {
                reason: "candidate must be object".into(),
            });
        };
        let mut stored_value = std::collections::BTreeMap::new();
        let mut secrets = previous_secrets.clone();
        for (key, value) in map {
            if self.is_secret_field(key) {
                match value {
                    ConfigValue::Secret(SecretState::Keep) => {
                        stored_value.insert(
                            key.clone(),
                            ConfigValue::Secret(SecretState::for_read(secrets.contains_key(key))),
                        );
                    }
                    ConfigValue::Secret(SecretState::Clear) => {
                        secrets.remove(key);
                        stored_value.insert(key.clone(), ConfigValue::Secret(SecretState::Absent));
                    }
                    ConfigValue::Secret(SecretState::Set { value }) => {
                        secrets.insert(key.clone(), value.expose().to_string());
                        stored_value
                            .insert(key.clone(), ConfigValue::Secret(SecretState::Configured));
                    }
                    ConfigValue::Secret(SecretState::Configured)
                    | ConfigValue::Secret(SecretState::Absent)
                    | ConfigValue::Secret(SecretState::Unavailable) => {
                        // Read-shaped values on write mean Keep.
                        stored_value.insert(
                            key.clone(),
                            ConfigValue::Secret(SecretState::for_read(secrets.contains_key(key))),
                        );
                    }
                    ConfigValue::String(plaintext) => {
                        // Reject plaintext string write for secret fields — force SecretUpdate.
                        let _ = plaintext;
                        return Err(ConfigError::ApplyRejected {
                            reason: format!(
                                "secret field `{key}` requires SecretUpdate keep/set/clear"
                            ),
                        });
                    }
                    _ => {
                        return Err(ConfigError::ApplyRejected {
                            reason: format!("invalid secret update for `{key}`"),
                        });
                    }
                }
            } else {
                stored_value.insert(key.clone(), value.clone());
            }
        }
        Ok((ConfigValue::Object(stored_value), secrets))
    }

    fn diff_objects(before: &ConfigValue, after: &ConfigValue) -> Vec<FieldDiff> {
        let mut diffs = Vec::new();
        let left = before.as_object().cloned().unwrap_or_default();
        let right = after.as_object().cloned().unwrap_or_default();
        let mut keys: std::collections::BTreeSet<_> = left.keys().cloned().collect();
        keys.extend(right.keys().cloned());
        for key in keys {
            let b = left.get(&key);
            let a = right.get(&key);
            if b != a {
                diffs.push(FieldDiff {
                    path: ConfigPath(vec![key]),
                    before: b.cloned(),
                    after: a.cloned(),
                });
            }
        }
        diffs
    }

    fn restart_policy_for(&self, diff: &[FieldDiff]) -> RestartPolicy {
        let mut policy = RestartPolicy::None;
        for item in diff {
            let key = item.path.0.first().map(String::as_str).unwrap_or("");
            if let Some(node) = self
                .descriptor
                .root
                .children
                .iter()
                .find(|node| node.key.as_str() == key)
            {
                policy = stronger(policy, node.restart_policy);
            }
        }
        policy
    }
}

fn stronger(a: RestartPolicy, b: RestartPolicy) -> RestartPolicy {
    use RestartPolicy::*;
    let rank = |p: RestartPolicy| match p {
        None => 0,
        Reconfigure => 1,
        PluginReload => 2,
        BotRestart => 3,
        HostRestart => 4,
    };
    if rank(b) > rank(a) { b } else { a }
}

fn actions_for(
    policy: RestartPolicy,
    mode: ConfigApplyMode,
) -> (Vec<ConfigAction>, Vec<ConfigAction>) {
    // Persistence is claimed only after durable/memory commit succeeds.
    // Hot-reload side effects stay pending until ConfigLifecycle executes them.
    let mut done = vec![ConfigAction::Persisted];
    let mut pending = Vec::new();
    match (policy, mode) {
        (RestartPolicy::None, _) => done.push(ConfigAction::None),
        (RestartPolicy::Reconfigure, ConfigApplyMode::HotReload) => {
            pending.push(ConfigAction::Reconfigured);
        }
        (RestartPolicy::PluginReload, ConfigApplyMode::HotReload) => {
            pending.push(ConfigAction::PluginReloaded);
        }
        (RestartPolicy::BotRestart, _) => pending.push(ConfigAction::BotRestartScheduled),
        (RestartPolicy::HostRestart, _) => pending.push(ConfigAction::HostRestartScheduled),
        (RestartPolicy::Reconfigure | RestartPolicy::PluginReload, _) => {
            pending.push(ConfigAction::PluginReloaded);
        }
    }
    (done, pending)
}

#[async_trait]
impl ConfigProvider for MemoryConfigProvider {
    fn descriptor(&self) -> ConfigDescriptor {
        self.descriptor.clone()
    }

    async fn read(&self, context: ConfigContext) -> Result<ConfigSnapshot, ConfigError> {
        context.validate(&crate::budgets::DEFAULT_BUDGETS)?;
        if !self.descriptor.supports_scope(context.scope) {
            return Err(ConfigError::ScopeUnsupported {
                reason: format!("unsupported scope {:?}", context.scope),
            });
        }
        let key = context.storage_key();
        let guard = self.store.lock();
        if let Some(stored) = guard.get(&key) {
            Ok(ConfigSnapshot {
                value: self.redact_for_read(&stored.value, &stored.secrets),
                revision: stored.revision,
                schema_version: self.descriptor.schema_version,
                value_version: stored.value_version,
                source: if stored.persisted {
                    ConfigSource::Persisted
                } else {
                    ConfigSource::Memory
                },
            })
        } else {
            Ok(ConfigSnapshot {
                value: self.redact_for_read(&self.defaults, &HashMap::new()),
                revision: ConfigRevision::initial(),
                schema_version: self.descriptor.schema_version,
                value_version: self.descriptor.value_version,
                source: ConfigSource::Default,
            })
        }
    }

    async fn validate(
        &self,
        candidate: ConfigValue,
        context: ConfigContext,
    ) -> Result<ValidationResult, ConfigError> {
        context.validate(&crate::budgets::DEFAULT_BUDGETS)?;
        Ok(validate_structure(&self.descriptor, &candidate))
    }

    async fn apply(
        &self,
        request: ConfigApplyRequest,
        context: ConfigContext,
    ) -> Result<ConfigApplyResult, ConfigError> {
        context.validate(&crate::budgets::DEFAULT_BUDGETS)?;
        if !self.descriptor.supports_scope(context.scope) {
            return Err(ConfigError::ScopeUnsupported {
                reason: format!("unsupported scope {:?}", context.scope),
            });
        }
        let validation = validate_structure(&self.descriptor, &request.candidate);
        if !validation.ok {
            return Err(ConfigError::ValidationFailed { result: validation });
        }

        let key = context.storage_key();
        let mut guard = self.store.lock();
        let previous = guard.get(&key).cloned();
        let current_revision = previous
            .as_ref()
            .map(|s| s.revision)
            .unwrap_or_else(ConfigRevision::initial);
        if current_revision.0 != request.expected_revision.0 {
            return Err(ConfigError::RevisionConflict {
                expected: request.expected_revision.0,
                current: current_revision.0,
                diff: None,
            });
        }

        let prev_secrets = previous
            .as_ref()
            .map(|s| s.secrets.clone())
            .unwrap_or_default();
        let prev_value = previous
            .as_ref()
            .map(|s| s.value.clone())
            .unwrap_or_else(|| self.defaults.clone());
        let (merged, secrets) = self.merge_secrets(&request.candidate, &prev_secrets)?;
        let diff = Self::diff_objects(&prev_value, &merged);
        let policy = self.restart_policy_for(&diff);
        let (actions, pending) = actions_for(policy, self.apply_mode);
        let new_revision = if previous.is_some() {
            current_revision.next()
        } else {
            ConfigRevision::initial()
        };

        if request.dry_run {
            return Ok(ConfigApplyResult {
                revision: current_revision,
                applied: false,
                dry_run: true,
                actions: vec![ConfigAction::None],
                pending_actions: pending,
                restart_policy: policy,
                diff: Some(diff),
            });
        }

        let persisted = if let Some(sink) = &self.persist {
            sink.persist(&context, &merged, &secrets)?;
            true
        } else {
            false
        };

        guard.insert(
            key,
            Stored {
                value: merged,
                secrets,
                revision: if previous.is_some() {
                    current_revision.next()
                } else {
                    ConfigRevision(1)
                },
                value_version: self.descriptor.value_version,
                persisted,
            },
        );

        Ok(ConfigApplyResult {
            revision: new_revision,
            applied: true,
            dry_run: false,
            actions,
            pending_actions: pending,
            restart_policy: policy,
            diff: Some(diff),
        })
    }
}
