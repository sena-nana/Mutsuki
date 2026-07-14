use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    RuntimeError, RuntimeEvent, RuntimeEventKind, ScalarValue, SpanStatus, TraceSpan,
};

pub const DEFAULT_EVENT_CAPACITY: usize = 4096;

#[derive(Clone, Debug)]
pub struct EventLog {
    events: Vec<RuntimeEvent>,
    next_sequence: u64,
    capacity: usize,
    dropped: u64,
}

impl Default for EventLog {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_EVENT_CAPACITY)
    }
}

impl EventLog {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            events: Vec::with_capacity(capacity),
            next_sequence: 0,
            capacity,
            dropped: 0,
        }
    }

    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
        if self.events.len() > capacity {
            let removed = self.events.len() - capacity;
            self.events.truncate(capacity);
            self.dropped = self.dropped.saturating_add(removed as u64);
        }
        self.events.shrink_to(capacity);
    }

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
        if self.events.len() < self.capacity {
            self.events.push(event.clone());
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
        event
    }

    pub fn snapshot(&self) -> &[RuntimeEvent] {
        &self.events
    }

    pub fn drain(&mut self) -> Vec<RuntimeEvent> {
        self.events.drain(..).collect()
    }

    pub fn retained(&self) -> usize {
        self.events.len()
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
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
