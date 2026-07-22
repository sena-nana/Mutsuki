//! Transport-agnostic typed capability request contracts.
//!
//! These types describe request identity, capability negotiation, receipts and
//! idempotent replay. They do not know about OS processes, named pipes, UDS,
//! Tauri windows or product-specific payloads.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Stable request identity used for correlation and idempotent replay.
pub type CapabilityRequestId = String;

/// Opaque peer identity string owned by the host (for example an AppId).
pub type CapabilityPeerId = String;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDescriptor {
    pub name: String,
    pub protocol_version: u32,
    pub schema_version: u32,
}

impl CapabilityDescriptor {
    pub fn new(name: impl Into<String>, protocol_version: u32, schema_version: u32) -> Self {
        Self {
            name: name.into(),
            protocol_version,
            schema_version,
        }
    }

    pub fn is_compatible_with(&self, offered: &Self) -> bool {
        self.name == offered.name
            && self.protocol_version == offered.protocol_version
            && self.schema_version == offered.schema_version
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapabilityRequestEnvelope {
    pub request_id: CapabilityRequestId,
    pub source: CapabilityPeerId,
    pub target: CapabilityPeerId,
    pub capability: CapabilityDescriptor,
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_unix_ms: Option<u64>,
}

impl CapabilityRequestEnvelope {
    pub fn new(
        request_id: impl Into<CapabilityRequestId>,
        source: impl Into<CapabilityPeerId>,
        target: impl Into<CapabilityPeerId>,
        capability: CapabilityDescriptor,
        payload: Value,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            source: source.into(),
            target: target.into(),
            capability,
            payload,
            deadline_unix_ms: None,
        }
    }

    pub fn with_deadline(mut self, deadline: SystemTime) -> Self {
        self.deadline_unix_ms = Some(system_time_to_unix_ms(deadline));
        self
    }

    pub fn is_expired_at(&self, now: SystemTime) -> bool {
        self.deadline_unix_ms
            .is_some_and(|deadline| system_time_to_unix_ms(now) >= deadline)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionReason {
    CapabilityUnavailable,
    ProtocolIncompatible,
    PermissionDenied,
    PayloadInvalid,
    DeadlineExceeded,
    Cancelled,
    Other { code: String, message: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeliveryReceipt {
    Accepted {
        request_id: CapabilityRequestId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote_task_id: Option<String>,
    },
    Duplicate {
        request_id: CapabilityRequestId,
        previous: Box<DeliveryReceipt>,
    },
    Rejected {
        request_id: CapabilityRequestId,
        reason: RejectionReason,
    },
    Completed {
        request_id: CapabilityRequestId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote_task_id: Option<String>,
        #[serde(default)]
        output: Value,
    },
    Failed {
        request_id: CapabilityRequestId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote_task_id: Option<String>,
        code: String,
        message: String,
    },
}

impl DeliveryReceipt {
    pub fn request_id(&self) -> &str {
        match self {
            Self::Accepted { request_id, .. }
            | Self::Duplicate { request_id, .. }
            | Self::Rejected { request_id, .. }
            | Self::Completed { request_id, .. }
            | Self::Failed { request_id, .. } => request_id,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Rejected { .. } | Self::Completed { .. } | Self::Failed { .. }
        )
    }
}

/// Retention limits for in-memory idempotent receipts.
///
/// Missing limits are unbounded on that dimension. Eviction follows completion
/// order (oldest finished receipt first) and never rescans the whole map on the
/// hot path beyond queue pops.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptRetentionPolicy {
    pub max_entries: Option<usize>,
    pub max_bytes: Option<usize>,
    pub ttl: Option<Duration>,
}

impl ReceiptRetentionPolicy {
    pub const UNBOUNDED: Self = Self {
        max_entries: None,
        max_bytes: None,
        ttl: None,
    };

    /// Desktop-oriented defaults: 10_000 entries, 16 MiB, 1 hour TTL.
    pub fn desktop_default() -> Self {
        Self {
            max_entries: Some(10_000),
            max_bytes: Some(16 * 1024 * 1024),
            ttl: Some(Duration::from_secs(60 * 60)),
        }
    }

    pub fn is_unbounded(&self) -> bool {
        self.max_entries.is_none() && self.max_bytes.is_none() && self.ttl.is_none()
    }
}

impl Default for ReceiptRetentionPolicy {
    fn default() -> Self {
        Self::UNBOUNDED
    }
}

/// Snapshot of receipt-store occupancy and eviction counters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReceiptStoreStats {
    pub entries: usize,
    pub estimated_bytes: usize,
    pub evictions: u64,
    pub oldest_age: Option<Duration>,
}

#[derive(Clone, Debug)]
struct StoredReceipt {
    receipt: DeliveryReceipt,
    completed_at: Instant,
    estimated_bytes: usize,
}

/// Minimal in-memory idempotent receipt store for hosts and tests.
///
/// Evicted or TTL-expired request IDs are treated as never seen (`TreatAsNew`):
/// `get` returns `None` and a later `accept_or_duplicate` records a fresh first
/// receipt. Within the retention window the first receipt still wins.
#[derive(Clone, Debug)]
pub struct IdempotentReceiptStore {
    receipts: BTreeMap<CapabilityRequestId, StoredReceipt>,
    order: VecDeque<CapabilityRequestId>,
    policy: ReceiptRetentionPolicy,
    estimated_bytes: usize,
    evictions: u64,
}

impl Default for IdempotentReceiptStore {
    fn default() -> Self {
        Self::new()
    }
}

impl IdempotentReceiptStore {
    /// Unbounded store for light tests. Production hosts should use
    /// [`Self::with_policy`] / [`ReceiptRetentionPolicy::desktop_default`].
    pub fn new() -> Self {
        Self::with_policy(ReceiptRetentionPolicy::UNBOUNDED)
    }

    pub fn with_policy(policy: ReceiptRetentionPolicy) -> Self {
        Self {
            receipts: BTreeMap::new(),
            order: VecDeque::new(),
            policy,
            estimated_bytes: 0,
            evictions: 0,
        }
    }

    pub fn policy(&self) -> &ReceiptRetentionPolicy {
        &self.policy
    }

    /// Record the first receipt for a request id. Replays return a `Duplicate`.
    pub fn accept_or_duplicate(
        &mut self,
        request_id: impl Into<CapabilityRequestId>,
        receipt: DeliveryReceipt,
    ) -> DeliveryReceipt {
        self.accept_or_duplicate_at(request_id, receipt, Instant::now())
    }

    pub fn accept_or_duplicate_at(
        &mut self,
        request_id: impl Into<CapabilityRequestId>,
        receipt: DeliveryReceipt,
        now: Instant,
    ) -> DeliveryReceipt {
        let request_id = request_id.into();
        self.expire_due(now);
        if let Some(previous) = self.receipts.get(&request_id) {
            return DeliveryReceipt::Duplicate {
                request_id,
                previous: Box::new(previous.receipt.clone()),
            };
        }
        let estimated_bytes = estimate_receipt_bytes(&request_id, &receipt);
        self.receipts.insert(
            request_id.clone(),
            StoredReceipt {
                receipt: receipt.clone(),
                completed_at: now,
                estimated_bytes,
            },
        );
        self.order.push_back(request_id);
        self.estimated_bytes = self.estimated_bytes.saturating_add(estimated_bytes);
        self.enforce_budget(now);
        receipt
    }

    pub fn get(&self, request_id: &str) -> Option<&DeliveryReceipt> {
        self.get_at(request_id, Instant::now())
    }

    pub fn get_at(&self, request_id: &str, now: Instant) -> Option<&DeliveryReceipt> {
        let entry = self.receipts.get(request_id)?;
        if self.is_expired(entry, now) {
            return None;
        }
        Some(&entry.receipt)
    }

    /// Drop TTL-expired entries, then return a live receipt if present.
    pub fn take_live(&mut self, request_id: &str, now: Instant) -> Option<DeliveryReceipt> {
        self.expire_due(now);
        self.receipts
            .get(request_id)
            .map(|entry| entry.receipt.clone())
    }

    pub fn len(&self) -> usize {
        self.receipts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.receipts.is_empty()
    }

    pub fn stats(&self) -> ReceiptStoreStats {
        self.stats_at(Instant::now())
    }

    pub fn stats_at(&self, now: Instant) -> ReceiptStoreStats {
        let oldest_age = self
            .order
            .front()
            .and_then(|id| self.receipts.get(id))
            .map(|entry| now.saturating_duration_since(entry.completed_at));
        ReceiptStoreStats {
            entries: self.receipts.len(),
            estimated_bytes: self.estimated_bytes,
            evictions: self.evictions,
            oldest_age,
        }
    }

    fn is_expired(&self, entry: &StoredReceipt, now: Instant) -> bool {
        self.policy
            .ttl
            .is_some_and(|ttl| now.saturating_duration_since(entry.completed_at) >= ttl)
    }

    fn expire_due(&mut self, now: Instant) {
        let Some(ttl) = self.policy.ttl else {
            return;
        };
        while let Some(front_id) = self.order.front().cloned() {
            let Some(entry) = self.receipts.get(&front_id) else {
                self.order.pop_front();
                continue;
            };
            if now.saturating_duration_since(entry.completed_at) < ttl {
                break;
            }
            self.evict_front();
        }
    }

    fn enforce_budget(&mut self, now: Instant) {
        self.expire_due(now);
        while self.over_budget() {
            if !self.evict_front() {
                break;
            }
        }
    }

    fn over_budget(&self) -> bool {
        if self
            .policy
            .max_entries
            .is_some_and(|limit| self.receipts.len() > limit)
        {
            return true;
        }
        if self
            .policy
            .max_bytes
            .is_some_and(|limit| self.estimated_bytes > limit)
        {
            return true;
        }
        false
    }

    fn evict_front(&mut self) -> bool {
        let Some(request_id) = self.order.pop_front() else {
            return false;
        };
        if let Some(entry) = self.receipts.remove(&request_id) {
            self.estimated_bytes = self.estimated_bytes.saturating_sub(entry.estimated_bytes);
            self.evictions = self.evictions.saturating_add(1);
            true
        } else {
            true
        }
    }
}

fn estimate_receipt_bytes(request_id: &str, receipt: &DeliveryReceipt) -> usize {
    const TAG_OVERHEAD: usize = 32;
    let body = match receipt {
        DeliveryReceipt::Accepted { remote_task_id, .. } => {
            remote_task_id.as_ref().map_or(0, String::len)
        }
        DeliveryReceipt::Duplicate { previous, .. } => {
            estimate_receipt_bytes(previous.request_id(), previous)
        }
        DeliveryReceipt::Rejected { reason, .. } => estimate_rejection_bytes(reason),
        DeliveryReceipt::Completed {
            remote_task_id,
            output,
            ..
        } => remote_task_id.as_ref().map_or(0, String::len) + estimate_json_bytes(output),
        DeliveryReceipt::Failed {
            remote_task_id,
            code,
            message,
            ..
        } => remote_task_id.as_ref().map_or(0, String::len) + code.len() + message.len(),
    };
    request_id
        .len()
        .saturating_add(TAG_OVERHEAD)
        .saturating_add(body)
}

fn estimate_rejection_bytes(reason: &RejectionReason) -> usize {
    match reason {
        RejectionReason::Other { code, message } => code.len() + message.len() + 16,
        _ => 16,
    }
}

fn estimate_json_bytes(value: &Value) -> usize {
    match value {
        Value::Null | Value::Bool(_) => 8,
        Value::Number(number) => number.to_string().len(),
        Value::String(text) => text.len(),
        Value::Array(items) => items.iter().map(estimate_json_bytes).sum::<usize>() + 8,
        Value::Object(map) => {
            map.iter()
                .map(|(key, item)| key.len() + estimate_json_bytes(item))
                .sum::<usize>()
                + 8
        }
    }
}

fn system_time_to_unix_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn envelope_and_receipt_round_trip() {
        let envelope = CapabilityRequestEnvelope::new(
            "req-1",
            "source.app",
            "target.app",
            CapabilityDescriptor::new("demo.capability", 1, 1),
            json!({"ok": true}),
        );
        let encoded = serde_json::to_string(&envelope).unwrap();
        let decoded: CapabilityRequestEnvelope = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, envelope);

        let receipt = DeliveryReceipt::Accepted {
            request_id: "req-1".into(),
            remote_task_id: Some("task-1".into()),
        };
        let encoded = serde_json::to_string(&receipt).unwrap();
        let decoded: DeliveryReceipt = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, receipt);
    }

