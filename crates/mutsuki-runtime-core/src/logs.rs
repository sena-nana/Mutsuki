use std::collections::{BTreeMap, VecDeque};

use mutsuki_runtime_contracts::{
    DEFAULT_EVENT_CAPACITY, DEFAULT_TRACE_CAPACITY, ObservabilityOutletProfile,
    ObservabilityOverflowPolicy, ObservabilityPage, RuntimeError, RuntimeEvent, RuntimeEventKind,
    ScalarValue, SpanStatus, TraceSpan,
};

#[derive(Clone, Debug)]
pub struct EventLog {
    events: VecDeque<RuntimeEvent>,
    next_sequence: u64,
    profile: ObservabilityOutletProfile,
    dropped: u64,
}

impl Default for EventLog {
    fn default() -> Self {
        Self::with_profile(ObservabilityOutletProfile::new(
            DEFAULT_EVENT_CAPACITY,
            ObservabilityOverflowPolicy::DropNew,
        ))
    }
}

impl EventLog {
    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_profile(ObservabilityOutletProfile::new(
            capacity,
            ObservabilityOverflowPolicy::DropNew,
        ))
    }

    pub fn with_profile(profile: ObservabilityOutletProfile) -> Self {
        Self {
            events: bounded_queue(profile.capacity),
            next_sequence: 0,
            profile,
            dropped: 0,
        }
    }

    pub fn configure(&mut self, profile: ObservabilityOutletProfile) {
        self.profile = profile;
        trim_to_profile(&mut self.events, &self.profile, &mut self.dropped);
    }

    pub fn set_capacity(&mut self, capacity: usize) {
        self.configure(ObservabilityOutletProfile::new(
            capacity,
            self.profile.overflow_policy,
        ));
    }

    pub fn record(
        &mut self,
        kind: RuntimeEventKind,
        name: impl Into<String>,
        subject_id: Option<String>,
        attributes: BTreeMap<String, ScalarValue>,
        error: Option<RuntimeError>,
    ) -> RuntimeEvent {
        self.next_sequence = self.next_sequence.saturating_add(1);
        let event = RuntimeEvent {
            sequence: self.next_sequence,
            kind,
            name: name.into(),
            subject_id,
            attributes,
            error,
        };
        retain_bounded(
            &mut self.events,
            event.clone(),
            &self.profile,
            &mut self.dropped,
        );
        event
    }

    pub fn is_enabled(&self) -> bool {
        self.profile.capacity > 0
    }

    pub fn iter(&self) -> impl Iterator<Item = &RuntimeEvent> {
        self.events.iter()
    }

    pub fn snapshot(&self) -> &VecDeque<RuntimeEvent> {
        &self.events
    }

    pub fn page_after(&self, sequence: u64, limit: usize) -> ObservabilityPage<RuntimeEvent> {
        page_after(
            &self.events,
            sequence,
            limit,
            self.next_sequence,
            self.dropped,
            |event| event.sequence,
        )
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

    pub fn allocated_capacity(&self) -> usize {
        self.events.capacity()
    }
}

#[derive(Clone, Debug)]
pub struct TraceLog {
    spans: VecDeque<TraceSpan>,
    next_sequence: u64,
    profile: ObservabilityOutletProfile,
    dropped: u64,
}

impl Default for TraceLog {
    fn default() -> Self {
        Self::with_profile(ObservabilityOutletProfile::new(
            DEFAULT_TRACE_CAPACITY,
            ObservabilityOverflowPolicy::DropOldest,
        ))
    }
}

