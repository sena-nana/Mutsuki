//! Layered memory routing owned by the Runtime domain.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mutsuki_agent_contracts::{
    AgentError, AgentMemoryActivateRequest, AgentMemoryDeleteRequest, AgentMemoryDisableRequest,
    AgentMemoryQueryRequest, AgentMemoryQueryResult, AgentMemoryRecord, AgentMemoryReviseRequest,
    AgentMemoryWriteRequest, AgentResult, ContextProviderRequest, ContextProviderResult,
    MemoryAdoptionFeedback, MemoryBaselinePolicy, MemoryCandidate, MemoryEntry, MemoryOpportunity,
    MemoryOpportunityMode, MemoryProvenance, MemoryQuery, MemoryRevision, MemoryScopeRef,
};
use mutsuki_agent_sdk::{memory_cell_ref, memory_resource_ref};

const MEMORY_OWNER_ID: &str = "mutsuki.agent.runtime.memory";

#[derive(Clone, Default)]
pub struct MemoryRouter {
    inner: Arc<MemoryRouterInner>,
}

#[derive(Default)]
struct MemoryRouterInner {
    next_id: AtomicU64,
    entries: Mutex<BTreeMap<String, MemoryEntry>>,
    active: Mutex<BTreeSet<String>>,
    opportunities: Mutex<BTreeMap<String, MemoryOpportunity>>,
    feedback: Mutex<Vec<MemoryAdoptionFeedback>>,
    baseline_policy: Mutex<MemoryBaselinePolicy>,
}

impl MemoryRouter {
    pub fn set_baseline_policy(&self, policy: MemoryBaselinePolicy) {
        *self
            .inner
            .baseline_policy
            .lock()
            .expect("memory router mutex poisoned") = policy;
    }

    pub fn baseline_policy(&self) -> MemoryBaselinePolicy {
        self.inner
            .baseline_policy
            .lock()
            .expect("memory router mutex poisoned")
            .clone()
    }