    #[test]
    fn idempotent_store_returns_duplicate_on_replay() {
        let mut store = IdempotentReceiptStore::new();
        let first = store.accept_or_duplicate(
            "req-1",
            DeliveryReceipt::Accepted {
                request_id: "req-1".into(),
                remote_task_id: Some("task-1".into()),
            },
        );
        assert!(matches!(first, DeliveryReceipt::Accepted { .. }));
        let second = store.accept_or_duplicate(
            "req-1",
            DeliveryReceipt::Accepted {
                request_id: "req-1".into(),
                remote_task_id: Some("task-2".into()),
            },
        );
        match second {
            DeliveryReceipt::Duplicate { previous, .. } => {
                assert_eq!(previous.request_id(), "req-1");
                assert!(matches!(
                    *previous,
                    DeliveryReceipt::Accepted {
                        remote_task_id: Some(ref id),
                        ..
                    } if id == "task-1"
                ));
            }
            other => panic!("expected duplicate, got {other:?}"),
        }
    }

    #[test]
    fn capability_compatibility_is_exact() {
        let required = CapabilityDescriptor::new("demo.capability", 1, 2);
        assert!(required.is_compatible_with(&CapabilityDescriptor::new("demo.capability", 1, 2)));
        assert!(!required.is_compatible_with(&CapabilityDescriptor::new("demo.capability", 1, 1)));
        assert!(!required.is_compatible_with(&CapabilityDescriptor::new("other", 1, 2)));
    }

