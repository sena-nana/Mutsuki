use std::collections::{BTreeMap, VecDeque};

use mutsuki_runtime_contracts::{
    DEFAULT_EVENT_CAPACITY, DEFAULT_TRACE_CAPACITY, ObservabilityOutletProfile,
    ObservabilityOverflowPolicy, ObservabilityPage, RuntimeError, RuntimeEvent, RuntimeEventKind,
    ScalarValue, SpanStatus, TraceSpan,
};

#[derive(Clone, Debug)]
struct BoundedLog<T> {
    records: VecDeque<T>,
    next_sequence: u64,
    profile: ObservabilityOutletProfile,
    dropped: u64,
}

impl<T> BoundedLog<T> {
    fn new(profile: ObservabilityOutletProfile) -> Self {
        Self {
            records: if profile.capacity == 0 {
                VecDeque::new()
            } else {
                VecDeque::with_capacity(profile.capacity)
            },
            next_sequence: 0,
            profile,
            dropped: 0,
        }
    }

    fn configure(&mut self, profile: ObservabilityOutletProfile) {
        self.profile = profile;
        while self.records.len() > self.profile.capacity {
            match self.profile.overflow_policy {
                ObservabilityOverflowPolicy::DropOldest => self.records.pop_front(),
                ObservabilityOverflowPolicy::DropNew => self.records.pop_back(),
            };
            self.dropped = self.dropped.saturating_add(1);
        }
        if self.profile.capacity == 0 {
            self.records.shrink_to_fit();
        } else {
            self.records.shrink_to(self.profile.capacity);
        }
    }

    fn set_capacity(&mut self, capacity: usize) {
        self.configure(ObservabilityOutletProfile::new(
            capacity,
            self.profile.overflow_policy,
        ));
    }

    fn next_sequence(&mut self) -> u64 {
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.next_sequence
    }

    fn is_enabled(&self) -> bool {
        self.profile.capacity > 0
    }

    fn will_retain_next(&self) -> bool {
        self.is_enabled()
            && (self.records.len() < self.profile.capacity
                || self.profile.overflow_policy == ObservabilityOverflowPolicy::DropOldest)
    }