    pub fn query(&self, request: AgentMemoryQueryRequest) -> AgentResult<AgentMemoryQueryResult> {
        let now = request.now_unix_ms.unwrap_or(0);
        let query = request.query.trim().to_lowercase();
        if query.is_empty() {
            return Ok(AgentMemoryQueryResult::default());
        }

        let tag_filter: BTreeSet<_> = request.tags.into_iter().collect();
        let mut records: Vec<_> = self
            .inner
            .entries
            .lock()
            .expect("memory router mutex poisoned")
            .values()
            .filter(|entry| request.include_disabled || entry.enabled)
            .filter(|entry| !is_expired(entry, now))
            .filter(|entry| {
                request
                    .scope
                    .as_ref()
                    .is_none_or(|scope| scope_matches(scope, &entry.scope))
            })
            .filter(|entry| {
                tag_filter.is_empty() || entry.tags.iter().any(|tag| tag_filter.contains(tag))
            })
            .filter_map(|entry| {
                let text = entry.text.to_lowercase();
                let score = score_text(&text, &query);
                (score > 0.0).then(|| {
                    let mut record = AgentMemoryRecord::from(entry);
                    record.score = score;
                    record
                })
            })
            .collect();
        records.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.memory_id.cmp(&right.memory_id))
        });
        records.truncate(request.limit.max(1));
        Ok(AgentMemoryQueryResult { records })
    }

    pub fn write(&self, request: AgentMemoryWriteRequest) -> AgentResult<AgentMemoryRecord> {
        if request.text.trim().is_empty() {
            return Err(AgentError::invalid_input("memory text is required"));
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let memory_id = format!("agent-memory-{id}");
        let scope = request.scope.unwrap_or_else(|| MemoryScopeRef {
            namespace: "default".into(),
            scope_id: "global".into(),
        });
        validate_scope(&scope)?;
        let entry = MemoryEntry {
            memory_id: memory_id.clone(),
            text: request.text,
            scope,
            tags: request.tags,
            score: 1.0,
            priority: request.priority.unwrap_or(0),
            confidence: request.confidence.unwrap_or(1.0),
            enabled: true,
            expiry_unix_ms: request.expiry_unix_ms,
            provenance: request.provenance.unwrap_or(MemoryProvenance {
                source: "manual".into(),
                generation: Some(1),
                actor: None,
                captured_at_unix_ms: None,
            }),
            revision: MemoryRevision {
                revision: 1,
                updated_at_unix_ms: 0,
            },
            metadata: request.metadata,
            resource: Some(
                request
                    .details_ref
                    .unwrap_or_else(|| memory_resource_ref(MEMORY_OWNER_ID, &memory_id)),
            ),
            cell: Some(memory_cell_ref(MEMORY_OWNER_ID, &memory_id)),
        };
        let record = AgentMemoryRecord::from(&entry);
        self.inner
            .entries
            .lock()
            .expect("memory router mutex poisoned")
            .insert(entry.memory_id.clone(), entry);
        Ok(record)
    }

    pub fn activate(&self, request: AgentMemoryActivateRequest) -> AgentResult<AgentMemoryRecord> {
        let entry = self.entry(&request.memory_id)?;
        if !entry.enabled {
            return Err(AgentError::invalid_input(
                "disabled memory cannot be activated",
            ));
        }
        self.inner
            .active
            .lock()
            .expect("memory router mutex poisoned")
            .insert(entry.memory_id.clone());
        Ok(AgentMemoryRecord::from(&entry))
    }

    pub fn disable(&self, request: AgentMemoryDisableRequest) -> AgentResult<AgentMemoryRecord> {
        let mut entries = self
            .inner
            .entries
            .lock()
            .expect("memory router mutex poisoned");
        let entry = entries.get_mut(&request.memory_id).ok_or_else(|| {
            AgentError::not_found(format!("memory `{}` not found", request.memory_id))
        })?;
        entry.enabled = false;
        entry.revision.revision = entry.revision.revision.saturating_add(1);
        self.inner
            .active
            .lock()
            .expect("memory router mutex poisoned")
            .remove(&request.memory_id);
        Ok(AgentMemoryRecord::from(&*entry))
    }

    pub fn delete(&self, request: AgentMemoryDeleteRequest) -> AgentResult<AgentMemoryRecord> {
        let entry = self
            .inner
            .entries
            .lock()
            .expect("memory router mutex poisoned")
            .remove(&request.memory_id)
            .ok_or_else(|| {
                AgentError::not_found(format!("memory `{}` not found", request.memory_id))
            })?;
        self.inner
            .active
            .lock()
            .expect("memory router mutex poisoned")
            .remove(&request.memory_id);
        Ok(AgentMemoryRecord::from(&entry))
    }

    pub fn revise(&self, request: AgentMemoryReviseRequest) -> AgentResult<AgentMemoryRecord> {
        if request.text.trim().is_empty() {
            return Err(AgentError::invalid_input("memory text is required"));
        }
        let mut entries = self
            .inner
            .entries
            .lock()
            .expect("memory router mutex poisoned");
        let entry = entries.get_mut(&request.memory_id).ok_or_else(|| {
            AgentError::not_found(format!("memory `{}` not found", request.memory_id))
        })?;
        entry.text = request.text;
        if let Some(tags) = request.tags {
            entry.tags = tags;
        }
        if request.metadata.is_some() {
            entry.metadata = request.metadata;
        }
        if let Some(details) = request.details_ref {
            entry.resource = Some(details);
        }
        entry.revision.revision = entry.revision.revision.saturating_add(1);
        Ok(AgentMemoryRecord::from(&*entry))
    }

    pub fn detect_opportunity(
        &self,
        session_id: impl Into<String>,
        turn_id: impl Into<String>,
        query: MemoryQuery,
        mode: MemoryOpportunityMode,
    ) -> AgentResult<MemoryOpportunity> {
        validate_scope(&query.scope)?;
        let now = query.now_unix_ms.unwrap_or(0);
        let needle = query.query.trim().to_lowercase();
        if needle.is_empty() {
            return Err(AgentError::invalid_input("opportunity query is required"));
        }
        let mut candidates: Vec<_> = self
            .inner
            .entries
            .lock()
            .expect("memory router mutex poisoned")
            .values()
            .filter(|entry| entry.enabled && !is_expired(entry, now))
            .filter(|entry| scope_matches(&query.scope, &entry.scope))
            .filter(|entry| {
                query.tags.is_empty() || entry.tags.iter().any(|tag| query.tags.contains(tag))
            })
            .filter_map(|entry| {
                let score = score_text(&entry.text.to_lowercase(), &needle);
                (score > 0.0).then(|| MemoryCandidate {
                    entry: {
                        let mut cloned = entry.clone();
                        cloned.score = score;
                        cloned
                    },
                    rank: 0,
                    reason: "lexical_match".into(),
                })
            })
            .collect();
        candidates.sort_by(|left, right| {
            right
                .entry
                .score
                .total_cmp(&left.entry.score)
                .then_with(|| left.entry.memory_id.cmp(&right.entry.memory_id))
                .then_with(|| {
                    left.entry
                        .revision
                        .revision
                        .cmp(&right.entry.revision.revision)
                })
        });
        candidates.truncate(query.limit.max(1));
        for (index, candidate) in candidates.iter_mut().enumerate() {
            candidate.rank = index as u32;
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let opportunity = MemoryOpportunity {
            opportunity_id: format!("memory-opportunity-{id}"),
            session_id: session_id.into(),
            turn_id: turn_id.into(),
            mode,
            candidates,
            trigger: "turn_context".into(),
        };
        self.inner
            .opportunities
            .lock()
            .expect("memory router mutex poisoned")
            .insert(opportunity.opportunity_id.clone(), opportunity.clone());
        Ok(opportunity)
    }

    pub fn record_feedback(&self, feedback: MemoryAdoptionFeedback) -> AgentResult<()> {
        if feedback.opportunity_id.trim().is_empty() || feedback.memory_id.trim().is_empty() {
            return Err(AgentError::invalid_input(
                "feedback requires opportunity_id and memory_id",
            ));
        }
        if !self
            .inner
            .opportunities
            .lock()
            .expect("memory router mutex poisoned")
            .contains_key(&feedback.opportunity_id)
        {
            return Err(AgentError::not_found("memory opportunity was not found"));
        }
        self.inner
            .feedback
            .lock()
            .expect("memory router mutex poisoned")
            .push(feedback);
        Ok(())
    }

    pub fn feedback_log(&self) -> Vec<MemoryAdoptionFeedback> {
        self.inner
            .feedback
            .lock()
            .expect("memory router mutex poisoned")
            .clone()
    }

    /// Layer 1 baseline for session start. Never injects Layer 2 opportunities.
    pub fn baseline_context(
        &self,
        request: ContextProviderRequest,
        scope: &MemoryScopeRef,
        now_unix_ms: u64,
        budget: usize,
    ) -> AgentResult<ContextProviderResult> {
        match self.baseline_policy() {
            MemoryBaselinePolicy::Disabled => Ok(ContextProviderResult {
                provider_id: request.provider_id,
                summary: String::new(),
                details: None,
                estimated_tokens: 0,
                estimated_bytes: 0,
                priority: 0,
                required: false,
            }),
            MemoryBaselinePolicy::ScopeSelected | MemoryBaselinePolicy::ProfileSelected { .. } => {
                let active = self
                    .inner
                    .active
                    .lock()
                    .expect("memory router mutex poisoned")
                    .clone();
                let mut entries: Vec<_> = self
                    .inner
                    .entries
                    .lock()
                    .expect("memory router mutex poisoned")
                    .values()
                    .filter(|entry| entry.enabled && !is_expired(entry, now_unix_ms))
                    .filter(|entry| scope_matches(scope, &entry.scope))
                    .filter(|entry| active.is_empty() || active.contains(&entry.memory_id))
                    .cloned()
                    .collect();
                entries.sort_by(|left, right| {
                    right
                        .priority
                        .cmp(&left.priority)
                        .then_with(|| left.memory_id.cmp(&right.memory_id))
                });
                entries.truncate(budget.max(1));
                let summary = entries
                    .iter()
                    .map(|entry| entry.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                let details = entries.first().and_then(|entry| entry.resource.clone());
                Ok(ContextProviderResult {
                    provider_id: request.provider_id,
                    summary,
                    details,
                    estimated_tokens: entries.len() as u64 * 32,
                    estimated_bytes: entries.iter().map(|entry| entry.text.len() as u64).sum(),
                    priority: 10,
                    required: false,
                })
            }
        }
    }

    /// Layer 2 candidates as hints only unless mode is AutoInject.
    pub fn opportunity_context(
        &self,
        opportunity: &MemoryOpportunity,
    ) -> AgentResult<ContextProviderResult> {
        match opportunity.mode {
            MemoryOpportunityMode::AutoInject => {
                let summary = opportunity
                    .candidates
                    .iter()
                    .map(|candidate| candidate.entry.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(ContextProviderResult {
                    provider_id: "memory.opportunity".into(),
                    summary: summary.clone(),
                    details: opportunity
                        .candidates
                        .first()
                        .and_then(|candidate| candidate.entry.resource.clone()),
                    estimated_tokens: opportunity.candidates.len() as u64 * 24,
                    estimated_bytes: summary.len() as u64,
                    priority: 5,
                    required: false,
                })
            }
            MemoryOpportunityMode::HintOnly
            | MemoryOpportunityMode::Ask
            | MemoryOpportunityMode::AgentDecides => {
                let summary = opportunity
                    .candidates
                    .iter()
                    .map(|candidate| {
                        format!(
                            "hint:{}:{}",
                            candidate.entry.memory_id, candidate.entry.priority
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                Ok(ContextProviderResult {
                    provider_id: "memory.opportunity.hint".into(),
                    summary: summary.clone(),
                    details: None,
                    estimated_tokens: 8,
                    estimated_bytes: summary.len() as u64,
                    priority: 1,
                    required: false,
                })
            }
        }
    }

    fn entry(&self, memory_id: &str) -> AgentResult<MemoryEntry> {
        self.inner
            .entries
            .lock()
            .expect("memory router mutex poisoned")
            .get(memory_id)
            .cloned()
            .ok_or_else(|| AgentError::not_found(format!("memory `{memory_id}` not found")))
    }
}

fn validate_scope(scope: &MemoryScopeRef) -> AgentResult<()> {
    if scope.namespace.trim().is_empty() || scope.scope_id.trim().is_empty() {
        return Err(AgentError::invalid_input(
            "memory scope namespace and scope_id are required",
        ));
    }
    Ok(())
}

fn scope_matches(expected: &MemoryScopeRef, actual: &MemoryScopeRef) -> bool {
    expected.namespace == actual.namespace && expected.scope_id == actual.scope_id
}

fn is_expired(entry: &MemoryEntry, now_unix_ms: u64) -> bool {
    entry
        .expiry_unix_ms
        .is_some_and(|expiry| now_unix_ms > 0 && now_unix_ms >= expiry)
}

fn score_text(text: &str, query: &str) -> f32 {
    if text.contains(query) {
        return 1.0;
    }
    let terms = query.split_whitespace().collect::<Vec<_>>();
    if terms.is_empty() {
        return 0.0;
    }
    terms.iter().filter(|term| text.contains(*term)).count() as f32 / terms.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_agent_contracts::MemoryAdoptionDecision;

    fn scope(ns: &str, id: &str) -> MemoryScopeRef {
        MemoryScopeRef {
            namespace: ns.into(),
            scope_id: id.into(),
        }
    }

    #[test]
    fn manual_memory_lifecycle_and_revision() {
        let router = MemoryRouter::default();
        let written = router
            .write(AgentMemoryWriteRequest {
                text: "prefers rust".into(),
                tags: vec!["pref".into()],
                metadata: None,
                scope: Some(scope("tenant", "user-1")),
                priority: Some(3),
                confidence: Some(0.9),
                expiry_unix_ms: None,
                provenance: None,
                details_ref: None,
            })
            .unwrap();
        router
            .activate(AgentMemoryActivateRequest {
                memory_id: written.memory_id.clone(),
            })
            .unwrap();
        let revised = router
            .revise(AgentMemoryReviseRequest {
                memory_id: written.memory_id.clone(),
                text: "prefers rust 2024".into(),
                tags: None,
                metadata: None,
                details_ref: None,
            })
            .unwrap();
        assert_eq!(revised.text, "prefers rust 2024");
        router
            .disable(AgentMemoryDisableRequest {
                memory_id: written.memory_id.clone(),
            })
            .unwrap();
        let queried = router
            .query(AgentMemoryQueryRequest {
                query: "rust".into(),
                limit: 8,
                tags: Vec::new(),
                scope: Some(scope("tenant", "user-1")),
                include_disabled: false,
                now_unix_ms: None,
            })
            .unwrap();
        assert!(queried.records.is_empty());
        router
            .delete(AgentMemoryDeleteRequest {
                memory_id: written.memory_id,
            })
            .unwrap();
    }

    #[test]
    fn opportunity_is_hint_only_by_default_and_feedback_is_recorded() {
        let router = MemoryRouter::default();
        router
            .write(AgentMemoryWriteRequest {
                text: "uses dark theme".into(),
                tags: vec!["ui".into()],
                metadata: None,
                scope: Some(scope("tenant", "user-1")),
                priority: Some(1),
                confidence: None,
                expiry_unix_ms: None,
                provenance: None,
                details_ref: None,
            })
            .unwrap();
        let opportunity = router
            .detect_opportunity(
                "session",
                "turn",
                MemoryQuery {
                    query: "theme".into(),
                    scope: scope("tenant", "user-1"),
                    limit: 4,
                    tags: Vec::new(),
                    now_unix_ms: None,
                },
                MemoryOpportunityMode::HintOnly,
            )
            .unwrap();
        let hint = router.opportunity_context(&opportunity).unwrap();
        assert!(hint.summary.starts_with("hint:"));
        assert!(hint.details.is_none());
        router
            .record_feedback(MemoryAdoptionFeedback {
                opportunity_id: opportunity.opportunity_id,
                memory_id: opportunity.candidates[0].entry.memory_id.clone(),
                decision: MemoryAdoptionDecision::Rejected,
                reason: "not relevant".into(),
                recorded_at_unix_ms: 10,
            })
            .unwrap();
        assert_eq!(router.feedback_log().len(), 1);
    }

    #[test]
    fn scope_isolation_and_expiry_hold() {
        let router = MemoryRouter::default();
        router
            .write(AgentMemoryWriteRequest {
                text: "alpha fact".into(),
                tags: Vec::new(),
                metadata: None,
                scope: Some(scope("tenant", "a")),
                priority: None,
                confidence: None,
                expiry_unix_ms: Some(100),
                provenance: None,
                details_ref: None,
            })
            .unwrap();
        let other = router
            .query(AgentMemoryQueryRequest {
                query: "alpha".into(),
                limit: 8,
                tags: Vec::new(),
                scope: Some(scope("tenant", "b")),
                include_disabled: false,
                now_unix_ms: Some(50),
            })
            .unwrap();
        assert!(other.records.is_empty());
        let expired = router
            .query(AgentMemoryQueryRequest {
                query: "alpha".into(),
                limit: 8,
                tags: Vec::new(),
                scope: Some(scope("tenant", "a")),
                include_disabled: false,
                now_unix_ms: Some(100),
            })
            .unwrap();
        assert!(expired.records.is_empty());
    }
}
