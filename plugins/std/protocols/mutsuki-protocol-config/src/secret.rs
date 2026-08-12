//! Secret field semantics: redaction, keep/set/clear patch, never plaintext in logs.

use serde::{Deserialize, Serialize};

/// Opaque secret material. Debug redacts contents.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretValue(<redacted>)")
    }
}

/// Read-side secret status — never carries plaintext.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SecretState {
    Configured,
    Absent,
    Unavailable,
    /// Apply/patch sentinel: leave existing secret unchanged.
    Keep,
    /// Apply/patch: replace secret. Only accepted on write path; never returned from read.
    Set {
        value: SecretValue,
    },
    /// Apply/patch: clear secret.
    Clear,
}

impl SecretState {
    pub fn for_read(present: bool) -> Self {
        if present {
            Self::Configured
        } else {
            Self::Absent
        }
    }

    pub fn is_write_sentinel(&self) -> bool {
        matches!(self, Self::Keep | Self::Set { .. } | Self::Clear)
    }
}

/// Explicit secret update operation (preferred over empty-string meaning keep).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SecretUpdate {
    Keep,
    Set { value: SecretValue },
    Clear,
}

impl From<SecretUpdate> for SecretState {
    fn from(value: SecretUpdate) -> Self {
        match value {
            SecretUpdate::Keep => Self::Keep,
            SecretUpdate::Set { value } => Self::Set { value },
            SecretUpdate::Clear => Self::Clear,
        }
    }
}