impl TraceLog {
    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_profile(ObservabilityOutletProfile::new(
            capacity,
            ObservabilityOverflowPolicy::DropOldest,
        ))
    }

    pub fn with_profile(profile: ObservabilityOutletProfile) -> Self {
        Self {
            spans: bounded_queue(profile.capacity),
            next_sequence: 0,
            profile,
            dropped: 0,
        }
    }

    pub fn configure(&mut self, profile: ObservabilityOutletProfile) {
        self.profile = profile;
        trim_to_profile(&mut self.spans, &self.profile, &mut self.dropped);
    }

    pub fn set_capacity(&mut self, capacity: usize) {
        self.configure(ObservabilityOutletProfile::new(
            capacity,
            self.profile.overflow_policy,
        ));
    }

    pub fn is_enabled(&self) -> bool {
        self.profile.capacity > 0
    }

    pub fn will_retain_next(&self) -> bool {
        self.is_enabled()
            && (self.spans.len() < self.profile.capacity
                || self.profile.overflow_policy == ObservabilityOverflowPolicy::DropOldest)
    }

    pub fn record(
        &mut self,
        trace_id: impl Into<String>,
        name: impl Into<String>,
        parent_span_id: Option<String>,
        status: SpanStatus,
        attributes: BTreeMap<String, ScalarValue>,
    ) -> Option<TraceSpan> {
        self.record_with(|sequence| TraceSpan {
            sequence,
            trace_id: trace_id.into(),
            span_id: format!("span-{sequence}"),
            parent_span_id,
            name: name.into(),
            start: sequence as f64,
            end: Some(sequence as f64),
            attributes,
            status,
        })
    }

    pub fn record_with(&mut self, build: impl FnOnce(u64) -> TraceSpan) -> Option<TraceSpan> {
        self.next_sequence = self.next_sequence.saturating_add(1);
        if !self.will_retain_next() {
            self.dropped = self.dropped.saturating_add(1);
            return None;
        }
        let span = build(self.next_sequence);
        retain_bounded(
            &mut self.spans,
            span.clone(),
            &self.profile,
            &mut self.dropped,
        );
        Some(span)
    }

    pub fn iter(&self) -> impl Iterator<Item = &TraceSpan> {
        self.spans.iter()
    }

    pub fn spans(&self) -> &VecDeque<TraceSpan> {
        &self.spans
    }

    pub fn page_after(&self, sequence: u64, limit: usize) -> ObservabilityPage<TraceSpan> {
        page_after(
            &self.spans,
            sequence,
            limit,
            self.next_sequence,
            self.dropped,
            |span| span.sequence,
        )
    }

    pub fn retained(&self) -> usize {
        self.spans.len()
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn allocated_capacity(&self) -> usize {
        self.spans.capacity()
    }
}

fn bounded_queue<T>(capacity: usize) -> VecDeque<T> {
    if capacity == 0 {
        VecDeque::new()
    } else {
        VecDeque::with_capacity(capacity)
    }
}

fn retain_bounded<T>(
    records: &mut VecDeque<T>,
    record: T,
    profile: &ObservabilityOutletProfile,
    dropped: &mut u64,
) {
    if profile.capacity == 0 {
        *dropped = dropped.saturating_add(1);
        return;
    }
    if records.len() < profile.capacity {
        records.push_back(record);
        return;
    }
    *dropped = dropped.saturating_add(1);
    if profile.overflow_policy == ObservabilityOverflowPolicy::DropOldest {
        records.pop_front();
        records.push_back(record);
    }
}

fn trim_to_profile<T>(
    records: &mut VecDeque<T>,
    profile: &ObservabilityOutletProfile,
    dropped: &mut u64,
) {
    while records.len() > profile.capacity {
        match profile.overflow_policy {
            ObservabilityOverflowPolicy::DropOldest => {
                records.pop_front();
            }
            ObservabilityOverflowPolicy::DropNew => {
                records.pop_back();
            }
        }
        *dropped = dropped.saturating_add(1);
    }
    if profile.capacity == 0 {
        records.shrink_to_fit();
    } else {
        records.shrink_to(profile.capacity);
    }
}

