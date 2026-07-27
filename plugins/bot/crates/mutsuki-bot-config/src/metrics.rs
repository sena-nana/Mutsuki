//! Lightweight metrics for config operations.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone)]
pub struct ConfigMetrics {
    inner: Arc<ConfigMetricsInner>,
}

#[derive(Debug, Default)]
struct ConfigMetricsInner {
    provider_count: AtomicU64,
    schema_cache_hit: AtomicU64,
    read_latency_ms_total: AtomicU64,
    read_count: AtomicU64,
    validate_latency_ms_total: AtomicU64,
    validate_count: AtomicU64,
    apply_latency_ms_total: AtomicU64,
    apply_count: AtomicU64,
    revision_conflict: AtomicU64,
    reload_required: AtomicU64,
    apply_failed: AtomicU64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigMetricsSnapshot {
    pub config_provider_count: u64,
    pub config_schema_cache_hit: u64,
    pub config_read_latency_avg_ms: u64,
    pub config_validate_latency_avg_ms: u64,
    pub config_apply_latency_avg_ms: u64,
    pub config_revision_conflict: u64,
    pub config_reload_required: u64,
    pub config_apply_failed: u64,
}

impl ConfigMetrics {
    pub fn set_provider_count(&self, value: u64) {
        self.inner.provider_count.store(value, Ordering::Relaxed);
    }

    pub fn inc_schema_cache_hit(&self) {
        self.inner.schema_cache_hit.fetch_add(1, Ordering::Relaxed);
    }

    pub fn observe_read(&self, ms: u64) {
        self.inner
            .read_latency_ms_total
            .fetch_add(ms, Ordering::Relaxed);
        self.inner.read_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn observe_validate(&self, ms: u64) {
        self.inner
            .validate_latency_ms_total
            .fetch_add(ms, Ordering::Relaxed);
        self.inner.validate_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn observe_apply(&self, ms: u64) {
        self.inner
            .apply_latency_ms_total
            .fetch_add(ms, Ordering::Relaxed);
        self.inner.apply_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_revision_conflict(&self) {
        self.inner.revision_conflict.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_reload_required(&self) {
        self.inner.reload_required.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_apply_failed(&self) {
        self.inner.apply_failed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> ConfigMetricsSnapshot {
        let avg = |total: &AtomicU64, count: &AtomicU64| {
            let c = count.load(Ordering::Relaxed);
            if c == 0 {
                0
            } else {
                total.load(Ordering::Relaxed) / c
            }
        };
        ConfigMetricsSnapshot {
            config_provider_count: self.inner.provider_count.load(Ordering::Relaxed),
            config_schema_cache_hit: self.inner.schema_cache_hit.load(Ordering::Relaxed),
            config_read_latency_avg_ms: avg(
                &self.inner.read_latency_ms_total,
                &self.inner.read_count,
            ),
            config_validate_latency_avg_ms: avg(
                &self.inner.validate_latency_ms_total,
                &self.inner.validate_count,
            ),
            config_apply_latency_avg_ms: avg(
                &self.inner.apply_latency_ms_total,
                &self.inner.apply_count,
            ),
            config_revision_conflict: self.inner.revision_conflict.load(Ordering::Relaxed),
            config_reload_required: self.inner.reload_required.load(Ordering::Relaxed),
            config_apply_failed: self.inner.apply_failed.load(Ordering::Relaxed),
        }
    }
}
