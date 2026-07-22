use super::types::DeliveryPhase;
use std::collections::{BTreeMap, VecDeque};

/// Max terminal delivery phases retained for desktop hosts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationHistoryPolicy {
    pub max_terminal_entries: usize,
}

impl OperationHistoryPolicy {
    pub fn desktop_default() -> Self {
        Self {
            max_terminal_entries: 10_000,
        }
    }
}

impl Default for OperationHistoryPolicy {
    fn default() -> Self {
        Self::desktop_default()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OperationHistoryStats {
    pub entries: usize,
    pub terminal_entries: usize,
    pub evictions: u64,
}

#[derive(Debug)]
pub(crate) struct OperationHistory {
    phases: BTreeMap<String, DeliveryPhase>,
    terminal_order: VecDeque<String>,
    policy: OperationHistoryPolicy,
    evictions: u64,
}

impl OperationHistory {
    pub(crate) fn new(policy: OperationHistoryPolicy) -> Self {
        Self {
            phases: BTreeMap::new(),
            terminal_order: VecDeque::new(),
            policy,
            evictions: 0,
        }
    }

    pub(crate) fn record(&mut self, request_id: impl Into<String>, phase: DeliveryPhase) {
        let request_id = request_id.into();
        let previously_terminal = self
            .phases
            .get(&request_id)
            .is_some_and(DeliveryPhase::is_terminal);
        self.phases.insert(request_id.clone(), phase.clone());
        if phase.is_terminal() {
            if !previously_terminal {
                self.terminal_order.push_back(request_id);
            }
            while self.terminal_order.len() > self.policy.max_terminal_entries {
                let Some(old_id) = self.terminal_order.pop_front() else {
                    break;
                };
                if self
                    .phases
                    .get(&old_id)
                    .is_some_and(DeliveryPhase::is_terminal)
                {
                    self.phases.remove(&old_id);
                    self.evictions = self.evictions.saturating_add(1);
                }
            }
        }
    }

    pub(crate) fn phase_for(&self, request_id: &str) -> Option<DeliveryPhase> {
        self.phases.get(request_id).cloned()
    }

    pub(crate) fn stats(&self) -> OperationHistoryStats {
        OperationHistoryStats {
            entries: self.phases.len(),
            terminal_entries: self.terminal_order.len(),
            evictions: self.evictions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_active_entries_and_bounds_terminal_history() {
        let mut history = OperationHistory::new(OperationHistoryPolicy {
            max_terminal_entries: 2,
        });
        history.record("a", DeliveryPhase::Connecting);
        history.record("b", DeliveryPhase::Connecting);
        history.record("a", DeliveryPhase::Accepted);
        history.record("b", DeliveryPhase::Completed);
        history.record("c", DeliveryPhase::DeliveryFailed);
        let stats = history.stats();
        assert_eq!(stats.terminal_entries, 2);
        assert_eq!(stats.evictions, 1);
        assert!(history.phase_for("a").is_none());
        assert_eq!(history.phase_for("b"), Some(DeliveryPhase::Completed));
        assert_eq!(history.phase_for("c"), Some(DeliveryPhase::DeliveryFailed));
    }
}
