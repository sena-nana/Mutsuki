use std::collections::BTreeMap;

use mutsuki_runtime_contracts::DispatchLane;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneBudget {
    pub max_entries: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchBudget {
    pub max_entries: usize,
    pub max_batches: usize,
    pub max_bytes: usize,
    pub lane_budget: BTreeMap<DispatchLane, LaneBudget>,
}

impl DispatchBudget {
    pub fn single_batch(max_entries: usize) -> Self {
        Self {
            max_entries,
            max_batches: usize::from(max_entries > 0),
            max_bytes: usize::MAX,
            lane_budget: BTreeMap::new(),
        }
    }

    pub fn clamp_to(mut self, hard_capacity: usize) -> Self {
        self.max_entries = self.max_entries.min(hard_capacity);
        if self.max_entries == 0 {
            self.max_batches = 0;
        } else {
            self.max_batches = self.max_batches.min(self.max_entries);
        }
        self
    }

    pub fn clamp_batches(mut self, hard_batch_capacity: usize) -> Self {
        self.max_batches = self.max_batches.min(hard_batch_capacity);
        if self.max_batches == 0 {
            self.max_entries = 0;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_budget_preserves_bounded_multi_batch_capacity() {
        let budget = DispatchBudget {
            max_entries: 8,
            max_batches: 4,
            max_bytes: usize::MAX,
            lane_budget: BTreeMap::new(),
        }
        .clamp_to(8);

        assert_eq!(budget.max_entries, 8);
        assert_eq!(budget.max_batches, 4);
    }

    #[test]
    fn empty_dispatch_budget_has_no_active_batch() {
        let budget = DispatchBudget {
            max_entries: 8,
            max_batches: 4,
            max_bytes: usize::MAX,
            lane_budget: BTreeMap::new(),
        }
        .clamp_to(0);

        assert_eq!(budget.max_entries, 0);
        assert_eq!(budget.max_batches, 0);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleDecision {
    pub scheduler_id: String,
    pub reason: String,
    pub requested_dispatch_limit: usize,
    pub dispatch_limit: usize,
    pub budget: DispatchBudget,
}

impl ScheduleDecision {
    pub fn new(
        scheduler_id: impl Into<String>,
        dispatch_limit: usize,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            scheduler_id: scheduler_id.into(),
            reason: reason.into(),
            requested_dispatch_limit: dispatch_limit,
            dispatch_limit,
            budget: DispatchBudget::single_batch(dispatch_limit),
        }
    }

    pub fn clamp_to(mut self, hard_capacity: usize) -> Self {
        self.dispatch_limit = self.dispatch_limit.min(hard_capacity);
        self.budget = self.budget.clamp_to(hard_capacity);
        self
    }

    pub fn clamp_batches(mut self, hard_batch_capacity: usize) -> Self {
        self.budget = self.budget.clamp_batches(hard_batch_capacity);
        if self.budget.max_batches == 0 {
            self.dispatch_limit = 0;
        }
        self
    }

    pub fn with_budget(mut self, budget: DispatchBudget) -> Self {
        self.budget = budget;
        self.dispatch_limit = self.dispatch_limit.min(self.budget.max_entries);
        self
    }
}
