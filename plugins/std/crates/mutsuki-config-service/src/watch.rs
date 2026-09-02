//! Revision-changed watch hub for CLI / Web / automation consumers.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use crate::{ConfigContext, ConfigProviderId, ConfigRevision};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RevisionChangedEvent {
    pub provider_id: ConfigProviderId,
    pub revision: ConfigRevision,
    pub context: ConfigContext,
}

pub type RevisionChangedListener = Arc<dyn Fn(RevisionChangedEvent) + Send + Sync>;

#[derive(Default)]
pub struct ConfigWatchHub {
    listeners: Mutex<BTreeMap<u64, RevisionChangedListener>>,
    next_subscription_id: AtomicU64,
}

impl ConfigWatchHub {
    pub fn subscribe(
        self: &Arc<Self>,
        listener: RevisionChangedListener,
    ) -> ConfigWatchSubscription {
        let subscription_id = self.next_subscription_id.fetch_add(1, Ordering::Relaxed);
        self.listeners
            .lock()
            .unwrap()
            .insert(subscription_id, listener);
        ConfigWatchSubscription {
            hub: Arc::downgrade(self),
            subscription_id,
            disposed: false,
        }
    }

    pub fn notify(&self, event: RevisionChangedEvent) {
        let listeners = self
            .listeners
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for listener in listeners {
            listener(event.clone());
        }
    }
}

pub struct ConfigWatchSubscription {
    hub: Weak<ConfigWatchHub>,
    subscription_id: u64,
    disposed: bool,
}

impl ConfigWatchSubscription {
    /// # Panics
    ///
    /// Panics if the listener registry lock is poisoned.
    pub fn dispose(&mut self) -> bool {
        if self.disposed {
            return false;
        }
        self.disposed = true;
        self.hub.upgrade().is_some_and(|hub| {
            hub.listeners
                .lock()
                .unwrap()
                .remove(&self.subscription_id)
                .is_some()
        })
    }
}

impl Drop for ConfigWatchSubscription {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}
