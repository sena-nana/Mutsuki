use serde_json::{Value, json};

pub const VERSION: &str = "0.1.0";
pub const ABI_CODEC: &str = "serde-json";
pub const READ: &str = "mutsuki.fs.read";
pub const WRITE: &str = "mutsuki.fs.write";
pub const APPEND: &str = "mutsuki.fs.append";
pub const COPY: &str = "mutsuki.fs.copy";
pub const MOVE: &str = "mutsuki.fs.move";
pub const REMOVE: &str = "mutsuki.fs.remove";
pub const MKDIR: &str = "mutsuki.fs.mkdir";
pub const LIST: &str = "mutsuki.fs.list";
pub const STAT: &str = "mutsuki.fs.stat";
pub const EXISTS: &str = "mutsuki.fs.exists";
pub const PROTOCOL_IDS: &[&str] = &[
    READ, WRITE, APPEND, COPY, MOVE, REMOVE, MKDIR, LIST, STAT, EXISTS,
];

pub fn input_schema(protocol_id: &str) -> Option<Value> {
    PROTOCOL_IDS
        .contains(&protocol_id)
        .then(|| json!({"type": "object", "required": ["path", "allowlist"]}))
}

pub fn output_schema(protocol_id: &str) -> Option<Value> {
    PROTOCOL_IDS
        .contains(&protocol_id)
        .then(|| json!({"type": "object"}))
}

pub fn error_schema(protocol_id: &str) -> Option<Value> {
    output_schema(protocol_id)
}
