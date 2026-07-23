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
    Error {
        code: String,
        message: String,
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
    pub fn encode(&self) -> ProtocolResult<Vec<u8>> {
        serde_json::to_vec(self).map_err(|err| ProtocolError::InvalidMessage(err.to_string()))
    }

    pub fn decode(bytes: &[u8]) -> ProtocolResult<Self> {
        serde_json::from_slice(bytes).map_err(|err| ProtocolError::InvalidMessage(err.to_string()))
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
        };
        let encoded = message.encode().expect("encode");
        let text = String::from_utf8(encoded.clone()).expect("utf8");
        assert!(!text.contains("token="));
        let decoded = WireMessage::decode(&encoded).expect("decode");
        assert_eq!(decoded, message);
    }
}