    fn make_room(&mut self) -> bool {
        if !self.will_retain_next() {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        if self.records.len() == self.profile.capacity {
            self.records.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        true
    }

    fn retain(&mut self, record: T) {
        if !self.make_room() {
            return;
        }
        self.records.push_back(record);
    }

    fn record_with(&mut self, build: impl FnOnce(u64) -> T) -> Option<T>
    where
        T: Clone,
    {
        let sequence = self.next_sequence();
        if !self.will_retain_next() {
            self.dropped = self.dropped.saturating_add(1);
            return None;
        }
        let record = build(sequence);
        self.retain(record.clone());
        Some(record)
    }

    fn page_after(
        &self,
        sequence: u64,
        limit: usize,
        item_sequence: impl Fn(&T) -> u64,
    ) -> ObservabilityPage<T>
    where
        T: Clone,
    {
        let earliest_available_sequence = self.records.front().map(&item_sequence);
        let items: Vec<_> = self
            .records
            .iter()
            .filter(|item| item_sequence(item) > sequence)
            .take(limit)
            .cloned()
            .collect();
        let next_sequence = if limit == 0 {
            sequence
        } else {
            items
                .last()
                .map(&item_sequence)
                .unwrap_or_else(|| sequence.max(self.next_sequence))
        };
        let lost = if limit == 0 {
            0
        } else {
            next_sequence
                .saturating_sub(sequence)
                .saturating_sub(items.len() as u64)
        };
        ObservabilityPage {
            items,
            next_sequence,
            earliest_available_sequence,
            latest_sequence: self.next_sequence,
            lost,
            truncated: self.next_sequence > next_sequence,
            dropped: self.dropped,
        }
    }

    fn retained(&self) -> usize {
        self.records.len()
    }

    fn dropped(&self) -> u64 {
        self.dropped
    }

    fn allocated_capacity(&self) -> usize {
        self.records.capacity()
    }
}

#[derive(Clone, Debug)]
pub struct EventLog {
    inner: BoundedLog<RuntimeEvent>,
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
    pub fn with_profile(profile: ObservabilityOutletProfile) -> Self {
        Self {
            inner: BoundedLog::new(profile),
        }
    }

    pub fn configure(&mut self, profile: ObservabilityOutletProfile) {
        self.inner.configure(profile);
    }

    pub fn set_capacity(&mut self, capacity: usize) {
        self.inner.set_capacity(capacity);
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.is_enabled()
    }

    pub fn will_retain_next(&self) -> bool {
        self.inner.will_retain_next()
    }

    pub fn iter(&self) -> impl Iterator<Item = &RuntimeEvent> {
        self.inner.records.iter()
    }

    pub fn record(
        &mut self,
        kind: RuntimeEventKind,
        name: impl Into<String>,
        subject_id: Option<String>,
        attributes: BTreeMap<String, ScalarValue>,
        error: Option<RuntimeError>,
    ) -> Option<RuntimeEvent> {
        self.record_with(|sequence| RuntimeEvent {
            sequence,
            kind,
            name: name.into(),
            subject_id,
            attributes,
            error,
        })
    }

    pub fn record_with(&mut self, build: impl FnOnce(u64) -> RuntimeEvent) -> Option<RuntimeEvent> {
        self.inner.record_with(build)
    }

    pub fn snapshot(&self) -> &VecDeque<RuntimeEvent> {
        &self.inner.records
    }

    pub fn page_after(&self, sequence: u64, limit: usize) -> ObservabilityPage<RuntimeEvent> {
        self.inner
            .page_after(sequence, limit, |event| event.sequence)
    }

    pub fn drain(&mut self) -> Vec<RuntimeEvent> {
        self.inner.records.drain(..).collect()
    }

    pub fn retained(&self) -> usize {
        self.inner.retained()
    }

    pub fn dropped(&self) -> u64 {
        self.inner.dropped()
    }
}

#[derive(Clone, Debug)]
pub struct TraceLog {
    inner: BoundedLog<TraceSpan>,
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
            inner: BoundedLog::new(profile),
        }
    }

    pub fn configure(&mut self, profile: ObservabilityOutletProfile) {
        self.inner.configure(profile);
    }

    pub fn set_capacity(&mut self, capacity: usize) {
        self.inner.set_capacity(capacity);
    }

    pub fn will_retain_next(&self) -> bool {
        self.inner.will_retain_next()
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
        self.inner.record_with(build)
    }

    pub fn spans(&self) -> &VecDeque<TraceSpan> {
        &self.inner.records
    }

    pub fn page_after(&self, sequence: u64, limit: usize) -> ObservabilityPage<TraceSpan> {
        self.inner.page_after(sequence, limit, |span| span.sequence)
    }

    pub fn retained(&self) -> usize {
        self.inner.retained()
    }

    pub fn dropped(&self) -> u64 {
        self.inner.dropped()
    }

    pub fn allocated_capacity(&self) -> usize {
        self.inner.allocated_capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_trace(log: &mut TraceLog, trace_id: &str) {
        log.record(trace_id, "test.span", None, SpanStatus::Ok, BTreeMap::new());
    }

    #[test]
    fn disabled_and_drop_new_event_logs_do_not_invoke_lazy_builder() {
        for profile in [
            ObservabilityOutletProfile::new(0, ObservabilityOverflowPolicy::DropOldest),
            ObservabilityOutletProfile::new(1, ObservabilityOverflowPolicy::DropNew),
        ] {
            let mut log = EventLog::with_profile(profile);
            if log.inner.profile.capacity == 1 {
                log.record(
                    RuntimeEventKind::Task,
                    "retained",
                    None,
                    BTreeMap::new(),
                    None,
                );
            }
            let mut built = false;
            let event = log.record_with(|sequence| {
                built = true;
                RuntimeEvent {
                    sequence,
                    kind: RuntimeEventKind::Task,
                    name: "discarded".into(),
                    subject_id: None,
                    attributes: BTreeMap::new(),
                    error: None,
                }
            });
            assert!(event.is_none());
            assert!(!built);
        }
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
            log.spans()
                .iter()
                .map(|span| span.sequence)
                .collect::<Vec<_>>(),
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
