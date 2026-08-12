//! SQLite-backed configuration repository.
//!
//! The caller supplies both path and logical namespace. This crate never
//! derives product directories or installs itself as a framework default.

use std::path::{Path, PathBuf};
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
    /// Opens the repository and recovers any incomplete coordinated writes.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when the database cannot be opened, migrated, or recovered.
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
                   before_json TEXT,
                   commit_marker TEXT,
                   PRIMARY KEY(namespace, provider_id, context_key)
                 );",
            )
            .map_err(sql_error)?;
        ensure_pending_column(&connection, "before_json", "TEXT")?;
        ensure_pending_column(&connection, "commit_marker", "TEXT")?;
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
        let revision = connection
            .query_row(
                "SELECT revision FROM config_documents
                 WHERE namespace = ?1 AND provider_id = ?2 AND context_key = ?3",
                params![namespace, key.provider_id, key.context.storage_key()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(sql_error)?;
        revision
            .map(revision_from_sql)
            .transpose()
            .map(|revision| revision.unwrap_or(ConfigRevision::ABSENT))
    }
}

struct SqlitePreparedWrite {
    repository: SqliteConfigRepository,
    request: ConfigCompareAndSetRequest,
    before: Option<ConfigDocumentSnapshot>,
    committed: bool,
    finished: bool,
    commit_marker: Option<PathBuf>,
}

impl PreparedConfigWrite for SqlitePreparedWrite {
    fn set_commit_marker(&mut self, marker: Option<&Path>) -> Result<(), ConfigError> {
        self.commit_marker = marker.map(Path::to_path_buf);
        self.repository
            .connection
            .lock()
            .execute(
                "UPDATE config_pending SET commit_marker = ?4
                 WHERE namespace = ?1 AND provider_id = ?2 AND context_key = ?3",
                params![
                    &*self.repository.namespace,
                    self.request.key.provider_id,
                    self.request.key.context.storage_key(),
                    self.commit_marker
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned()),
                ],
            )
            .map(drop)
            .map_err(sql_error)
    }

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
                    revision_to_sql(next)?,
                    self.request.schema_version,
                    self.request.value_version,
                    value_json,
                ],
            )
            .map_err(sql_error)?;
        if self.commit_marker.is_none() {
            delete_pending(&transaction, &self.repository.namespace, &self.request.key)?;
        }
        transaction.commit().map_err(sql_error)?;
        self.committed = true;
        self.finished = self.commit_marker.is_none();
        Ok(ConfigDocumentSnapshot {
            key: self.request.key.clone(),
            value: self.request.value.clone(),
            revision: next,
            schema_version: self.request.schema_version,
            value_version: self.request.value_version,
            source: ConfigSource::Persisted,
        })
    }

    fn finish(&mut self) -> Result<(), ConfigError> {
        self.repository
            .connection
            .lock()
            .execute(
                "DELETE FROM config_pending
                 WHERE namespace = ?1 AND provider_id = ?2 AND context_key = ?3",
                params![
                    &*self.repository.namespace,
                    self.request.key.provider_id,
                    self.request.key.context.storage_key(),
                ],
            )
            .map_err(sql_error)?;
        if let Some(marker) = &self.commit_marker {
            remove_commit_marker(marker)?;
        }
        self.finished = true;
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), ConfigError> {
        if self.commit_marker.as_deref().is_some_and(Path::exists) {
            return self.finish();
        }
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
                                revision_to_sql(before.revision)?,
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
        if let Some(marker) = &self.commit_marker {
            remove_commit_marker(marker)?;
        }
        self.finished = true;
        Ok(())
    }
}

impl Drop for SqlitePreparedWrite {
    fn drop(&mut self) {
        if !self.finished {
            let committed_decision =
                self.committed && self.commit_marker.as_deref().is_some_and(Path::exists);
            if !committed_decision {
                let _ = self.rollback();
            }
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
                        revision: revision_from_sql(revision)?,
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
        let before_json = before
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(json_error)?;
        self.connection
            .lock()
            .execute(
                "INSERT INTO config_pending
                   (namespace, provider_id, context_key, expected_revision, request_json,
                    before_json, commit_marker)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
                params![
                    &*self.namespace,
                    request.key.provider_id,
                    request.key.context.storage_key(),
                    revision_to_sql(request.expected_revision)?,
                    request_json,
                    before_json,
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
            commit_marker: None,
        }))
    }

