use serde_json::{Value, json};

pub const VERSION: &str = "0.1.0";
pub const ABI_CODEC: &str = "serde-json";
pub const OPEN: &str = "mutsuki.db.open";
pub const QUERY: &str = "mutsuki.db.query";
pub const EXECUTE: &str = "mutsuki.db.execute";
pub const TRANSACTION: &str = "mutsuki.db.transaction";
pub const CLOSE: &str = "mutsuki.db.close";
pub const PROTOCOL_IDS: &[&str] = &[OPEN, QUERY, EXECUTE, TRANSACTION, CLOSE];

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
