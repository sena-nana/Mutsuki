//! SQLite-backed configuration repository.
//!
//! The caller supplies both path and logical namespace. This crate never
//! derives product directories or installs itself as a framework default.

use std::path::Path;
use std::sync::Arc;

use mutsuki_config_service::{
    ConfigCompareAndSetRequest, ConfigDocumentKey, ConfigDocumentSnapshot, ConfigError,
    ConfigRepository, ConfigRevision, ConfigSource, PreparedConfigWrite,
};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

pub const PLUGIN_ID: &str = "mutsuki.config.repository.sqlite";

#[derive(Clone)]
pub struct SqliteConfigRepository {
    connection: Arc<Mutex<Connection>>,
    namespace: Arc<str>,
}

impl SqliteConfigRepository {
    pub fn open(path: impl AsRef<Path>, namespace: impl Into<String>) -> Result<Self, ConfigError> {
        let namespace = namespace.into();
        if namespace.trim().is_empty() {
            return Err(persistence("document namespace must be non-empty"));
        }
        let connection = Connection::open(path).map_err(sql_error)?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 CREATE TABLE IF NOT EXISTS config_documents (
                   namespace TEXT NOT NULL,
                   provider_id TEXT NOT NULL,
                   context_key TEXT NOT NULL,
                   context_json TEXT NOT NULL,
                   revision INTEGER NOT NULL,
                   schema_version INTEGER NOT NULL,
                   value_version INTEGER NOT NULL,
                   value_json TEXT NOT NULL,
                   PRIMARY KEY(namespace, provider_id, context_key)
                 );
                 CREATE TABLE IF NOT EXISTS config_pending (
                   namespace TEXT NOT NULL,
                   provider_id TEXT NOT NULL,
                   context_key TEXT NOT NULL,
                   expected_revision INTEGER NOT NULL,
                   request_json TEXT NOT NULL,
                   PRIMARY KEY(namespace, provider_id, context_key)
                 );",
            )
            .map_err(sql_error)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            namespace: namespace.into(),
        })
    }

    fn current_revision(
        connection: &Connection,
        namespace: &str,
        key: &ConfigDocumentKey,
    ) -> Result<ConfigRevision, ConfigError> {
        connection
            .query_row(
                "SELECT revision FROM config_documents
                 WHERE namespace = ?1 AND provider_id = ?2 AND context_key = ?3",
                params![namespace, key.provider_id, key.context.storage_key()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map(|revision| {
                revision.map_or(ConfigRevision::ABSENT, |value| ConfigRevision(value as u64))
            })
            .map_err(sql_error)
    }
}

struct SqlitePreparedWrite {
    repository: SqliteConfigRepository,
    request: ConfigCompareAndSetRequest,
    before: Option<ConfigDocumentSnapshot>,
    committed: bool,
    finished: bool,
}

impl PreparedConfigWrite for SqlitePreparedWrite {
    fn commit(&mut self) -> Result<ConfigDocumentSnapshot, ConfigError> {
        let mut connection = self.repository.connection.lock();
        let transaction = connection.transaction().map_err(sql_error)?;
        let current = SqliteConfigRepository::current_revision(
            &transaction,
            &self.repository.namespace,
            &self.request.key,
        )?;
        if current != self.request.expected_revision {
            return Err(ConfigError::RevisionConflict {
                expected: self.request.expected_revision.0,
                current: current.0,
                diff: None,
            });
        }
        let next = current.next();
        let context_json = serde_json::to_string(&self.request.key.context).map_err(json_error)?;
        let value_json = serde_json::to_string(&self.request.value).map_err(json_error)?;
        transaction
            .execute(
                "INSERT INTO config_documents
                   (namespace, provider_id, context_key, context_json, revision,
                    schema_version, value_version, value_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(namespace, provider_id, context_key) DO UPDATE SET
                   context_json = excluded.context_json,
                   revision = excluded.revision,
                   schema_version = excluded.schema_version,
                   value_version = excluded.value_version,
                   value_json = excluded.value_json",
                params![
                    &*self.repository.namespace,
                    self.request.key.provider_id,
                    self.request.key.context.storage_key(),
                    context_json,
                    next.0 as i64,
                    self.request.schema_version,
                    self.request.value_version,
                    value_json,
                ],
            )
            .map_err(sql_error)?;
        delete_pending(&transaction, &self.repository.namespace, &self.request.key)?;
        transaction.commit().map_err(sql_error)?;
        self.committed = true;
        self.finished = true;
        Ok(ConfigDocumentSnapshot {
            key: self.request.key.clone(),
            value: self.request.value.clone(),
            revision: next,
            schema_version: self.request.schema_version,
            value_version: self.request.value_version,
            source: ConfigSource::Persisted,
        })
    }

    fn rollback(&mut self) -> Result<(), ConfigError> {
        let mut connection = self.repository.connection.lock();
        let transaction = connection.transaction().map_err(sql_error)?;
        if self.committed {
            match &self.before {
                Some(before) => {
                    transaction
                        .execute(
                            "UPDATE config_documents SET context_json = ?4, revision = ?5,
                               schema_version = ?6, value_version = ?7, value_json = ?8
                             WHERE namespace = ?1 AND provider_id = ?2 AND context_key = ?3",
                            params![
                                &*self.repository.namespace,
                                before.key.provider_id,
                                before.key.context.storage_key(),
                                serde_json::to_string(&before.key.context).map_err(json_error)?,
                                before.revision.0 as i64,
                                before.schema_version,
                                before.value_version,
                                serde_json::to_string(&before.value).map_err(json_error)?,
                            ],
                        )
                        .map_err(sql_error)?;
                }
                None => {
                    transaction
                        .execute(
                            "DELETE FROM config_documents
                             WHERE namespace = ?1 AND provider_id = ?2 AND context_key = ?3",
                            params![
                                &*self.repository.namespace,
                                self.request.key.provider_id,
                                self.request.key.context.storage_key(),
                            ],
                        )
                        .map_err(sql_error)?;
                }
            }
        }
        delete_pending(&transaction, &self.repository.namespace, &self.request.key)?;
        transaction.commit().map_err(sql_error)?;
        self.finished = true;
        Ok(())
    }
}

impl Drop for SqlitePreparedWrite {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.rollback();
        }
    }
}

