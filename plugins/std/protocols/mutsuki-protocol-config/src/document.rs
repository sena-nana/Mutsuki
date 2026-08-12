use serde::{Deserialize, Serialize};

use crate::{ConfigContext, ConfigValue, FieldDiff, RestartPolicy};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct ConfigRevision(pub u64);

impl ConfigRevision {
    pub const ABSENT: Self = Self(0);

    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConfigDocumentKey {
    pub provider_id: String,
    pub context: ConfigContext,
}

impl ConfigDocumentKey {
    #[must_use]
    pub fn new(provider_id: impl Into<String>, context: ConfigContext) -> Self {
        Self {
            provider_id: provider_id.into(),
            context,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSource {
    Default,
    Persisted,
    Migrated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigDocumentSnapshot {
    pub key: ConfigDocumentKey,
    pub value: ConfigValue,
    pub revision: ConfigRevision,
    pub schema_version: u32,
    pub value_version: u32,
    pub source: ConfigSource,
}

/// Compatibility read shape used by provider-specific clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigSnapshot {
    pub value: ConfigValue,
    pub revision: ConfigRevision,
    pub schema_version: u32,
    pub value_version: u32,
    pub source: ConfigSource,
}

impl From<ConfigDocumentSnapshot> for ConfigSnapshot {
    fn from(snapshot: ConfigDocumentSnapshot) -> Self {
        Self {
            value: snapshot.value,
            revision: snapshot.revision,
            schema_version: snapshot.schema_version,
            value_version: snapshot.value_version,
            source: snapshot.source,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigCompareAndSetRequest {
    pub key: ConfigDocumentKey,
    pub expected_revision: ConfigRevision,
    pub value: ConfigValue,
    pub schema_version: u32,
    pub value_version: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigApplyRequest {
    pub candidate: ConfigValue,
    pub expected_revision: ConfigRevision,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigAction {
    Persisted,
    Reconfigured,
    PluginReloaded,
    ApplicationRestartScheduled,
    HostRestartScheduled,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigApplyResult {
    pub revision: ConfigRevision,
    pub applied: bool,
    pub dry_run: bool,
    pub actions: Vec<ConfigAction>,
    pub pending_actions: Vec<ConfigAction>,
    pub restart_policy: RestartPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<Vec<FieldDiff>>,
}
