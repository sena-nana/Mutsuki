use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{ProtocolError, ProtocolResult};

pub type JsonValue = serde_json::Value;

/// Top-level WebSocket wire message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireMessage {
    Hello {
        protocol_version: String,
        #[serde(default)]
        capabilities: Vec<String>,
        #[serde(default)]
        auth_token: Option<String>,
    },
    HelloAck {
        protocol_version: String,
        session: SessionInfo,
    },
    Rpc(RpcRequest),
    RpcResult(RpcResponse),
    Subscribe(EventSubscription),
    Unsubscribe {
        subscription_id: Uuid,
    },
    Event(EventEnvelope),
    /// Server-side failure. `code` is a stable [`crate::ProtocolError::code`]; clients branch on
    /// it, never on `message`. A failure caused by one RPC or subscription names it, so the
    /// client fails exactly that request instead of waiting for its timeout.
    Error {
        code: String,
        message: String,
        #[serde(default)]
        request_id: Option<Uuid>,
        #[serde(default)]
        subscription_id: Option<Uuid>,
    },
    Ping,
    Pong,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: Uuid,
    pub capabilities: Vec<String>,
    pub safe_mode: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcRequest {
    pub id: Uuid,
    pub namespace: String,
    pub method: String,
    #[serde(default)]
    pub params: JsonValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcResponse {
    pub id: Uuid,
    #[serde(default)]
    pub result: Option<JsonValue>,
    #[serde(default)]
    pub error: Option<RpcErrorBody>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventSubscription {
    pub subscription_id: Uuid,
    pub topic: String,
    #[serde(default)]
    pub required_capability: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub subscription_id: Uuid,
    pub topic: String,
    pub sequence: u64,
    pub payload: JsonValue,
}

impl WireMessage {
    /// An uncorrelated error frame.
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.into(),
            message: message.into(),
            request_id: None,
            subscription_id: None,
        }
    }

    /// The RPC id or subscription id a client message carries, for correlating its failure.
    pub fn correlation(&self) -> (Option<Uuid>, Option<Uuid>) {
        match self {
            Self::Rpc(request) => (Some(request.id), None),
            Self::Subscribe(subscription) => (None, Some(subscription.subscription_id)),
            Self::Unsubscribe { subscription_id } => (None, Some(*subscription_id)),
            _ => (None, None),
        }
    }

    pub fn encode(&self) -> ProtocolResult<Vec<u8>> {
        rmp_serde::to_vec_named(self).map_err(|err| ProtocolError::InvalidMessage(err.to_string()))
    }

    pub fn decode(bytes: &[u8]) -> ProtocolResult<Self> {
        rmp_serde::from_slice(bytes).map_err(|err| ProtocolError::InvalidMessage(err.to_string()))
    }

    pub fn payload_size(&self) -> usize {
        self.encode().map(|bytes| bytes.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_roundtrip_does_not_embed_raw_token_in_error() {
        let message = WireMessage::Error {
            code: "capability_denied".into(),
            message: "missing extension.read".into(),
            request_id: None,
            subscription_id: Some(Uuid::new_v4()),
        };
        let encoded = message.encode().expect("encode");
        assert_ne!(encoded.first().copied(), Some(b'{'));
        let decoded = WireMessage::decode(&encoded).expect("decode");
        assert_eq!(decoded, message);
    }

    #[test]
    fn error_frames_from_older_hosts_decode_without_correlation() {
        #[derive(Serialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum Legacy {
            Error { code: String, message: String },
        }
        let bytes = rmp_serde::to_vec_named(&Legacy::Error {
            code: "invalid_message".into(),
            message: "bad".into(),
        })
        .unwrap();
        assert_eq!(
            WireMessage::decode(&bytes).unwrap(),
            WireMessage::error("invalid_message", "bad")
        );
    }

    #[test]
    fn correlation_names_the_failed_request_or_subscription() {
        let id = Uuid::new_v4();
        let rpc = WireMessage::Rpc(RpcRequest {
            id,
            namespace: "host".into(),
            method: "status".into(),
            params: JsonValue::Null,
        });
        assert_eq!(rpc.correlation(), (Some(id), None));
        let subscribe = WireMessage::Subscribe(EventSubscription {
            subscription_id: id,
            topic: "t".into(),
            required_capability: None,
        });
        assert_eq!(subscribe.correlation(), (None, Some(id)));
        assert_eq!(WireMessage::Ping.correlation(), (None, None));
    }

    #[test]
    fn wire_decode_rejects_text_json_frames() {
        assert!(WireMessage::decode(br#"{"type":"ping"}"#).is_err());
    }
}
