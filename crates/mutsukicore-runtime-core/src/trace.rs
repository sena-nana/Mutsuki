use std::collections::{BTreeMap, HashMap, HashSet};

use mutsukicore_runtime_contracts::{ScalarValue, SpanStatus, TraceSpan};

#[derive(Clone, Debug, Default)]
pub struct TraceBook {
    spans: Vec<TraceSpan>,
    next_span: u64,
}

impl TraceBook {
    pub(crate) fn record(
        &mut self,
        agent_id: &str,
        name: impl Into<String>,
        parent_span_id: Option<String>,
        status: SpanStatus,
    ) -> TraceSpan {
        self.next_span += 1;
        let mut attributes = BTreeMap::new();
        attributes.insert(
            "agent_id".to_string(),
            ScalarValue::String(agent_id.to_string()),
        );
        let span_id = format!("span-{}", self.next_span);
        let span = TraceSpan {
            trace_id: format!("trace-{agent_id}"),
            span_id,
            parent_span_id,
            name: name.into(),
            start: self.next_span as f64,
            end: Some(self.next_span as f64),
            attributes,
            status,
        };
        self.spans.push(span.clone());
        span
    }

    pub fn spans(&self) -> &[TraceSpan] {
        &self.spans
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceClosureIssue {
    DuplicateSpanId {
        span_id: String,
    },
    MissingParent {
        span_id: String,
        parent_span_id: String,
    },
    ParentTraceMismatch {
        span_id: String,
        parent_span_id: String,
    },
    InvalidInterval {
        span_id: String,
    },
}

pub fn validate_trace_closure(spans: &[TraceSpan]) -> Vec<TraceClosureIssue> {
    let mut issues = Vec::new();
    let mut seen = HashSet::new();
    let mut by_span_id: HashMap<&str, &TraceSpan> = HashMap::new();

    for span in spans {
        if !seen.insert(span.span_id.as_str()) {
            issues.push(TraceClosureIssue::DuplicateSpanId {
                span_id: span.span_id.clone(),
            });
        }
        by_span_id.entry(span.span_id.as_str()).or_insert(span);
        if span.end.is_some_and(|end| end < span.start) {
            issues.push(TraceClosureIssue::InvalidInterval {
                span_id: span.span_id.clone(),
            });
        }
    }

    for span in spans {
        let Some(parent_span_id) = &span.parent_span_id else {
            continue;
        };
        let Some(parent) = by_span_id.get(parent_span_id.as_str()) else {
            issues.push(TraceClosureIssue::MissingParent {
                span_id: span.span_id.clone(),
                parent_span_id: parent_span_id.clone(),
            });
            continue;
        };
        if parent.trace_id != span.trace_id {
            issues.push(TraceClosureIssue::ParentTraceMismatch {
                span_id: span.span_id.clone(),
                parent_span_id: parent_span_id.clone(),
            });
        }
    }

    issues
}
