use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{ScalarValue, SpanStatus, TraceSpan};

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
