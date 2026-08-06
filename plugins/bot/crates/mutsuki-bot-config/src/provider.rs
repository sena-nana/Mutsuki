//! ConfigProvider lifecycle: read / validate / apply / revision.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::{ConfigError, FieldDiff, ValidationResult};
use crate::schema::{ConfigApplyMode, ConfigDescriptor, RestartPolicy};
use crate::scope::ConfigContext;
use crate::value::ConfigValue;

pub trait ConfigChangeTransaction: Send {
    fn activate(&mut self) -> Result<(), ConfigError>;
    fn commit(&mut self) -> Result<(), ConfigError>;
    fn rollback(&mut self) -> Result<(), ConfigError>;
}

pub struct PreparedConfigChange {
    result: ConfigApplyResult,
    transaction: Option<Box<dyn ConfigChangeTransaction>>,
}

impl PreparedConfigChange {
    pub fn new(result: ConfigApplyResult, transaction: Box<dyn ConfigChangeTransaction>) -> Self {
        Self {
            result,
            transaction: Some(transaction),
        }
    }

    pub fn dry_run(result: ConfigApplyResult) -> Self {
        Self {
            result,
            transaction: None,
        }
    }

    pub fn result(&self) -> &ConfigApplyResult {
        &self.result
    }

    pub fn result_mut(&mut self) -> &mut ConfigApplyResult {
        &mut self.result
    }

    pub fn activate(&mut self) -> Result<(), ConfigError> {
        if let Some(transaction) = &mut self.transaction {
            transaction.activate()?;
        }
        Ok(())
    }

    pub fn commit(&mut self) -> Result<(), ConfigError> {
        if let Some(transaction) = &mut self.transaction {
            transaction.commit()?;
        }
        self.transaction = None;
        Ok(())
    }

    pub fn rollback(&mut self) -> Result<(), ConfigError> {
        if let Some(transaction) = &mut self.transaction {
            transaction.rollback()?;
        }
        self.transaction = None;
        Ok(())
    }

    pub fn into_result(self) -> ConfigApplyResult {
        self.result
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConfigRevision(pub u64);

impl ConfigRevision {
    pub fn initial() -> Self {
        Self(1)
    }

    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSource {
    Default,
    Persisted,
    Memory,
    Migrated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigSnapshot {
    pub value: ConfigValue,
    pub revision: ConfigRevision,
    pub schema_version: u32,
    pub value_version: u32,
    pub source: ConfigSource,
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
    BotRestartScheduled,
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

#[derive(Debug, Clone)]
pub struct ConfigRegistration {
    pub id: String,
    pub scope: crate::scope::ConfigScope,
    pub apply_mode: ConfigApplyMode,
}

#[async_trait]
pub trait ConfigProvider: Send + Sync {
    fn descriptor(&self) -> ConfigDescriptor;

    async fn read(&self, context: ConfigContext) -> Result<ConfigSnapshot, ConfigError>;

    async fn validate(
        &self,
        candidate: ConfigValue,
        context: ConfigContext,
    ) -> Result<ValidationResult, ConfigError>;

    async fn prepare(
        &self,
        request: ConfigApplyRequest,
        context: ConfigContext,
    ) -> Result<PreparedConfigChange, ConfigError>;

    async fn apply(
        &self,
        request: ConfigApplyRequest,
        context: ConfigContext,
    ) -> Result<ConfigApplyResult, ConfigError> {
        let mut prepared = self.prepare(request, context).await?;
        prepared.activate()?;
        prepared.commit()?;
        Ok(prepared.into_result())
    }
}