    #[test]
    fn bounded_store_evicts_oldest_and_treats_expired_as_new() {
        let policy = ReceiptRetentionPolicy {
            max_entries: Some(2),
            max_bytes: None,
            ttl: None,
        };
        let mut store = IdempotentReceiptStore::with_policy(policy);
        let now = Instant::now();
        for index in 1..=3 {
            let id = format!("req-{index}");
            store.accept_or_duplicate_at(
                id.clone(),
                DeliveryReceipt::Completed {
                    request_id: id,
                    remote_task_id: None,
                    output: json!({"n": index}),
                },
                now,
            );
        }
        let stats = store.stats_at(now);
        assert_eq!(stats.entries, 2);
        assert_eq!(stats.evictions, 1);
        assert!(store.get_at("req-1", now).is_none());
        assert!(store.get_at("req-2", now).is_some());
        assert!(store.get_at("req-3", now).is_some());

        let replay = store.accept_or_duplicate_at(
            "req-1",
            DeliveryReceipt::Accepted {
                request_id: "req-1".into(),
                remote_task_id: Some("fresh".into()),
            },
            now,
        );
        assert!(matches!(
            replay,
            DeliveryReceipt::Accepted {
                remote_task_id: Some(ref id),
                ..
            } if id == "fresh"
        ));
        assert!(matches!(
            store.accept_or_duplicate_at(
                "req-3",
                DeliveryReceipt::Accepted {
                    request_id: "req-3".into(),
                    remote_task_id: Some("other".into()),
                },
                now,
            ),
            DeliveryReceipt::Duplicate { .. }
        ));
    }

