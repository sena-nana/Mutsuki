use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{ScalarValue, SpanId, TraceId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanStatus {
    Ok,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TraceSpan {
    pub sequence: u64,
    pub trace_id: TraceId,
    pub span_id: SpanId,
    pub parent_span_id: Option<SpanId>,
    pub name: String,
    pub start: f64,
    pub end: Option<f64>,
    pub attributes: BTreeMap<String, ScalarValue>,
    pub status: SpanStatus,
}
