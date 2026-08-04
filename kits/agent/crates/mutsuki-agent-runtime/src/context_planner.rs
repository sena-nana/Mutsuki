use mutsuki_agent_contracts::{
    AgentError, ContextBudget, ContextDecision, ContextDecisionKind, ContextItemRef, ContextPlan,
    SessionVersion,
};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Default)]
pub struct ContextPlanner;

impl ContextPlanner {
    pub fn plan(
        &self,
        session_id: impl Into<String>,
        turn_id: impl Into<String>,
        version: SessionVersion,
        budget: ContextBudget,
        items: Vec<ContextItemRef>,
    ) -> Result<ContextPlan, AgentError> {
        self.plan_with_decisions(session_id, turn_id, version, budget, items, Vec::new())
    }

    pub fn plan_with_decisions(
        &self,
        session_id: impl Into<String>,
        turn_id: impl Into<String>,
        version: SessionVersion,
        budget: ContextBudget,
        mut items: Vec<ContextItemRef>,
        mut decisions: Vec<ContextDecision>,
    ) -> Result<ContextPlan, AgentError> {
        items.sort_by(|left, right| {
            left.priority
                .rank()
                .cmp(&right.priority.rank())
                .then_with(|| left.category.cmp(&right.category))
                .then_with(|| left.item_id.cmp(&right.item_id))
        });
        decisions.sort_by(|left, right| left.item_id.cmp(&right.item_id));
        if decisions
            .windows(2)
            .any(|pair| pair[0].item_id == pair[1].item_id)
        {
            return Err(AgentError::invalid_input(
                "context decisions require unique item ids",
            ));
        }
        let mut included = Vec::new();
        decisions.reserve(items.len());
        let mut total_tokens = 0_u64;
        let mut total_bytes = 0_u64;
        for item in items {
            if item.item_id.trim().is_empty() || item.provenance.provider_id.trim().is_empty() {
                return Err(AgentError::invalid_input(
                    "context item id and provider provenance are required",
                ));
            }
            let next_tokens = total_tokens.saturating_add(item.estimated_tokens);
            let next_bytes = total_bytes.saturating_add(item.estimated_bytes);
            let next_items = included.len().saturating_add(1) as u32;
            let fits = budget.max_tokens.is_none_or(|limit| next_tokens <= limit)
                && budget.max_bytes.is_none_or(|limit| next_bytes <= limit)
                && budget.max_items.is_none_or(|limit| next_items <= limit);
            if fits || item.required {
                if item.required && !fits {
                    decisions.push(ContextDecision {
                        item_id: item.item_id.clone(),
                        decision: ContextDecisionKind::CompactionRequired,
                        reason: "required item exceeds context budget".into(),
                    });
                } else {
                    decisions.push(ContextDecision {
                        item_id: item.item_id.clone(),
                        decision: ContextDecisionKind::Included,
                        reason: "stable priority order within budget".into(),
                    });
                }
                total_tokens = next_tokens;
                total_bytes = next_bytes;
                included.push(item);
            } else {
                decisions.push(ContextDecision {
                    item_id: item.item_id,
                    decision: ContextDecisionKind::DroppedBudget,
                    reason: "optional item exceeds context budget".into(),
                });
            }
        }
        let stable_hash = stable_hash(version, &budget, &included, &decisions)?;
        Ok(ContextPlan {
            session_id: session_id.into(),
            turn_id: turn_id.into(),
            session_version: version,
            budget,
            items: included,
            decisions,
            total_tokens,
            total_bytes,
            stable_hash,
        })
    }
}

fn stable_hash(
    version: SessionVersion,
    budget: &ContextBudget,
    items: &[ContextItemRef],
    decisions: &[ContextDecision],
) -> Result<String, AgentError> {
    let bytes = serde_json::to_vec(&(version, budget, items, decisions))
        .map_err(|err| AgentError::invalid_input(err.to_string()))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use mutsuki_agent_contracts::{ContextPriority, ContextProvenance};

    use super::*;

    fn item(id: &str, priority: ContextPriority, tokens: u64) -> ContextItemRef {
        ContextItemRef {
            item_id: id.into(),
            category: "workspace".into(),
            summary: id.into(),
            details: None,
            priority,
            estimated_tokens: tokens,
            estimated_bytes: tokens * 2,
            provenance: ContextProvenance {
                provider_id: "test".into(),
                source_kind: "fixture".into(),
                source_id: id.into(),
                source_version: "1".into(),
            },
            required: false,
        }
    }

    #[test]
    fn planner_is_deterministic_and_drops_optional_items_by_stable_priority() {
        let planner = ContextPlanner;
        let budget = ContextBudget {
            max_tokens: Some(10),
            max_bytes: Some(100),
            max_items: None,
        };
        let first = planner
            .plan(
                "session",
                "turn",
                SessionVersion(1),
                budget.clone(),
                vec![
                    item("low", ContextPriority::Low, 6),
                    item("high", ContextPriority::High, 6),
                ],
            )
            .unwrap();
        let second = planner
            .plan(
                "session",
                "turn",
                SessionVersion(1),
                budget,
                vec![
                    item("high", ContextPriority::High, 6),
                    item("low", ContextPriority::Low, 6),
                ],
            )
            .unwrap();
        assert_eq!(first.stable_hash, second.stable_hash);
        assert_eq!(first.items[0].item_id, "high");
        assert_eq!(
            first.decisions[1].decision,
            ContextDecisionKind::DroppedBudget
        );
    }
}
