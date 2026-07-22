//! Transport-agnostic typed capability request contracts.
//!
//! These types describe request identity, capability negotiation, receipts and
//! idempotent replay. They do not know about OS processes, named pipes, UDS,
//! Tauri windows or product-specific payloads.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

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

/// Minimal in-memory idempotent receipt store for hosts and tests.
#[derive(Clone, Debug, Default)]
pub struct IdempotentReceiptStore {
    receipts: BTreeMap<CapabilityRequestId, DeliveryReceipt>,
}

impl IdempotentReceiptStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the first receipt for a request id. Replays return a `Duplicate`.
    pub fn accept_or_duplicate(
        &mut self,
        request_id: impl Into<CapabilityRequestId>,
        receipt: DeliveryReceipt,
    ) -> DeliveryReceipt {
        let request_id = request_id.into();
        if let Some(previous) = self.receipts.get(&request_id) {
            return DeliveryReceipt::Duplicate {
                request_id,
                previous: Box::new(previous.clone()),
            };
        }
        self.receipts.insert(request_id, receipt.clone());
        receipt
    }

    pub fn get(&self, request_id: &str) -> Option<&DeliveryReceipt> {
        self.receipts.get(request_id)
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
}
