use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

/// In-process counters for the metrics names defined by Issue #1.
#[derive(Debug, Default)]
pub struct BridgeMetrics {
    connections: AtomicU64,
    sessions: AtomicU64,
    rpc_inflight: AtomicU64,
    rpc_latency_total_ms: AtomicU64,
    rpc_latency_samples: AtomicU64,
    event_subscriptions: AtomicU64,
    event_dropped: AtomicU64,
    extension_loaded: AtomicU64,
    extension_failed: AtomicU64,
    static_cache_bytes: AtomicU64,
    ws_queue_depth: AtomicU64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BridgeMetricsSnapshot {
    pub web_host_connections: u64,
    pub web_host_active_sessions: u64,
    pub web_rpc_inflight: u64,
    pub web_rpc_latency: u64,
    pub web_event_subscriptions: u64,
    pub web_event_dropped: u64,
    pub web_extension_loaded: u64,
    pub web_extension_failed: u64,
    pub web_static_cache_bytes: u64,
    pub web_ws_queue_depth: u64,
}

impl BridgeMetrics {
    pub fn snapshot(&self) -> Self {
        Self {
            connections: AtomicU64::new(self.connections.load(Ordering::Relaxed)),
            sessions: AtomicU64::new(self.sessions.load(Ordering::Relaxed)),
            rpc_inflight: AtomicU64::new(self.rpc_inflight.load(Ordering::Relaxed)),
            rpc_latency_total_ms: AtomicU64::new(self.rpc_latency_total_ms.load(Ordering::Relaxed)),
            rpc_latency_samples: AtomicU64::new(self.rpc_latency_samples.load(Ordering::Relaxed)),
            event_subscriptions: AtomicU64::new(self.event_subscriptions.load(Ordering::Relaxed)),
            event_dropped: AtomicU64::new(self.event_dropped.load(Ordering::Relaxed)),
            extension_loaded: AtomicU64::new(self.extension_loaded.load(Ordering::Relaxed)),
            extension_failed: AtomicU64::new(self.extension_failed.load(Ordering::Relaxed)),
            static_cache_bytes: AtomicU64::new(self.static_cache_bytes.load(Ordering::Relaxed)),
            ws_queue_depth: AtomicU64::new(self.ws_queue_depth.load(Ordering::Relaxed)),
        }
    }

    pub fn inc_sessions(&self) {
        self.sessions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn dec_sessions(&self) {
        self.sessions.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn inc_rpc_inflight(&self) {
        self.rpc_inflight.fetch_add(1, Ordering::Relaxed);
    }

    pub fn dec_rpc_inflight(&self) {
        self.rpc_inflight.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn observe_rpc_latency(&self, ms: u64) {
        self.rpc_latency_total_ms.fetch_add(ms, Ordering::Relaxed);
        self.rpc_latency_samples.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_subscriptions(&self) {
        self.event_subscriptions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_events_dropped(&self) {
        self.event_dropped.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_connections(&self, value: u64) {
        self.connections.store(value, Ordering::Relaxed);
    }

    pub fn set_extension_counts(&self, loaded: u64, failed: u64) {
        self.extension_loaded.store(loaded, Ordering::Relaxed);
        self.extension_failed.store(failed, Ordering::Relaxed);
    }

    pub fn set_static_cache_bytes(&self, value: u64) {
        self.static_cache_bytes.store(value, Ordering::Relaxed);
    }

    pub fn set_ws_queue_depth(&self, value: u64) {
        self.ws_queue_depth.store(value, Ordering::Relaxed);
    }

    pub fn export(&self) -> BridgeMetricsSnapshot {
        let samples = self.rpc_latency_samples.load(Ordering::Relaxed).max(1);
        let latency = self.rpc_latency_total_ms.load(Ordering::Relaxed) / samples;
        BridgeMetricsSnapshot {
            web_host_connections: self.connections.load(Ordering::Relaxed),
            web_host_active_sessions: self.sessions.load(Ordering::Relaxed),
            web_rpc_inflight: self.rpc_inflight.load(Ordering::Relaxed),
            web_rpc_latency: latency,
            web_event_subscriptions: self.event_subscriptions.load(Ordering::Relaxed),
            web_event_dropped: self.event_dropped.load(Ordering::Relaxed),
            web_extension_loaded: self.extension_loaded.load(Ordering::Relaxed),
            web_extension_failed: self.extension_failed.load(Ordering::Relaxed),
            web_static_cache_bytes: self.static_cache_bytes.load(Ordering::Relaxed),
            web_ws_queue_depth: self.ws_queue_depth.load(Ordering::Relaxed),
        }
    }
}

// Use export snapshot for Serialize in bridge host.status/metrics.
impl Serialize for BridgeMetrics {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.export().serialize(serializer)
    }
}
