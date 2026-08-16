use std::collections::{BTreeMap, BTreeSet};

use mutsuki_agent_contracts::{
    AGENT_CONTEXT_PROVIDER_COLLECT_PROTOCOL, AgentError, ContextBudget, ContextDecision,
    ContextDecisionKind, ContextItemRef, ContextPlan, ContextProvenance,
    ContextProviderBatchRequest, ContextProviderRequest, ContextProviderResult,
};
use mutsuki_runtime_contracts::{Task, TaskBatch};

use crate::ContextPlanner;

pub const LARGE_CONTEXT_RESULT_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug)]
pub struct ContextProviderCompletion {
    pub provider_id: String,
    pub result: Result<ContextProviderResult, AgentError>,
}

#[derive(Clone, Debug)]
pub struct ContextProviderBatchPlan {
    pub request: ContextProviderBatchRequest,
    pub batch: TaskBatch,
}

impl ContextProviderBatchPlan {
    pub fn build(request: ContextProviderBatchRequest) -> Result<Self, AgentError> {
        if request.session_id.trim().is_empty() || request.turn_id.trim().is_empty() {
            return Err(AgentError::invalid_input(
                "context provider batch requires session and turn ids",
            ));
        }
        let mut provider_ids = BTreeSet::new();
        let mut tasks = Vec::with_capacity(request.providers.len());
        for provider in &request.providers {
            if provider.provider_id.trim().is_empty()
                || !provider_ids.insert(provider.provider_id.as_str())
            {
                return Err(AgentError::invalid_input(
                    "context provider ids must be non-empty and unique",
                ));
            }
            let payload = serde_json::to_value(ContextProviderRequest {
                session_id: request.session_id.clone(),
                turn_id: request.turn_id.clone(),
                provider_id: provider.provider_id.clone(),
                input: provider.input.clone(),
            })
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
            let mut task = Task::new(
                format!(
                    "{}:{}:context:{}",
                    request.session_id, request.turn_id, provider.provider_id
                ),
                AGENT_CONTEXT_PROVIDER_COLLECT_PROTOCOL,
                payload,
            );
            task.correlation_id = Some(request.turn_id.clone());
            task.runner_hint = Some(provider.provider_id.clone());
            tasks.push(task);
        }
        let batch = TaskBatch {
            batch_id: format!(
                "{}:{}:context-providers",
                request.session_id, request.turn_id
            )
            .into(),
            tick_id: Some(format!("{}:{}", request.session_id, request.turn_id).into()),
            tasks,
            resource_plan: None,
        };
        Ok(Self { request, batch })
    }

    pub fn resolve(
        self,
        budget: ContextBudget,
        completions: Vec<ContextProviderCompletion>,
    ) -> Result<ContextPlan, AgentError> {
        let mut by_provider = BTreeMap::new();
        for completion in completions {
            if by_provider
                .insert(completion.provider_id.clone(), completion.result)
                .is_some()
            {
                return Err(AgentError::invalid_input(
                    "context provider completions must be unique",
                ));
            }
        }

        let mut items = Vec::new();
        let mut decisions = Vec::new();
        for provider in &self.request.providers {
            let item_id = format!("provider:{}", provider.provider_id);
            let completion = by_provider.remove(&provider.provider_id);
            match completion {
                Some(Ok(result)) => {
                    if result.provider_id != provider.provider_id
                        || result.required != provider.required
                    {
                        return Err(AgentError::invalid_input(format!(
                            "context provider `{}` returned mismatched identity or requirement",
                            provider.provider_id
                        )));
                    }
                    if result.summary.trim().is_empty() {
                        return Err(AgentError::invalid_input(format!(
                            "context provider `{}` returned an empty summary",
                            provider.provider_id
                        )));
                    }
                    if result.estimated_bytes > LARGE_CONTEXT_RESULT_BYTES
                        && result.details.is_none()
                    {
                        return Err(AgentError::new(
                            "agent.context.resource_ref_required",
                            format!(
                                "large context result from `{}` requires a ResourceRef",
                                provider.provider_id
                            ),
                        ));
                    }
                    let estimated_bytes = if result.estimated_bytes == 0 {
                        result.summary.len() as u64
                    } else {
                        result.estimated_bytes
                    };
                    items.push(ContextItemRef {
                        item_id,
                        category: "provider".into(),
                        summary: result.summary,
                        details: result.details,
                        priority: provider.priority.clone(),
                        estimated_tokens: result.estimated_tokens,
                        estimated_bytes,
                        provenance: ContextProvenance {
                            provider_id: provider.provider_id.clone(),
                            source_kind: "context_provider".into(),
                            source_id: provider.provider_id.clone(),
                            source_version: self.request.session_version.0.to_string(),
                        },
                        required: provider.required,
                    });
                }
                Some(Err(error)) if provider.required => {
                    return Err(required_provider_failure(&provider.provider_id, error));
                }
                None if provider.required => {
                    return Err(required_provider_failure(
                        &provider.provider_id,
                        AgentError::provider_unavailable("provider did not complete"),
                    ));
                }
                Some(Err(error)) => decisions.push(ContextDecision {
                    item_id,
                    decision: ContextDecisionKind::DroppedOptionalProviderFailure,
                    reason: format!("optional provider failed: {}", error.code),
                }),
                None => decisions.push(ContextDecision {
                    item_id,
                    decision: ContextDecisionKind::DroppedOptionalProviderFailure,
                    reason: "optional provider did not complete".into(),
                }),
            }
        }
        if let Some(provider_id) = by_provider.keys().next() {
            return Err(AgentError::invalid_input(format!(
                "unexpected context provider completion `{provider_id}`"
            )));
        }

        ContextPlanner.plan_with_decisions(
            self.request.session_id,
            self.request.turn_id,
            self.request.session_version,
            budget,
            items,
            decisions,
        )
    }
}

