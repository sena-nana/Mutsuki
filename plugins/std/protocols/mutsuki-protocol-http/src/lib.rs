use serde_json::{Value, json};

pub const VERSION: &str = "0.1.0";
pub const ABI_CODEC: &str = "serde-json";
pub const REQUEST: &str = "mutsuki.http.request";
pub const PROTOCOL_IDS: &[&str] = &[REQUEST];

pub fn input_schema(protocol_id: &str) -> Option<Value> {
    (protocol_id == REQUEST)
        .then(|| json!({"type": "object", "required": ["url", "domain_allowlist"]}))
}

pub fn output_schema(protocol_id: &str) -> Option<Value> {
    (protocol_id == REQUEST).then(|| json!({"type": "object"}))
}

pub fn error_schema(protocol_id: &str) -> Option<Value> {
    output_schema(protocol_id)
}
