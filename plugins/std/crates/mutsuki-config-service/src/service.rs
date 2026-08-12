use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use crate::watch::ConfigWatchHub;
use crate::{
    ConfigAction, ConfigApplyRequest, ConfigApplyResult, ConfigCompareAndSetRequest, ConfigContext,
    ConfigDescriptor, ConfigDocumentKey, ConfigError, ConfigLifecycle, ConfigPath,
    ConfigProviderId, ConfigProviderRegistry, ConfigRepository, ConfigRevision, ConfigSnapshot,
    ConfigSource, ConfigValue, FieldDiff, RestartPolicy, RevisionChangedEvent,
    RevisionChangedListener, ValidationResult, capability,
};
use parking_lot::RwLock;

#[derive(Clone)]
pub struct ConfigService {
    registry: Arc<ConfigProviderRegistry>,
    repository: Arc<dyn ConfigRepository>,
    lifecycle: Arc<RwLock<Option<Arc<dyn ConfigLifecycle>>>>,
    watch: Arc<ConfigWatchHub>,
}

impl ConfigService {
    pub fn new(
        registry: Arc<ConfigProviderRegistry>,
        repository: Arc<dyn ConfigRepository>,
    ) -> Result<Self, ConfigError> {
        repository.recover()?;
        Ok(Self {
            registry,
            repository,
            lifecycle: Arc::new(RwLock::new(None)),
            watch: Arc::new(ConfigWatchHub::default()),
        })
    }

    #[must_use]
    pub fn with_lifecycle(self, lifecycle: Arc<dyn ConfigLifecycle>) -> Self {
        *self.lifecycle.write() = Some(lifecycle);
        self
    }

    pub fn set_lifecycle(&self, lifecycle: Arc<dyn ConfigLifecycle>) {
        *self.lifecycle.write() = Some(lifecycle);
    }

    #[must_use]
    pub fn registry(&self) -> &ConfigProviderRegistry {
        &self.registry
    }

    #[must_use]
    pub fn repository(&self) -> &Arc<dyn ConfigRepository> {
        &self.repository
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
        let entry = self.registry.ensure_scope(provider_id, &context.scope)?;
        let key = ConfigDocumentKey::new(provider_id, context.clone());
        let started = Instant::now();
        let result = self.repository.read(&key)?.map_or_else(
            || {
                Ok(ConfigSnapshot {
                    value: entry.provider.default_value(&context)?,
                    revision: ConfigRevision::ABSENT,
                    schema_version: entry.cached_schema.schema_version,
                    value_version: entry.cached_schema.value_version,
                    source: ConfigSource::Default,
                })
            },
            |snapshot| Ok(snapshot.into()),
        );
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
    ) -> Result<ValidationResult, ConfigError> {
        require_cap(caps, capability::VALUE_WRITE)?;
        let entry = self.registry.ensure_scope(provider_id, &context.scope)?;
        let started = Instant::now();
        let result = entry.provider.validate(candidate, context).await;
        self.registry
            .metrics()
            .observe_validate(started.elapsed().as_millis() as u64);
        result
    }

    pub async fn restore(
        &self,
        provider_id: &str,
        context: ConfigContext,
    ) -> Result<ConfigSnapshot, ConfigError> {
        let entry = self.registry.ensure_scope(provider_id, &context.scope)?;
        let snapshot = self
            .repository
            .read(&ConfigDocumentKey::new(provider_id, context.clone()))?
            .map(ConfigSnapshot::from)
            .unwrap_or(ConfigSnapshot {
                value: entry.provider.default_value(&context)?,
                revision: ConfigRevision::ABSENT,
                schema_version: entry.cached_schema.schema_version,
                value_version: entry.cached_schema.value_version,
                source: ConfigSource::Default,
            });
        entry
            .provider
            .restore(snapshot.value.clone(), snapshot.revision, context)
            .await?;
        Ok(snapshot)
    }

    pub async fn create_if_absent(
        &self,
        provider_id: &str,
        candidate: ConfigValue,
        context: ConfigContext,
    ) -> Result<ConfigSnapshot, ConfigError> {
        match self
            .apply_unchecked(
                provider_id,
                ConfigApplyRequest {
                    candidate,
                    expected_revision: ConfigRevision::ABSENT,
                    dry_run: false,
                },
                context.clone(),
            )
            .await
        {
            Ok(result) => self.read_unchecked(provider_id, context, result.revision),
            Err(ConfigError::RevisionConflict { .. }) => {
                let snapshot = self.read_unchecked_any(provider_id, context)?;
                Ok(snapshot)
            }
            Err(error) => Err(error),
        }
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
        self.apply_unchecked(provider_id, request, context).await
    }

