use serde_json::{Value, json};

pub const VERSION: &str = "0.1.0";
pub const ABI_CODEC: &str = "serde-json";
pub const CONFIG_DESCRIBE: &str = "mutsuki.config.describe";
pub const PERMISSION_CHECK: &str = "mutsuki.permission.check";
pub const PROTOCOL_IDS: &[&str] = &[CONFIG_DESCRIBE, PERMISSION_CHECK];

pub fn input_schema(protocol_id: &str) -> Option<Value> {
    PROTOCOL_IDS
        .contains(&protocol_id)
        .then(|| json!({"type": "object"}))
}

pub fn output_schema(protocol_id: &str) -> Option<Value> {
    input_schema(protocol_id)
}

pub fn error_schema(protocol_id: &str) -> Option<Value> {
    input_schema(protocol_id)
}
