use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::{
    ConfigCompareAndSetRequest, ConfigDocumentKey, ConfigDocumentSnapshot, ConfigError,
    ConfigRevision, ConfigSource,
};

pub trait PreparedConfigWrite: Send {
    fn commit(&mut self) -> Result<ConfigDocumentSnapshot, ConfigError>;
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
    pending: HashMap<ConfigDocumentKey, ConfigCompareAndSetRequest>,
}

struct MemoryPreparedWrite {
    state: Arc<Mutex<MemoryState>>,
    request: ConfigCompareAndSetRequest,
    before: Option<ConfigDocumentSnapshot>,
    committed: bool,
    finished: bool,
}

impl PreparedConfigWrite for MemoryPreparedWrite {
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
        state.pending.remove(&snapshot.key);
        self.committed = true;
        self.finished = true;
        Ok(snapshot)
    }

    fn rollback(&mut self) -> Result<(), ConfigError> {
        let mut state = self.state.lock();
        state.pending.remove(&self.request.key);
        if self.committed {
            if let Some(before) = self.before.clone() {
                state.documents.insert(self.request.key.clone(), before);
            } else {
                state.documents.remove(&self.request.key);
            }
        }
        self.finished = true;
        Ok(())
    }
}

impl Drop for MemoryPreparedWrite {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.rollback();
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
            .insert(request.key.clone(), request.clone())
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
        }))
    }

    fn recover(&self) -> Result<(), ConfigError> {
        self.state.lock().pending.clear();
        Ok(())
    }
}