    async fn apply_unchecked(
        &self,
        provider_id: &str,
        request: ConfigApplyRequest,
        context: ConfigContext,
    ) -> Result<ConfigApplyResult, ConfigError> {
        let entry = self.registry.ensure_scope(provider_id, &context.scope)?;
        let started = Instant::now();
        let current = self.read_unchecked_any(provider_id, context.clone())?;
        if current.revision != request.expected_revision {
            self.registry.metrics().inc_revision_conflict();
            return Err(ConfigError::RevisionConflict {
                expected: request.expected_revision.0,
                current: current.revision.0,
                diff: None,
            });
        }
        let validation = entry
            .provider
            .validate(request.candidate.clone(), context.clone())
            .await?;
        if !validation.ok {
            return Err(ConfigError::ValidationFailed { result: validation });
        }
        let next_revision = current.revision.next();
        let mut activation = entry
            .provider
            .prepare_activation(
                request.candidate,
                current.clone(),
                next_revision,
                context.clone(),
            )
            .await?;
        let diff = diff_objects(&current.value, activation.persisted_value());
        let restart_policy = restart_policy_for(&entry.cached_schema, &diff);
        let pending_actions = pending_actions_for(restart_policy);
        if request.dry_run {
            return Ok(ConfigApplyResult {
                revision: current.revision,
                applied: false,
                dry_run: true,
                actions: vec![ConfigAction::None],
                pending_actions,
                restart_policy,
                diff: Some(diff),
            });
        }

        let key = ConfigDocumentKey::new(provider_id, context.clone());
        let mut write = self
            .repository
            .prepare_compare_and_set(ConfigCompareAndSetRequest {
                key,
                expected_revision: current.revision,
                value: activation.persisted_value().clone(),
                schema_version: entry.cached_schema.schema_version,
                value_version: entry.cached_schema.value_version,
            })?;
        let commit_marker = activation.commit_marker().map(std::path::Path::to_path_buf);
        write.set_commit_marker(commit_marker.as_deref())?;
        if let Err(error) = activation.activate() {
            return Err(transaction_error(
                error,
                [activation.rollback(), write.rollback()],
            ));
        }
        let lifecycle = self.lifecycle.read().clone();
        let completed = if let Some(lifecycle) = &lifecycle {
            match lifecycle.execute(
                provider_id,
                &current.value,
                activation.persisted_value(),
                restart_policy,
                &pending_actions,
            ) {
                Ok(completed) => completed,
                Err(error) => {
                    return Err(transaction_error(
                        error,
                        [
                            activation.rollback(),
                            lifecycle.rollback(provider_id, &current.value, restart_policy, &[]),
                            write.rollback(),
                        ],
                    ));
                }
            }
        } else {
            Vec::new()
        };
        let committed = match write.commit() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let lifecycle_rollback = lifecycle.as_ref().map_or(Ok(()), |lifecycle| {
                    lifecycle.rollback(provider_id, &current.value, restart_policy, &completed)
                });
                return Err(transaction_error(
                    error,
                    [activation.rollback(), lifecycle_rollback, write.rollback()],
                ));
            }
        };
        if let Err(error) = activation.commit() {
            let lifecycle_rollback = lifecycle.as_ref().map_or(Ok(()), |lifecycle| {
                lifecycle.rollback(provider_id, &current.value, restart_policy, &completed)
            });
            return Err(transaction_error(
                error,
                [activation.rollback(), lifecycle_rollback, write.rollback()],
            ));
        }
        activation.finish()?;
        write.finish()?;

        let mut actions = vec![ConfigAction::Persisted];
        let mut pending = pending_actions;
        for action in completed {
            pending.retain(|candidate| candidate != &action);
            if !actions.contains(&action) {
                actions.push(action);
            }
        }
        self.watch.notify(RevisionChangedEvent {
            provider_id: ConfigProviderId::new(provider_id),
            revision: committed.revision,
            context,
        });
        self.registry
            .metrics()
            .observe_apply(started.elapsed().as_millis() as u64);
        Ok(ConfigApplyResult {
            revision: committed.revision,
            applied: true,
            dry_run: false,
            actions,
            pending_actions: pending,
            restart_policy,
            diff: Some(diff),
        })
    }

    fn read_unchecked_any(
        &self,
        provider_id: &str,
        context: ConfigContext,
    ) -> Result<ConfigSnapshot, ConfigError> {
        let entry = self.registry.ensure_scope(provider_id, &context.scope)?;
        self.repository
            .read(&ConfigDocumentKey::new(provider_id, context.clone()))?
            .map(ConfigSnapshot::from)
            .map_or_else(
                || {
                    Ok(ConfigSnapshot {
                        value: entry.provider.default_value(&context)?,
                        revision: ConfigRevision::ABSENT,
                        schema_version: entry.cached_schema.schema_version,
                        value_version: entry.cached_schema.value_version,
                        source: ConfigSource::Default,
                    })
                },
                Ok,
            )
    }

    fn read_unchecked(
        &self,
        provider_id: &str,
        context: ConfigContext,
        expected_revision: ConfigRevision,
    ) -> Result<ConfigSnapshot, ConfigError> {
        let snapshot = self.read_unchecked_any(provider_id, context)?;
        if snapshot.revision == expected_revision {
            Ok(snapshot)
        } else {
            Err(ConfigError::RevisionConflict {
                expected: expected_revision.0,
                current: snapshot.revision.0,
                diff: None,
            })
        }
    }

    #[must_use]
    pub fn metrics_snapshot(&self) -> crate::ConfigMetricsSnapshot {
        self.registry.metrics().snapshot()
    }
}

