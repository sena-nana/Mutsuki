use serde::{Deserialize, Serialize};

pub const DEFAULT_EVENT_CAPACITY: usize = 4096;
pub const DEFAULT_TRACE_CAPACITY: usize = 4096;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservabilityOverflowPolicy {
    DropOldest,
    #[default]
    DropNew,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservabilityOutletProfile {
    pub capacity: usize,
    pub overflow_policy: ObservabilityOverflowPolicy,
}

impl ObservabilityOutletProfile {
    pub const fn new(capacity: usize, overflow_policy: ObservabilityOverflowPolicy) -> Self {
        Self {
            capacity,
            overflow_policy,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservabilityProfile {
    pub events: ObservabilityOutletProfile,
    pub traces: ObservabilityOutletProfile,
    pub detailed_scheduler_decisions: bool,
    pub dispatch_spans: bool,
}

impl Default for ObservabilityProfile {
    fn default() -> Self {
        Self {
            events: ObservabilityOutletProfile::new(
                DEFAULT_EVENT_CAPACITY,
                ObservabilityOverflowPolicy::DropNew,
            ),
            traces: ObservabilityOutletProfile::new(
                DEFAULT_TRACE_CAPACITY,
                ObservabilityOverflowPolicy::DropOldest,
            ),
            detailed_scheduler_decisions: false,
            dispatch_spans: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObservabilityPage<T> {
    pub items: Vec<T>,
    pub next_sequence: u64,
    pub earliest_available_sequence: Option<u64>,
    pub latest_sequence: u64,
    pub lost: u64,
    pub truncated: bool,
    pub dropped: u64,
}

impl<T> ObservabilityPage<T> {
    pub fn cursor_lost(&self) -> bool {
        self.lost > 0
    }
}
