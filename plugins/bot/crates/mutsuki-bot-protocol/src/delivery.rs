use serde::{Deserialize, Serialize};

use crate::{MessageSegment, QqConversationRef};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotDeliveryContent {
    pub segments: Vec<MessageSegment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryPolicy {
    pub max_attempts: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_unix_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotActiveDeliveryRequest {
    pub delivery_id: String,
    pub idempotency_key: String,
    pub conversation: QqConversationRef,
    pub content: BotDeliveryContent,
    pub policy: DeliveryPolicy,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_execution_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Pending,
    Sending,
    Succeeded,
    RetryScheduled,
    PermanentlyFailed,
    Cancelled,
    Previewed,
    /// External send may have completed while local receipt was lost; requires manual reconcile.
    ReconcileRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryPartStatus {
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotDeliveryPartReceipt {
    pub part_index: u32,
    pub status: DeliveryPartStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotDeliveryAttempt {
    pub delivery_id: String,
    pub attempt: u32,
    pub status: DeliveryStatus,
    pub started_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotDeliveryReceipt {
    pub delivery_id: String,
    pub idempotency_key: String,
    pub status: DeliveryStatus,
    pub attempt_count: u32,
    #[serde(default)]
    pub platform_message_ids: Vec<String>,
    #[serde(default)]
    pub part_receipts: Vec<BotDeliveryPartReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// Monotonic claim generation used by CAS lease recovery.
    #[serde(default)]
    pub generation: u64,
    /// Exclusive send lease deadline; expired `Sending` becomes `ReconcileRequired`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_expires_at_unix_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotReplyDeliveryPart {
    pub part_id: String,
    pub content: BotDeliveryContent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotReplyDeliveryRequest {
    pub reply_id: String,
    pub idempotency_key: String,
    pub conversation: QqConversationRef,
    pub parts: Vec<BotReplyDeliveryPart>,
    pub policy: DeliveryPolicy,
    pub source_event_id: String,
    pub source_turn_id: String,
    /// Durable Agent event owner completed atomically with reply reservation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_binding_key: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotReplyDeliveryReceipt {
    pub reply_id: String,
    pub idempotency_key: String,
    pub part_receipts: Vec<BotDeliveryReceipt>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BotActiveDeliveryCommand {
    Submit {
        request: Box<BotActiveDeliveryRequest>,
        now_unix_ms: u64,
    },
    ResumeDue {
        now_unix_ms: u64,
    },
    Inspect {
        delivery_id: String,
    },
    Preview {
        delivery_id: String,
    },
    Retry {
        delivery_id: String,
        now_unix_ms: u64,
    },
    Cancel {
        delivery_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BotReplyDeliveryCommand {
    Reserve {
        request: Box<BotReplyDeliveryRequest>,
    },
    Submit {
        request: Box<BotReplyDeliveryRequest>,
        now_unix_ms: u64,
    },
    ResumeDue {
        now_unix_ms: u64,
    },
    Inspect {
        reply_id: String,
    },
    RetryPart {
        delivery_id: String,
        now_unix_ms: u64,
    },
    CancelPart {
        delivery_id: String,
    },
}