fn required_provider_failure(provider_id: &str, error: AgentError) -> AgentError {
    AgentError::new(
        "agent.context.required_provider_failed",
        format!("required context provider `{provider_id}` failed: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use mutsuki_agent_contracts::{
        ContextPriority, ContextProviderSpec, ResourceRef, SessionVersion,
    };
    use mutsuki_runtime_contracts::{
        ResourceAccess, ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic,
    };
    use serde_json::json;

    use super::*;

    fn request() -> ContextProviderBatchRequest {
        ContextProviderBatchRequest {
            session_id: "session".into(),
            turn_id: "turn".into(),
            session_version: SessionVersion(4),
            providers: vec![
                ContextProviderSpec {
                    provider_id: "workspace".into(),
                    priority: ContextPriority::Required,
                    required: true,
                    input: json!({"query": "symbol"}),
                },
                ContextProviderSpec {
                    provider_id: "optional-memory".into(),
                    priority: ContextPriority::Low,
                    required: false,
                    input: json!(null),
                },
            ],
        }
    }

    fn details() -> ResourceRef {
        ResourceRef {
            ref_id: "context:workspace".into(),
            resource_id: ResourceId {
                kind_id: "agent.context".into(),
                slot_id: "workspace".into(),
                generation: 1,
                version: 1,
            },
            semantic: ResourceSemantic::FrozenValue,
            provider_id: "test".into(),
            resource_kind: "agent.context".into(),
            schema: "mutsuki.agent.context.provider-result@1".into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::Inline,
            size_hint: Some(LARGE_CONTEXT_RESULT_BYTES + 1),
            content_hash: Some("sha256:test".into()),
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        }
    }

    #[test]
    fn providers_are_one_runtime_batch_and_optional_failure_is_deterministic() {
        let plan = ContextProviderBatchPlan::build(request()).unwrap();
        assert_eq!(plan.batch.tasks.len(), 2);
        assert!(plan.batch.tasks.iter().all(|task| {
            task.protocol_id == AGENT_CONTEXT_PROVIDER_COLLECT_PROTOCOL
                && task.correlation_id.as_deref() == Some("turn")
        }));
        let resolved = plan
            .resolve(
                ContextBudget {
                    max_tokens: Some(64),
                    max_bytes: Some(128 * 1024),
                    max_items: Some(2),
                },
                vec![
                    ContextProviderCompletion {
                        provider_id: "workspace".into(),
                        result: Ok(ContextProviderResult {
                            provider_id: "workspace".into(),
                            summary: "workspace symbols".into(),
                            details: Some(details()),
                            estimated_tokens: 8,
                            estimated_bytes: LARGE_CONTEXT_RESULT_BYTES + 1,
                            priority: 0,
                            required: true,
                        }),
                    },
                    ContextProviderCompletion {
                        provider_id: "optional-memory".into(),
                        result: Err(AgentError::provider_unavailable("offline")),
                    },
                ],
            )
            .unwrap();
        assert_eq!(resolved.items.len(), 1);
        assert_eq!(
            resolved.decisions[0].decision,
            ContextDecisionKind::DroppedOptionalProviderFailure
        );
        assert!(resolved.items[0].details.is_some());
    }

    #[test]
    fn required_failure_and_large_inline_result_are_rejected() {
        let required = ContextProviderBatchPlan::build(request())
            .unwrap()
            .resolve(
                ContextBudget::default(),
                vec![ContextProviderCompletion {
                    provider_id: "workspace".into(),
                    result: Err(AgentError::provider_unavailable("offline")),
                }],
            )
            .unwrap_err();
        assert_eq!(required.code, "agent.context.required_provider_failed");

        let mut only_required = request();
        only_required.providers.truncate(1);
        let large = ContextProviderBatchPlan::build(only_required)
            .unwrap()
            .resolve(
                ContextBudget::default(),
                vec![ContextProviderCompletion {
                    provider_id: "workspace".into(),
                    result: Ok(ContextProviderResult {
                        provider_id: "workspace".into(),
                        summary: "large".into(),
                        details: None,
                        estimated_tokens: 8,
                        estimated_bytes: LARGE_CONTEXT_RESULT_BYTES + 1,
                        priority: 0,
                        required: true,
                    }),
                }],
            )
            .unwrap_err();
        assert_eq!(large.code, "agent.context.resource_ref_required");
    }
}
