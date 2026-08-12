use std::collections::BTreeMap;
use std::path::Path;

use crate::{ConfigError, ConfigValue};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigSecretMutation {
    Set(String),
    Clear,
}

/// Reversible owner persistence prepared as part of one ConfigService CAS.
pub trait ConfigPersistTransaction: Send {
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

/// Optional persistence boundary for data that must never enter ConfigRepository.
///
/// The persisted value is already redacted. Secret plaintext is present only in
/// `secret_mutations` and must be handed directly to the Host secret boundary.
pub trait ConfigPersistSink: Send + Sync {
    fn prepare(
        &self,
        persisted_value: &ConfigValue,
        secret_mutations: BTreeMap<String, ConfigSecretMutation>,
    ) -> Result<Box<dyn ConfigPersistTransaction>, ConfigError>;
}
