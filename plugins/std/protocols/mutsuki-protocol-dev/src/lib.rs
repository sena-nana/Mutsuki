use serde_json::{Value, json};

pub const VERSION: &str = "0.1.0";
pub const ABI_CODEC: &str = "serde-json";
pub const ECHO: &str = "mutsuki.dev.echo";
pub const SLEEP: &str = "mutsuki.dev.sleep";
pub const FAIL: &str = "mutsuki.dev.fail";
pub const RANDOM_FAIL: &str = "mutsuki.dev.random_fail";
pub const PRODUCE_RESOURCE: &str = "mutsuki.dev.produce_resource";
pub const CONSUME_RESOURCE: &str = "mutsuki.dev.consume_resource";
pub const PROTOCOL_IDS: &[&str] = &[
    ECHO,
    SLEEP,
    FAIL,
    RANDOM_FAIL,
    PRODUCE_RESOURCE,
    CONSUME_RESOURCE,
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
