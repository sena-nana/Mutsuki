//! Revision-changed watch hub for CLI / Web / automation consumers.

use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::provider::ConfigRevision;
use crate::scope::{ConfigContext, ConfigProviderId};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RevisionChangedEvent {
    pub provider_id: ConfigProviderId,
    pub revision: ConfigRevision,
    pub context: ConfigContext,
}

pub type RevisionChangedListener = Arc<dyn Fn(RevisionChangedEvent) + Send + Sync>;

#[derive(Default)]
pub struct ConfigWatchHub {
    listeners: Mutex<Vec<RevisionChangedListener>>,
}

impl ConfigWatchHub {
    pub fn subscribe(&self, listener: RevisionChangedListener) {
        self.listeners.lock().push(listener);
    }

    pub fn notify(&self, event: RevisionChangedEvent) {
        let listeners = self.listeners.lock().clone();
        for listener in listeners {
            listener(event.clone());
        }
    }

    pub fn listener_count(&self) -> usize {
        self.listeners.lock().len()
    }
}
