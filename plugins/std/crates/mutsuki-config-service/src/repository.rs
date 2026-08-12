use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::{
    ConfigCompareAndSetRequest, ConfigDocumentKey, ConfigDocumentSnapshot, ConfigError,
    ConfigRevision, ConfigSource,
};

pub trait PreparedConfigWrite: Send {
    fn set_commit_marker(&mut self, marker: Option<&Path>) -> Result<(), ConfigError> {
        if marker.is_some() {
            return Err(ConfigError::PersistenceFailed {
                reason: "repository does not support coordinated recovery".into(),
            });
        }
        Ok(())
    }
    fn commit(&mut self) -> Result<ConfigDocumentSnapshot, ConfigError>;
    fn finish(&mut self) -> Result<(), ConfigError> {
        Ok(())
    }
    fn rollback(&mut self) -> Result<(), ConfigError>;
}

pub trait ConfigRepository: Send + Sync {
    fn read(&self, key: &ConfigDocumentKey) -> Result<Option<ConfigDocumentSnapshot>, ConfigError>;

    /// Persist a durable pending record after checking the expected revision.
    fn prepare_compare_and_set(
        &self,
        request: ConfigCompareAndSetRequest,
    ) -> Result<Box<dyn PreparedConfigWrite>, ConfigError>;

    /// Recover interrupted writes while preserving the last committed document.
    fn recover(&self) -> Result<(), ConfigError>;
}

#[derive(Default)]
pub struct InMemoryConfigRepository {
    state: Arc<Mutex<MemoryState>>,
}

#[derive(Default)]
struct MemoryState {
    documents: HashMap<ConfigDocumentKey, ConfigDocumentSnapshot>,
    pending: HashMap<ConfigDocumentKey, MemoryPendingWrite>,
}

struct MemoryPendingWrite {
    before: Option<ConfigDocumentSnapshot>,
    commit_marker: Option<PathBuf>,
}

struct MemoryPreparedWrite {
    state: Arc<Mutex<MemoryState>>,
    request: ConfigCompareAndSetRequest,
    before: Option<ConfigDocumentSnapshot>,
    committed: bool,
    finished: bool,
    commit_marker: Option<PathBuf>,
}

impl PreparedConfigWrite for MemoryPreparedWrite {
    fn set_commit_marker(&mut self, marker: Option<&Path>) -> Result<(), ConfigError> {
        self.commit_marker = marker.map(Path::to_path_buf);
        if let Some(pending) = self.state.lock().pending.get_mut(&self.request.key) {
            pending.commit_marker = self.commit_marker.clone();
        }
        Ok(())
    }

    fn commit(&mut self) -> Result<ConfigDocumentSnapshot, ConfigError> {
        let mut state = self.state.lock();
        let current = state
            .documents
            .get(&self.request.key)
            .map_or(ConfigRevision::ABSENT, |snapshot| snapshot.revision);
        if current != self.request.expected_revision {
            return Err(ConfigError::RevisionConflict {
                expected: self.request.expected_revision.0,
                current: current.0,
                diff: None,
            });
        }
        let snapshot = ConfigDocumentSnapshot {
            key: self.request.key.clone(),
            value: self.request.value.clone(),
            revision: current.next(),
            schema_version: self.request.schema_version,
            value_version: self.request.value_version,
            source: ConfigSource::Persisted,
        };
        state
            .documents
            .insert(snapshot.key.clone(), snapshot.clone());
        if self.commit_marker.is_none() {
            state.pending.remove(&snapshot.key);
            self.finished = true;
        }
        self.committed = true;
        Ok(snapshot)
    }

    fn finish(&mut self) -> Result<(), ConfigError> {
        self.state.lock().pending.remove(&self.request.key);
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
        let mut state = self.state.lock();
        state.pending.remove(&self.request.key);
        if self.committed {
            if let Some(before) = self.before.clone() {
                state.documents.insert(self.request.key.clone(), before);
            } else {
                state.documents.remove(&self.request.key);
            }
        }
        if let Some(marker) = &self.commit_marker {
            remove_commit_marker(marker)?;
        }
        self.finished = true;
        Ok(())
    }
}

impl Drop for MemoryPreparedWrite {
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

impl ConfigRepository for InMemoryConfigRepository {
    fn read(&self, key: &ConfigDocumentKey) -> Result<Option<ConfigDocumentSnapshot>, ConfigError> {
        Ok(self.state.lock().documents.get(key).cloned())
    }

    fn prepare_compare_and_set(
        &self,
        request: ConfigCompareAndSetRequest,
    ) -> Result<Box<dyn PreparedConfigWrite>, ConfigError> {
        let mut state = self.state.lock();
        let before = state.documents.get(&request.key).cloned();
        let current = state
            .documents
            .get(&request.key)
            .map_or(ConfigRevision::ABSENT, |snapshot| snapshot.revision);
        if current != request.expected_revision {
            return Err(ConfigError::RevisionConflict {
                expected: request.expected_revision.0,
                current: current.0,
                diff: None,
            });
        }
        if state
            .pending
            .insert(
                request.key.clone(),
                MemoryPendingWrite {
                    before: before.clone(),
                    commit_marker: None,
                },
            )
            .is_some()
        {
            return Err(ConfigError::ApplyRejected {
                reason: "another prepared write exists for this document".into(),
            });
        }
        drop(state);
        Ok(Box::new(MemoryPreparedWrite {
            state: self.state.clone(),
            request,
            before,
            committed: false,
            finished: false,
            commit_marker: None,
        }))
    }

    fn recover(&self) -> Result<(), ConfigError> {
        let mut state = self.state.lock();
        let pending = std::mem::take(&mut state.pending);
        for (key, pending) in pending {
            let committed = pending.commit_marker.as_deref().is_some_and(Path::exists);
            if !committed {
                if let Some(before) = pending.before {
                    state.documents.insert(key, before);
                } else {
                    state.documents.remove(&key);
                }
            }
            if let Some(marker) = pending.commit_marker {
                remove_commit_marker(&marker)?;
            }
        }
        Ok(())
    }
}

fn remove_commit_marker(path: &Path) -> Result<(), ConfigError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ConfigError::PersistenceFailed {
            reason: format!("failed to remove commit marker {}: {error}", path.display()),
        }),
    }
}
