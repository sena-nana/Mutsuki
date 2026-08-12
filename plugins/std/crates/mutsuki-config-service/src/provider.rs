use async_trait::async_trait;
use std::path::Path;

use crate::{
    ConfigContext, ConfigDescriptor, ConfigError, ConfigRevision, ConfigSnapshot, ConfigValue,
    ValidationResult,
};

/// Reversible domain-side activation prepared before repository commit.
pub trait ConfigActivation: Send {
    fn activate(&mut self) -> Result<(), ConfigError>;
    fn commit_marker(&self) -> Option<&Path> {
        None
    }
    fn commit(&mut self) -> Result<(), ConfigError>;
    fn finish(&mut self) -> Result<(), ConfigError> {
        Ok(())
    }
    fn rollback(&mut self) -> Result<(), ConfigError>;
}

pub struct PreparedConfigActivation {
    persisted_value: ConfigValue,
    transaction: Option<Box<dyn ConfigActivation>>,
}

impl PreparedConfigActivation {
    #[must_use]
    pub fn new(persisted_value: ConfigValue, transaction: Box<dyn ConfigActivation>) -> Self {
        Self {
            persisted_value,
            transaction: Some(transaction),
        }
    }

    #[must_use]
    pub fn unchanged(persisted_value: ConfigValue) -> Self {
        Self {
            persisted_value,
            transaction: None,
        }
    }

    #[must_use]
    pub fn persisted_value(&self) -> &ConfigValue {
        &self.persisted_value
    }

    pub fn activate(&mut self) -> Result<(), ConfigError> {
        if let Some(transaction) = &mut self.transaction {
            transaction.activate()?;
        }
        Ok(())
    }

    #[must_use]
    pub fn commit_marker(&self) -> Option<&Path> {
        self.transaction
            .as_ref()
            .and_then(|transaction| transaction.commit_marker())
    }

    pub fn commit(&mut self) -> Result<(), ConfigError> {
        if let Some(transaction) = &mut self.transaction {
            transaction.commit()?;
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Result<(), ConfigError> {
        if let Some(transaction) = &mut self.transaction {
            transaction.finish()?;
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
}

#[derive(Debug, Clone)]
pub struct ConfigRegistration {
    pub id: String,
    pub scope: crate::ConfigScope,
    pub apply_mode: crate::ConfigApplyMode,
}

#[async_trait]
pub trait ConfigProvider: Send + Sync {
    fn descriptor(&self) -> ConfigDescriptor;

    fn default_value(&self, context: &ConfigContext) -> Result<ConfigValue, ConfigError>;

    async fn validate(
        &self,
        candidate: ConfigValue,
        context: ConfigContext,
    ) -> Result<ValidationResult, ConfigError>;

    async fn prepare_activation(
        &self,
        candidate: ConfigValue,
        current: ConfigSnapshot,
        next_revision: ConfigRevision,
        context: ConfigContext,
    ) -> Result<PreparedConfigActivation, ConfigError>;

    /// Restore a committed value during startup without writing the repository.
    async fn restore(
        &self,
        value: ConfigValue,
        revision: ConfigRevision,
        context: ConfigContext,
    ) -> Result<(), ConfigError> {
        let current = ConfigSnapshot {
            value: self.default_value(&context)?,
            revision: ConfigRevision::ABSENT,
            schema_version: self.descriptor().schema_version,
            value_version: self.descriptor().value_version,
            source: crate::ConfigSource::Default,
        };
        let mut activation = self
            .prepare_activation(value, current, revision, context)
            .await?;
        activation.activate()?;
        activation.commit()?;
        activation.finish()
    }
}
