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
    pub trace_id: TraceId,
    pub span_id: SpanId,
    #[serde(default)]
    pub parent_span_id: Option<SpanId>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub start: f64,
    #[serde(default)]
    pub end: Option<f64>,
    #[serde(default)]
    pub attributes: BTreeMap<String, ScalarValue>,
    #[serde(default = "default_span_status")]
    pub status: SpanStatus,
}

fn default_span_status() -> SpanStatus {
    SpanStatus::Ok
}