fn page_after<T: Clone>(
    records: &VecDeque<T>,
    sequence: u64,
    limit: usize,
    latest_sequence: u64,
    dropped: u64,
    item_sequence: impl Fn(&T) -> u64,
) -> ObservabilityPage<T> {
    let earliest_available_sequence = records.front().map(&item_sequence);
    if limit == 0 {
        return ObservabilityPage {
            items: Vec::new(),
            next_sequence: sequence,
            earliest_available_sequence,
            latest_sequence,
            lost: 0,
            truncated: latest_sequence > sequence,
            dropped,
        };
    }
    let items: Vec<_> = records
        .iter()
        .filter(|item| item_sequence(item) > sequence)
        .take(limit)
        .cloned()
        .collect();
    let next_sequence = items
        .last()
        .map(&item_sequence)
        .unwrap_or_else(|| sequence.max(latest_sequence));
    let observed = items.len() as u64;
    let lost = next_sequence
        .saturating_sub(sequence)
        .saturating_sub(observed);
    ObservabilityPage {
        items,
        next_sequence,
        earliest_available_sequence,
        latest_sequence,
        lost,
        truncated: latest_sequence > next_sequence,
        dropped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_trace(log: &mut TraceLog, trace_id: &str) {
        log.record(trace_id, "test.span", None, SpanStatus::Ok, BTreeMap::new());
    }

    #[test]
    fn drop_oldest_reports_evicted_cursor_and_keeps_newest_records() {
        let mut log = TraceLog::with_profile(ObservabilityOutletProfile::new(
            2,
            ObservabilityOverflowPolicy::DropOldest,
        ));
        for trace_id in ["one", "two", "three", "four"] {
            record_trace(&mut log, trace_id);
        }

        let page = log.page_after(0, 8);
        assert_eq!(
            page.items
                .iter()
                .map(|span| span.sequence)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
        assert_eq!(page.lost, 2);
        assert!(page.cursor_lost());
        assert_eq!(page.dropped, 2);
        assert!(!page.truncated);
    }

    #[test]
    fn drop_new_reports_missing_tail_without_replacing_retained_records() {
        let mut log = TraceLog::with_profile(ObservabilityOutletProfile::new(
            2,
            ObservabilityOverflowPolicy::DropNew,
        ));
        for trace_id in ["one", "two", "three", "four"] {
            record_trace(&mut log, trace_id);
        }

        let first = log.page_after(0, 8);
        assert_eq!(
            first
                .items
                .iter()
                .map(|span| span.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(first.truncated);
        let missing_tail = log.page_after(first.next_sequence, 8);
        assert!(missing_tail.items.is_empty());
        assert_eq!(missing_tail.next_sequence, 4);
        assert_eq!(missing_tail.lost, 2);
        assert!(missing_tail.cursor_lost());
    }

    #[test]
    fn page_limit_is_bounded_and_preserves_cursor_progress() {
        let mut log = TraceLog::with_capacity(4);
        for trace_id in ["one", "two", "three", "four"] {
            record_trace(&mut log, trace_id);
        }

        let first = log.page_after(0, 2);
        assert_eq!(first.items.len(), 2);
        assert_eq!(first.next_sequence, 2);
        assert!(first.truncated);
        let second = log.page_after(first.next_sequence, 2);
        assert_eq!(second.items.len(), 2);
        assert_eq!(second.next_sequence, 4);
        assert!(!second.truncated);
    }

    #[test]
    fn dynamic_capacity_change_obeys_policy_and_zero_releases_storage() {
        let mut log = TraceLog::with_capacity(4);
        for trace_id in ["one", "two", "three", "four"] {
            record_trace(&mut log, trace_id);
        }
        log.configure(ObservabilityOutletProfile::new(
            2,
            ObservabilityOverflowPolicy::DropOldest,
        ));
        assert_eq!(
            log.iter().map(|span| span.sequence).collect::<Vec<_>>(),
            vec![3, 4]
        );
        assert_eq!(log.dropped(), 2);

        log.set_capacity(0);
        assert_eq!(log.retained(), 0);
        assert_eq!(log.allocated_capacity(), 0);
    }

    #[test]
    fn zero_capacity_does_not_build_trace_or_allocate_container() {
        let mut log = TraceLog::with_capacity(0);
        let mut built = false;
        let recorded = log.record_with(|sequence| {
            built = true;
            TraceSpan {
                sequence,
                trace_id: "disabled".into(),
                span_id: format!("span-{sequence}"),
                parent_span_id: None,
                name: "disabled".into(),
                start: sequence as f64,
                end: Some(sequence as f64),
                attributes: BTreeMap::new(),
                status: SpanStatus::Ok,
            }
        });

        assert!(recorded.is_none());
        assert!(!built);
        assert_eq!(log.retained(), 0);
        assert_eq!(log.allocated_capacity(), 0);
        assert_eq!(log.dropped(), 1);
    }

    #[test]
    fn event_log_uses_the_same_cursor_loss_semantics() {
        let mut log = EventLog::with_profile(ObservabilityOutletProfile::new(
            1,
            ObservabilityOverflowPolicy::DropOldest,
        ));
        for name in ["one", "two", "three"] {
            log.record(
                RuntimeEventKind::Lifecycle,
                name,
                None,
                BTreeMap::new(),
                None,
            );
        }
        let page = log.page_after(0, 1);
        assert_eq!(page.items[0].sequence, 3);
        assert_eq!(page.lost, 2);
        assert!(page.cursor_lost());
    }
}
