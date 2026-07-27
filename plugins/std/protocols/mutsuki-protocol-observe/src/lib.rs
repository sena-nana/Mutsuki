use serde_json::{Value, json};

pub const VERSION: &str = "0.1.0";
pub const ABI_CODEC: &str = "serde-json";
pub const LOG_EMIT: &str = "mutsuki.log.emit";
pub const TRACE_SPAN_START: &str = "mutsuki.trace.span_start";
pub const TRACE_SPAN_END: &str = "mutsuki.trace.span_end";
pub const TRACE_EVENT: &str = "mutsuki.trace.event";
pub const PROTOCOL_IDS: &[&str] = &[LOG_EMIT, TRACE_SPAN_START, TRACE_SPAN_END, TRACE_EVENT];

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
