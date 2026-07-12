use serde_json::{Value, json};

pub const VERSION: &str = "0.1.0";
pub const ABI_CODEC: &str = "serde-json";
pub const PUT: &str = "mutsuki.resource.put";
pub const GET: &str = "mutsuki.resource.get";
pub const OPEN_READ: &str = "mutsuki.resource.open_read";
pub const OPEN_WRITE: &str = "mutsuki.resource.open_write";
pub const CLONE_REF: &str = "mutsuki.resource.clone_ref";
pub const DROP: &str = "mutsuki.resource.drop";
pub const STAT: &str = "mutsuki.resource.stat";
pub const LEASE: &str = "mutsuki.resource.lease";
pub const RELEASE: &str = "mutsuki.resource.release";
pub const PROTOCOL_IDS: &[&str] = &[
    PUT, GET, OPEN_READ, OPEN_WRITE, CLONE_REF, DROP, STAT, LEASE, RELEASE,
];

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