    #[test]
    fn bounded_store_counts_large_output_against_byte_budget() {
        let policy = ReceiptRetentionPolicy {
            max_entries: Some(100),
            max_bytes: Some(2_000),
            ttl: None,
        };
        let mut store = IdempotentReceiptStore::with_policy(policy);
        let now = Instant::now();
        for index in 0..10 {
            let id = format!("req-{index}");
            store.accept_or_duplicate_at(
                id.clone(),
                DeliveryReceipt::Completed {
                    request_id: id,
                    remote_task_id: None,
                    output: json!("x".repeat(400)),
                },
                now,
            );
        }
        let stats = store.stats_at(now);
        assert!(stats.entries < 10);
        assert!(stats.estimated_bytes <= 2_000);
        assert!(stats.evictions > 0);
    }

    #[test]
    fn ttl_expiry_drops_receipts_without_full_scan_of_live_window() {
        let policy = ReceiptRetentionPolicy {
            max_entries: None,
            max_bytes: None,
            ttl: Some(Duration::from_secs(10)),
        };
        let mut store = IdempotentReceiptStore::with_policy(policy);
        let start = Instant::now();
        store.accept_or_duplicate_at(
            "old",
            DeliveryReceipt::Accepted {
                request_id: "old".into(),
                remote_task_id: None,
            },
            start,
        );
        store.accept_or_duplicate_at(
            "fresh",
            DeliveryReceipt::Accepted {
                request_id: "fresh".into(),
                remote_task_id: None,
            },
            start + Duration::from_secs(5),
        );
        assert!(
            store
                .get_at("old", start + Duration::from_secs(9))
                .is_some()
        );
        assert!(
            store
                .get_at("old", start + Duration::from_secs(10))
                .is_none()
        );
        // Mutating accept path drops expired front entries without scanning the live window.
        store.accept_or_duplicate_at(
            "probe",
            DeliveryReceipt::Accepted {
                request_id: "probe".into(),
                remote_task_id: None,
            },
            start + Duration::from_secs(10),
        );
        assert!(
            store
                .get_at("old", start + Duration::from_secs(10))
                .is_none()
        );
        assert!(
            store
                .get_at("fresh", start + Duration::from_secs(10))
                .is_some()
        );
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn concurrent_first_receipt_wins_under_retention() {
        use std::sync::{Arc, Mutex};
        use std::thread;

        let store = Arc::new(Mutex::new(IdempotentReceiptStore::with_policy(
            ReceiptRetentionPolicy {
                max_entries: Some(1_000),
                max_bytes: None,
                ttl: None,
            },
        )));
        let mut handles = Vec::new();
        for index in 0..32 {
            let store = store.clone();
            handles.push(thread::spawn(move || {
                let mut guard = store.lock().unwrap();
                guard.accept_or_duplicate(
                    "shared",
                    DeliveryReceipt::Accepted {
                        request_id: "shared".into(),
                        remote_task_id: Some(format!("worker-{index}")),
                    },
                )
            }));
        }
        let mut accepted = 0;
        let mut duplicates = 0;
        let mut winner = None;
        for handle in handles {
            match handle.join().unwrap() {
                DeliveryReceipt::Accepted { remote_task_id, .. } => {
                    accepted += 1;
                    winner = remote_task_id;
                }
                DeliveryReceipt::Duplicate { previous, .. } => {
                    duplicates += 1;
                    assert!(matches!(*previous, DeliveryReceipt::Accepted { .. }));
                }
                other => panic!("unexpected receipt {other:?}"),
            }
        }
        assert_eq!(accepted, 1);
        assert_eq!(duplicates, 31);
        assert!(winner.is_some());
    }
}