impl ConfigRepository for SqliteConfigRepository {
    fn read(&self, key: &ConfigDocumentKey) -> Result<Option<ConfigDocumentSnapshot>, ConfigError> {
        let connection = self.connection.lock();
        connection
            .query_row(
                "SELECT context_json, revision, schema_version, value_version, value_json
                 FROM config_documents
                 WHERE namespace = ?1 AND provider_id = ?2 AND context_key = ?3",
                params![&*self.namespace, key.provider_id, key.context.storage_key()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, u32>(2)?,
                        row.get::<_, u32>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(sql_error)?
            .map(
                |(context, revision, schema_version, value_version, value)| {
                    Ok(ConfigDocumentSnapshot {
                        key: ConfigDocumentKey {
                            provider_id: key.provider_id.clone(),
                            context: serde_json::from_str(&context).map_err(json_error)?,
                        },
                        value: serde_json::from_str(&value).map_err(json_error)?,
                        revision: ConfigRevision(revision as u64),
                        schema_version,
                        value_version,
                        source: ConfigSource::Persisted,
                    })
                },
            )
            .transpose()
    }

    fn prepare_compare_and_set(
        &self,
        request: ConfigCompareAndSetRequest,
    ) -> Result<Box<dyn PreparedConfigWrite>, ConfigError> {
        let before = self.read(&request.key)?;
        let current = before
            .as_ref()
            .map_or(ConfigRevision::ABSENT, |snapshot| snapshot.revision);
        if current != request.expected_revision {
            return Err(ConfigError::RevisionConflict {
                expected: request.expected_revision.0,
                current: current.0,
                diff: None,
            });
        }
        let request_json = serde_json::to_string(&request).map_err(json_error)?;
        self.connection
            .lock()
            .execute(
                "INSERT INTO config_pending
                   (namespace, provider_id, context_key, expected_revision, request_json)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    &*self.namespace,
                    request.key.provider_id,
                    request.key.context.storage_key(),
                    request.expected_revision.0 as i64,
                    request_json,
                ],
            )
            .map_err(|error| match error {
                rusqlite::Error::SqliteFailure(ref code, _)
                    if code.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    ConfigError::ApplyRejected {
                        reason: "another prepared write exists for this document".into(),
                    }
                }
                other => sql_error(other),
            })?;
        Ok(Box::new(SqlitePreparedWrite {
            repository: self.clone(),
            request,
            before,
            committed: false,
            finished: false,
        }))
    }