    fn recover(&self) -> Result<(), ConfigError> {
        let mut connection = self.connection.lock();
        let pending = {
            let mut statement = connection
                .prepare(
                    "SELECT request_json, before_json, commit_marker FROM config_pending
                     WHERE namespace = ?1",
                )
                .map_err(sql_error)?;
            let rows = statement
                .query_map(params![&*self.namespace], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })
                .map_err(sql_error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)?
        };
        let transaction = connection.transaction().map_err(sql_error)?;
        let mut markers = Vec::new();
        for (request_json, before_json, marker) in pending {
            let request: ConfigCompareAndSetRequest =
                serde_json::from_str(&request_json).map_err(json_error)?;
            let marker = marker.map(PathBuf::from);
            let committed = marker.as_deref().is_some_and(Path::exists);
            if !committed {
                let before = before_json
                    .map(|value| serde_json::from_str::<ConfigDocumentSnapshot>(&value))
                    .transpose()
                    .map_err(json_error)?;
                let current = SqliteConfigRepository::current_revision(
                    &transaction,
                    &self.namespace,
                    &request.key,
                )?;
                if before.is_some() || current != request.expected_revision {
                    restore_document(&transaction, &self.namespace, &request.key, before.as_ref())?;
                }
            }
            delete_pending(&transaction, &self.namespace, &request.key)?;
            if let Some(marker) = marker {
                markers.push(marker);
            }
        }
        transaction.commit().map_err(sql_error)?;
        for marker in markers {
            remove_commit_marker(&marker)?;
        }
        Ok(())
    }
}

fn ensure_pending_column(
    connection: &Connection,
    column: &str,
    definition: &str,
) -> Result<(), ConfigError> {
    let exists = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('config_pending') WHERE name = ?1)",
            params![column],
            |row| row.get::<_, bool>(0),
        )
        .map_err(sql_error)?;
    if !exists {
        connection
            .execute_batch(&format!(
                "ALTER TABLE config_pending ADD COLUMN {column} {definition};"
            ))
            .map_err(sql_error)?;
    }
    Ok(())
}

fn restore_document(
    connection: &Connection,
    namespace: &str,
    key: &ConfigDocumentKey,
    before: Option<&ConfigDocumentSnapshot>,
) -> Result<(), ConfigError> {
    match before {
        Some(before) => connection
            .execute(
                "INSERT INTO config_documents
                   (namespace, provider_id, context_key, context_json, revision,
                    schema_version, value_version, value_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(namespace, provider_id, context_key) DO UPDATE SET
                   context_json=excluded.context_json, revision=excluded.revision,
                   schema_version=excluded.schema_version, value_version=excluded.value_version,
                   value_json=excluded.value_json",
                params![
                    namespace,
                    before.key.provider_id,
                    before.key.context.storage_key(),
                    serde_json::to_string(&before.key.context).map_err(json_error)?,
                    revision_to_sql(before.revision)?,
                    before.schema_version,
                    before.value_version,
                    serde_json::to_string(&before.value).map_err(json_error)?,
                ],
            )
            .map(drop)
            .map_err(sql_error),
        None => connection
            .execute(
                "DELETE FROM config_documents
                 WHERE namespace = ?1 AND provider_id = ?2 AND context_key = ?3",
                params![namespace, key.provider_id, key.context.storage_key()],
            )
            .map(drop)
            .map_err(sql_error),
    }
}

fn remove_commit_marker(path: &Path) -> Result<(), ConfigError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(persistence(format!(
            "failed to remove commit marker {}: {error}",
            path.display()
        ))),
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

fn sql_error(error: impl std::fmt::Display) -> ConfigError {
    persistence(error.to_string())
}

fn json_error(error: impl std::fmt::Display) -> ConfigError {
    persistence(error.to_string())
}

fn revision_to_sql(revision: ConfigRevision) -> Result<i64, ConfigError> {
    i64::try_from(revision.0)
        .map_err(|_| persistence(format!("revision {} exceeds SQLite INTEGER", revision.0)))
}

fn revision_from_sql(revision: i64) -> Result<ConfigRevision, ConfigError> {
    u64::try_from(revision)
        .map(ConfigRevision)
        .map_err(|_| persistence(format!("stored revision {revision} is negative")))
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

    #[test]
    fn coordinated_recovery_uses_the_shared_commit_marker() {
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("config.sqlite3");
        let repository = SqliteConfigRepository::open(&database, "product").unwrap();
        let mut initial = repository
            .prepare_compare_and_set(request("flow", 0, "old"))
            .unwrap();
        initial.commit().unwrap();

        let rollback_marker = root.path().join("rollback.commit");
        let mut rollback = repository
            .prepare_compare_and_set(request("flow", 1, "uncommitted"))
            .unwrap();
        rollback.set_commit_marker(Some(&rollback_marker)).unwrap();
        rollback.commit().unwrap();
        std::mem::forget(rollback);
        let reopened = SqliteConfigRepository::open(&database, "product").unwrap();
        reopened.recover().unwrap();
        assert_eq!(
            reopened
                .read(&request("flow", 0, "").key)
                .unwrap()
                .unwrap()
                .value,
            ConfigValue::String("old".into())
        );

        let commit_marker = root.path().join("forward.commit");
        let mut forward = reopened
            .prepare_compare_and_set(request("flow", 1, "committed"))
            .unwrap();
        forward.set_commit_marker(Some(&commit_marker)).unwrap();
        forward.commit().unwrap();
        std::fs::write(&commit_marker, b"committed\n").unwrap();
        std::mem::forget(forward);
        let recovered = SqliteConfigRepository::open(&database, "product").unwrap();
        recovered.recover().unwrap();
        assert_eq!(
            recovered
                .read(&request("flow", 0, "").key)
                .unwrap()
                .unwrap()
                .value,
            ConfigValue::String("committed".into())
        );
        assert!(!commit_marker.exists());
    }
}
