use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mutsuki_agent_contracts::{
    AgentError, CompactionRequest, CompactionResult, ContextBudget, ContextDecisionKind,
    ContextItemRef, ContextPlan, ContextPriority, ContextProvenance, ResourceRef, SessionVersion,
};

use crate::ContextPlanner;

pub trait ContextCompactor: Send + Sync {
    fn compact(
        &self,
        request: CompactionRequest,
        cancellation: &CompactionCancellation,
    ) -> Result<CompactionResult, AgentError>;
}

#[derive(Clone, Default)]
pub struct CompactionCancellation {
    cancelled: Arc<AtomicBool>,
}

impl CompactionCancellation {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompactionDisposition {
    NotRequired,
    Completed,
    FallbackCancelled,
    FallbackError { code: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedContext {
    pub plan: ContextPlan,
    pub compaction: CompactionDisposition,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PendingContextCompaction {
    initial: ContextPlan,
    request: CompactionRequest,
    source_snapshot: ResourceRef,
    version: SessionVersion,
    budget: ContextBudget,
}

impl PendingContextCompaction {
    pub fn request(&self) -> &CompactionRequest {
        &self.request
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ContextCompactionPreparation {
    Ready(PreparedContext),
    Required(Box<PendingContextCompaction>),
}

#[derive(Clone, Debug, Default)]
pub struct ContextCompactionCoordinator {
    planner: ContextPlanner,
}

impl ContextCompactionCoordinator {
    #[allow(clippy::too_many_arguments)]
    pub fn begin(
        &self,
        session_id: &str,
        turn_id: &str,
        version: SessionVersion,
        budget: ContextBudget,
        items: Vec<ContextItemRef>,
        source_snapshot: ResourceRef,
    ) -> Result<ContextCompactionPreparation, AgentError> {
        let initial = self
            .planner
            .plan(session_id, turn_id, version, budget.clone(), items)?;
        if !initial
            .decisions
            .iter()
            .any(|decision| decision.decision == ContextDecisionKind::CompactionRequired)
        {
            return Ok(ContextCompactionPreparation::Ready(PreparedContext {
                plan: initial,
                compaction: CompactionDisposition::NotRequired,
            }));
        }

        let request = CompactionRequest {
            session_id: session_id.into(),
            turn_id: turn_id.into(),
            source_snapshot: source_snapshot.clone(),
            target_budget: budget.clone(),
        };
        Ok(ContextCompactionPreparation::Required(Box::new(
            PendingContextCompaction {
                initial,
                request,
                source_snapshot,
                version,
                budget,
            },
        )))
    }

    pub fn finish(
        &self,
        pending: PendingContextCompaction,
        outcome: Result<CompactionResult, AgentError>,
        cancellation: &CompactionCancellation,
    ) -> Result<PreparedContext, AgentError> {
        debug_assert!(
            pending
                .initial
                .decisions
                .iter()
                .any(|decision| { decision.decision == ContextDecisionKind::CompactionRequired })
        );
        let (item, disposition) = if cancellation.is_cancelled() {
            (
                fallback_item(pending.source_snapshot.clone(), pending.version),
                CompactionDisposition::FallbackCancelled,
            )
        } else {
            match outcome {
                Ok(compacted) if !cancellation.is_cancelled() => {
                    (compacted_item(compacted), CompactionDisposition::Completed)
                }
                Ok(_) => (
                    fallback_item(pending.source_snapshot.clone(), pending.version),
                    CompactionDisposition::FallbackCancelled,
                ),
                Err(error) => (
                    fallback_item(pending.source_snapshot.clone(), pending.version),
                    CompactionDisposition::FallbackError { code: error.code },
                ),
            }
        };
        let plan = self.planner.plan(
            &pending.request.session_id,
            &pending.request.turn_id,
            pending.version,
            pending.budget,
            vec![item],
        )?;
        Ok(PreparedContext {
            plan,
            compaction: disposition,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &self,
        session_id: &str,
        turn_id: &str,
        version: SessionVersion,
        budget: ContextBudget,
        items: Vec<ContextItemRef>,
        source_snapshot: ResourceRef,
        compactor: &dyn ContextCompactor,
        cancellation: &CompactionCancellation,
    ) -> Result<PreparedContext, AgentError> {
        match self.begin(session_id, turn_id, version, budget, items, source_snapshot)? {
            ContextCompactionPreparation::Ready(prepared) => Ok(prepared),
            ContextCompactionPreparation::Required(pending) => {
                let outcome = if cancellation.is_cancelled() {
                    Err(AgentError::new(
                        "agent.context.compaction_cancelled",
                        "context compaction was cancelled",
                    ))
                } else {
                    compactor.compact(pending.request().clone(), cancellation)
                };
                self.finish(*pending, outcome, cancellation)
            }
        }
    }
}

fn compacted_item(result: CompactionResult) -> ContextItemRef {
    let estimated_bytes = result.summary.len() as u64;
    ContextItemRef {
        item_id: format!("compacted:{}", result.compacted_snapshot.ref_id),
        category: "session".into(),
        summary: result.summary,
        details: Some(result.compacted_snapshot),
        priority: ContextPriority::Required,
        estimated_tokens: estimated_bytes.div_ceil(4).max(1),
        estimated_bytes,
        provenance: result.provenance,
        required: true,
    }
}

fn fallback_item(source_snapshot: ResourceRef, version: SessionVersion) -> ContextItemRef {
    const SUMMARY: &str = "Session context remains available through its immutable snapshot.";
    ContextItemRef {
        item_id: format!("snapshot:{}", source_snapshot.ref_id),
        category: "session".into(),
        summary: SUMMARY.into(),
        details: Some(source_snapshot.clone()),
        priority: ContextPriority::Required,
        estimated_tokens: (SUMMARY.len() as u64).div_ceil(4),
        estimated_bytes: SUMMARY.len() as u64,
        provenance: ContextProvenance {
            provider_id: "mutsuki.agent.runtime.compaction".into(),
            source_kind: "session_snapshot".into(),
            source_id: source_snapshot.ref_id,
            source_version: version.0.to_string(),
        },
        required: true,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use mutsuki_runtime_contracts::{
        ResourceAccess, ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic,
    };

    use super::*;

    struct CountingCompactor(AtomicUsize);

    impl ContextCompactor for CountingCompactor {
        fn compact(
            &self,
            _request: CompactionRequest,
            _cancellation: &CompactionCancellation,
        ) -> Result<CompactionResult, AgentError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Err(AgentError::provider_unavailable("offline"))
        }
    }

    fn reference(slot: &str) -> ResourceRef {
        ResourceRef {
            ref_id: format!("context:{slot}"),
            resource_id: ResourceId {
                kind_id: "agent.context".into(),
                slot_id: slot.into(),
                generation: 1,
                version: 1,
            },
            semantic: ResourceSemantic::FrozenValue,
            provider_id: "test".into(),
            resource_kind: "agent.context".into(),
            schema: "agent.context@1".into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::Inline,
            size_hint: Some(1_000_000),
            content_hash: Some("sha256:test".into()),
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        }
    }

    fn oversized_item(source: ResourceRef) -> ContextItemRef {
        ContextItemRef {
            item_id: "history".into(),
            category: "session".into(),
            summary: "full history".into(),
            details: Some(source),
            priority: ContextPriority::Required,
            estimated_tokens: 10_000,
            estimated_bytes: 1_000_000,
            provenance: ContextProvenance {
                provider_id: "session".into(),
                source_kind: "snapshot".into(),
                source_id: "history".into(),
                source_version: "1".into(),
            },
            required: true,
        }
    }

    #[test]
    fn cancelled_compaction_skips_provider_and_uses_stable_snapshot_fallback() {
        let source = reference("source");
        let cancellation = CompactionCancellation::default();
        cancellation.cancel();
        let compactor = CountingCompactor(AtomicUsize::new(0));
        let coordinator = ContextCompactionCoordinator::default();
        let budget = ContextBudget {
            max_tokens: Some(64),
            max_bytes: Some(512),
            max_items: Some(1),
        };
        let first = coordinator
            .prepare(
                "session",
                "turn",
                SessionVersion(1),
                budget.clone(),
                vec![oversized_item(source.clone())],
                source.clone(),
                &compactor,
                &cancellation,
            )
            .unwrap();
        let second = coordinator
            .prepare(
                "session",
                "turn",
                SessionVersion(1),
                budget,
                vec![oversized_item(source.clone())],
                source,
                &compactor,
                &cancellation,
            )
            .unwrap();
        assert_eq!(first.compaction, CompactionDisposition::FallbackCancelled);
        assert_eq!(compactor.0.load(Ordering::Relaxed), 0);
        assert_eq!(first.plan.stable_hash, second.plan.stable_hash);
        assert!(first.plan.items[0].details.is_some());
        assert_eq!(
            first.plan.decisions[0].decision,
            ContextDecisionKind::Included
        );
    }
}