    fn recover(&self) -> Result<(), ConfigError> {
        self.connection
            .lock()
            .execute(
                "DELETE FROM config_pending WHERE namespace = ?1",
                params![&*self.namespace],
            )
            .map(drop)
            .map_err(sql_error)
    }
}

fn delete_pending(
    connection: &Connection,
    namespace: &str,
    key: &ConfigDocumentKey,
) -> Result<(), ConfigError> {
    connection
        .execute(
            "DELETE FROM config_pending
             WHERE namespace = ?1 AND provider_id = ?2 AND context_key = ?3",
            params![namespace, key.provider_id, key.context.storage_key()],
        )
        .map(drop)
        .map_err(sql_error)
}

fn sql_error(error: rusqlite::Error) -> ConfigError {
    persistence(error.to_string())
}

fn json_error(error: serde_json::Error) -> ConfigError {
    persistence(error.to_string())
}

fn persistence(reason: impl Into<String>) -> ConfigError {
    ConfigError::PersistenceFailed {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_config_service::{ConfigContext, ConfigValue};

    fn request(provider: &str, expected: u64, value: &str) -> ConfigCompareAndSetRequest {
        ConfigCompareAndSetRequest {
            key: ConfigDocumentKey::new(provider, ConfigContext::global()),
            expected_revision: ConfigRevision(expected),
            value: ConfigValue::String(value.into()),
            schema_version: 1,
            value_version: 1,
        }
    }

    #[test]
    fn documents_have_independent_cas_revisions() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let repository = SqliteConfigRepository::open(file.path(), "product").unwrap();
        let mut first = repository
            .prepare_compare_and_set(request("first", 0, "a"))
            .unwrap();
        assert_eq!(first.commit().unwrap().revision, ConfigRevision(1));
        let mut second = repository
            .prepare_compare_and_set(request("second", 0, "b"))
            .unwrap();
        assert_eq!(second.commit().unwrap().revision, ConfigRevision(1));
        assert!(matches!(
            repository.prepare_compare_and_set(request("first", 0, "stale")),
            Err(ConfigError::RevisionConflict { current: 1, .. })
        ));
    }

    #[test]
    fn pending_recovery_and_rollback_preserve_committed_document() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let repository = SqliteConfigRepository::open(file.path(), "product").unwrap();
        let mut initial = repository
            .prepare_compare_and_set(request("flow", 0, "old"))
            .unwrap();
        initial.commit().unwrap();

        let pending = repository
            .prepare_compare_and_set(request("flow", 1, "new"))
            .unwrap();
        std::mem::forget(pending);
        let reopened = SqliteConfigRepository::open(file.path(), "product").unwrap();
        reopened.recover().unwrap();
        assert_eq!(
            reopened
                .read(&request("flow", 0, "").key)
                .unwrap()
                .unwrap()
                .value,
            ConfigValue::String("old".into())
        );
    }
}
