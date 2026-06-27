#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleDecision {
    pub scheduler_id: String,
    pub reason: String,
    pub requested_dispatch_limit: usize,
    pub dispatch_limit: usize,
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
        }
    }

    pub fn clamp_to(mut self, hard_capacity: usize) -> Self {
        self.dispatch_limit = self.dispatch_limit.min(hard_capacity);
        self
    }
}
