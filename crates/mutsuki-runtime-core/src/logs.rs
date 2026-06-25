use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    RuntimeError, RuntimeEvent, RuntimeEventKind, ScalarValue, SpanStatus, TraceSpan,
};

#[derive(Clone, Debug, Default)]
pub struct EventLog {
    events: Vec<RuntimeEvent>,
    next_sequence: u64,
}

impl EventLog {
    pub fn record(
        &mut self,
        kind: RuntimeEventKind,
        name: impl Into<String>,
        subject_id: Option<String>,
        attributes: BTreeMap<String, ScalarValue>,
        error: Option<RuntimeError>,
    ) -> RuntimeEvent {
        self.next_sequence += 1;
        let event = RuntimeEvent {
            sequence: self.next_sequence,
            kind,
            name: name.into(),
            subject_id,
            attributes,
            error,
        };
        self.events.push(event.clone());
        event
    }

    pub fn snapshot(&self) -> &[RuntimeEvent] {
        &self.events
    }

    pub fn drain(&mut self) -> Vec<RuntimeEvent> {
        self.events.drain(..).collect()
    }
}

#[derive(Clone, Debug, Default)]
pub struct TraceLog {
    spans: Vec<TraceSpan>,
    next_span: u64,
}

impl TraceLog {
    pub fn record(
        &mut self,
        trace_id: impl Into<String>,
        name: impl Into<String>,
        parent_span_id: Option<String>,
        status: SpanStatus,
        attributes: BTreeMap<String, ScalarValue>,
    ) -> TraceSpan {
        self.next_span += 1;
        let span = TraceSpan {
            trace_id: trace_id.into(),
            span_id: format!("span-{}", self.next_span),
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
