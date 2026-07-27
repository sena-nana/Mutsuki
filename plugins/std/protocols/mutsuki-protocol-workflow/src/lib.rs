use serde_json::{Value, json};

pub const VERSION: &str = "0.1.0";
pub const ABI_CODEC: &str = "serde-json";
pub const LINEAR_RUN: &str = "mutsuki.workflow.linear.run";
pub const BROADCAST_EMIT: &str = "mutsuki.workflow.broadcast.emit";
pub const PROTOCOL_IDS: &[&str] = &[LINEAR_RUN, BROADCAST_EMIT];

pub fn input_schema(protocol_id: &str) -> Option<Value> {
    match protocol_id {
        LINEAR_RUN => Some(json!({"type": "object", "required": ["steps"]})),
        BROADCAST_EMIT => Some(json!({"type": "object", "required": ["targets"]})),
        _ => None,
    }
}

pub fn output_schema(protocol_id: &str) -> Option<Value> {
    PROTOCOL_IDS
        .contains(&protocol_id)
        .then(|| json!({"type": "object"}))
}

pub fn error_schema(protocol_id: &str) -> Option<Value> {
    output_schema(protocol_id)
}
