use mutsukicore_runtime_contracts::AgentId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElectionCandidate {
    pub agent_id: AgentId,
    pub priority: i64,
}

pub trait ElectionPolicy {
    fn select(&self, candidates: &[ElectionCandidate]) -> Option<AgentId>;
}

#[derive(Clone, Debug, Default)]
pub struct PriorityElectionPolicy;

impl ElectionPolicy for PriorityElectionPolicy {
    fn select(&self, candidates: &[ElectionCandidate]) -> Option<AgentId> {
        let mut candidates = candidates.to_vec();
        candidates.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.agent_id.cmp(&b.agent_id))
        });
        candidates
            .first()
            .map(|candidate| candidate.agent_id.clone())
    }
}
