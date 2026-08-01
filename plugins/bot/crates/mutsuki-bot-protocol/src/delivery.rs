use serde::{Deserialize, Serialize};

use crate::{MessageSegment, QqConversationRef};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotDeliveryContent {
    pub segments: Vec<MessageSegment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
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
    Retry {
        delivery_id: String,
        now_unix_ms: u64,
    },
    Cancel {
        delivery_id: String,
    },
}