fn diff_objects(before: &ConfigValue, after: &ConfigValue) -> Vec<FieldDiff> {
    let left = before.as_object().cloned().unwrap_or_default();
    let right = after.as_object().cloned().unwrap_or_default();
    let keys = left
        .keys()
        .chain(right.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    keys.into_iter()
        .filter_map(|key| {
            let before = left.get(&key);
            let after = right.get(&key);
            (before != after).then(|| FieldDiff {
                path: ConfigPath(vec![key]),
                before: before.cloned(),
                after: after.cloned(),
            })
        })
        .collect()
}

fn restart_policy_for(descriptor: &ConfigDescriptor, diff: &[FieldDiff]) -> RestartPolicy {
    diff.iter().fold(RestartPolicy::None, |current, item| {
        let candidate = item
            .path
            .0
            .first()
            .and_then(|key| {
                descriptor
                    .root
                    .children
                    .iter()
                    .find(|node| &node.key.0 == key)
            })
            .map_or(RestartPolicy::None, |node| node.restart_policy);
        if restart_rank(candidate) > restart_rank(current) {
            candidate
        } else {
            current
        }
    })
}

const fn restart_rank(policy: RestartPolicy) -> u8 {
    match policy {
        RestartPolicy::None => 0,
        RestartPolicy::Reconfigure => 1,
        RestartPolicy::PluginReload => 2,
        RestartPolicy::ApplicationRestart => 3,
        RestartPolicy::HostRestart => 4,
    }
}

fn pending_actions_for(policy: RestartPolicy) -> Vec<ConfigAction> {
    match policy {
        RestartPolicy::None => Vec::new(),
        RestartPolicy::Reconfigure => vec![ConfigAction::Reconfigured],
        RestartPolicy::PluginReload => vec![ConfigAction::PluginReloaded],
        RestartPolicy::ApplicationRestart => vec![ConfigAction::ApplicationRestartScheduled],
        RestartPolicy::HostRestart => vec![ConfigAction::HostRestartScheduled],
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
    if caps
        .iter()
        .any(|capability| capability == "*" || capability == needed)
    {
        Ok(())
    } else {
        Err(ConfigError::PermissionDenied {
            capability: needed.to_owned(),
        })
    }
}

fn candidate_writes_secret(value: &ConfigValue) -> bool {
    match value {
        ConfigValue::Secret(crate::SecretState::Set { .. } | crate::SecretState::Clear) => true,
        ConfigValue::Object(map) => map.values().any(candidate_writes_secret),
        ConfigValue::Array(items) => items.iter().any(candidate_writes_secret),
        _ => false,
    }
}
