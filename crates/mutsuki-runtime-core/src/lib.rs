use std::collections::{BTreeMap, HashMap, VecDeque};

use mutsuki_runtime_contracts::{
    AgentId, AgentParticipation, AgentPhase, AgentSpec, ERR_AGENT_NOT_FOUND,
    ERR_OPERATION_NOT_FOUND, ERR_RUNTIME_BACKEND_FAILED, ERR_SCOPE_NO_MATCH, Envelope, LeaseToken,
    OperationHandlerKey, OperationSnapshot, OperationStatus, RefDescriptor, ResourceRecord,
    RuntimeError, ScalarValue, SourceSnapshot, SpanStatus, StrategyResult, TraceSpan,
};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
pub enum BackendPayload {
    Json(Value),
    Envelope(Envelope),
}

#[derive(Debug, Error)]
#[error("runtime error: {0:?}")]
pub struct RuntimeFailure(pub Box<RuntimeError>);

impl RuntimeFailure {
    pub fn new(error: RuntimeError) -> Self {
        Self(Box::new(error))
    }

    pub fn error(&self) -> &RuntimeError {
        &self.0
    }
}

pub type RuntimeResult<T> = Result<T, RuntimeFailure>;

pub trait StrategyBackend {
    fn on_awake(&mut self, agent_id: &str) -> RuntimeResult<()>;
    fn on_input(&mut self, agent_id: &str, envelope: &Envelope) -> RuntimeResult<StrategyResult>;
    fn next_step(&mut self, agent_id: &str) -> RuntimeResult<StrategyResult>;
    fn on_stop(&mut self, agent_id: &str) -> RuntimeResult<()>;
}

pub trait OperationBackend {
    fn list_operations(&self, agent_id: &str) -> RuntimeResult<Vec<OperationSnapshot>>;
    fn invoke(
        &mut self,
        agent_id: &str,
        key: &OperationHandlerKey,
        payload: Value,
    ) -> RuntimeResult<BackendPayload>;
    fn operation_status(&self, agent_id: &str, key: &OperationHandlerKey) -> OperationStatus;
}

pub trait ResourceBackend {
    fn list_records(&self) -> Vec<ResourceRecord>;
}

pub trait RuntimeBackend: StrategyBackend + OperationBackend {}

impl<T> RuntimeBackend for T where T: StrategyBackend + OperationBackend {}

#[derive(Clone, Debug)]
pub struct ResourceGate {
    records: HashMap<String, ResourceRecord>,
    leases: HashMap<String, LeaseToken>,
}

impl Default for ResourceGate {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceGate {
    pub fn new() -> Self {
        Self {
            records: HashMap::new(),
            leases: HashMap::new(),
        }
    }

    pub fn register(&mut self, descriptor: RefDescriptor, owner: impl Into<String>) -> String {
        let ref_id = descriptor.ref_id.clone();
        self.records.insert(
            ref_id.clone(),
            ResourceRecord {
                descriptor,
                owner: owner.into(),
                lease_count: 0,
            },
        );
        ref_id
    }

    pub fn acquire(
        &mut self,
        ref_id: &str,
        requester: impl Into<String>,
    ) -> RuntimeResult<LeaseToken> {
        let record = self.records.get_mut(ref_id).ok_or_else(|| {
            RuntimeFailure::new(error(
                "ref.not_found",
                "runtime.resource_gate",
                format!("runtime.resource.acquire.{ref_id}"),
            ))
        })?;
        record.lease_count += 1;
        let token = LeaseToken {
            token_id: format!("lease-{}", Uuid::new_v4()),
            ref_id: ref_id.to_string(),
            owner: requester.into(),
        };
        self.leases.insert(token.token_id.clone(), token.clone());
        Ok(token)
    }

    pub fn release(&mut self, token: &LeaseToken) -> RuntimeResult<()> {
        let stored = self.leases.get(&token.token_id).ok_or_else(|| {
            RuntimeFailure::new(error(
                "ref.not_found",
                "runtime.resource_gate",
                format!("runtime.resource.release.{}", token.token_id),
            ))
        })?;
        if stored != token {
            let mut err = error(
                "ref.not_found",
                "runtime.resource_gate",
                format!("runtime.resource.release.{}", token.token_id),
            );
            err.evidence.insert(
                "reason".into(),
                ScalarValue::String("lease_token_mismatch".into()),
            );
            err.evidence.insert(
                "token_id".into(),
                ScalarValue::String(token.token_id.clone()),
            );
            err.evidence.insert(
                "expected_ref_id".into(),
                ScalarValue::String(stored.ref_id.clone()),
            );
            err.evidence.insert(
                "actual_ref_id".into(),
                ScalarValue::String(token.ref_id.clone()),
            );
            err.evidence.insert(
                "expected_owner".into(),
                ScalarValue::String(stored.owner.clone()),
            );
            err.evidence.insert(
                "actual_owner".into(),
                ScalarValue::String(token.owner.clone()),
            );
            return Err(RuntimeFailure::new(err));
        }
        let removed = self
            .leases
            .remove(&token.token_id)
            .expect("lease exists after prior lookup");
        if let Some(record) = self.records.get_mut(&removed.ref_id) {
            record.lease_count = record.lease_count.saturating_sub(1);
        }
        Ok(())
    }

    pub fn list_records(&self) -> Vec<ResourceRecord> {
        let mut records: Vec<ResourceRecord> = self.records.values().cloned().collect();
        records.sort_by(|a, b| a.descriptor.ref_id.cmp(&b.descriptor.ref_id));
        records
    }
}

#[derive(Clone, Debug)]
pub struct AgentState {
    pub spec: AgentSpec,
    pub phase: AgentPhase,
    pub inbox: VecDeque<Envelope>,
}

#[derive(Clone, Debug, Default)]
pub struct TraceBook {
    spans: Vec<TraceSpan>,
    next_span: u64,
}

impl TraceBook {
    fn record(
        &mut self,
        agent_id: &str,
        name: impl Into<String>,
        parent_span_id: Option<String>,
        status: SpanStatus,
    ) -> TraceSpan {
        self.next_span += 1;
        let mut attributes = BTreeMap::new();
        attributes.insert(
            "agent_id".to_string(),
            ScalarValue::String(agent_id.to_string()),
        );
        let span = TraceSpan {
            trace_id: format!("trace-{agent_id}"),
            span_id: format!("span-{}", self.next_span),
            parent_span_id,
            name: name.into(),
            start: self.next_span as f64,
            end: Some(self.next_span as f64),
            attributes,
            status,
        };
        self.spans.push(span.clone());
        span
    }

    pub fn spans(&self) -> &[TraceSpan] {
        &self.spans
    }
}

#[derive(Clone, Debug, Default)]
pub struct AgentRuntime {
    agents: HashMap<AgentId, AgentState>,
    operation_registry: HashMap<AgentId, HashMap<String, OperationSnapshot>>,
    source_registry: HashMap<AgentId, Vec<SourceSnapshot>>,
    resource_gate: ResourceGate,
    trace: TraceBook,
}

impl AgentRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_agent(&mut self, spec: AgentSpec) -> RuntimeResult<()> {
        let agent_id = spec.agent_id.clone();
        self.agents.insert(
            agent_id,
            AgentState {
                spec,
                phase: AgentPhase::Spawn,
                inbox: VecDeque::new(),
            },
        );
        Ok(())
    }

    pub fn start_agent<B: RuntimeBackend>(
        &mut self,
        agent_id: &str,
        backend: &mut B,
    ) -> RuntimeResult<()> {
        self.agent(agent_id)?;
        if let Err(err) = backend.on_awake(agent_id) {
            self.trace
                .record(agent_id, "agent.awake", None, SpanStatus::Error);
            return Err(err);
        }
        if let Err(err) = self.refresh_operations(agent_id, backend) {
            self.trace
                .record(agent_id, "agent.awake", None, SpanStatus::Error);
            return Err(err);
        }
        let agent = self.agent_mut(agent_id)?;
        agent.phase = AgentPhase::Awake;
        self.trace
            .record(agent_id, "agent.awake", None, SpanStatus::Ok);
        Ok(())
    }

    pub fn stop_agent<B: RuntimeBackend>(
        &mut self,
        agent_id: &str,
        backend: &mut B,
    ) -> RuntimeResult<()> {
        let agent = self.agent_mut(agent_id)?;
        agent.phase = AgentPhase::Sleep;
        self.trace
            .record(agent_id, "agent.sleep", None, SpanStatus::Ok);
        backend.on_stop(agent_id)?;
        let agent = self.agent_mut(agent_id)?;
        agent.phase = AgentPhase::Stop;
        self.trace
            .record(agent_id, "agent.stop", None, SpanStatus::Ok);
        Ok(())
    }

    pub fn publish(&mut self, envelope: Envelope) -> RuntimeResult<Vec<AgentId>> {
        let mut matched = Vec::new();
        for (agent_id, agent) in &mut self.agents {
            if agent.phase != AgentPhase::Awake {
                continue;
            }
            if agent
                .spec
                .accepts
                .iter()
                .any(|rule| rule.matches(&envelope))
            {
                agent.inbox.push_back(envelope.clone());
                matched.push(agent_id.clone());
            }
        }
        if matched.is_empty() {
            self.trace
                .record("runtime", "runtime.scope_no_match", None, SpanStatus::Error);
        }
        Ok(matched)
    }

    pub fn select_accepting(&self, envelope: &Envelope) -> Option<AgentId> {
        let mut candidates: Vec<&AgentState> = self
            .agents
            .values()
            .filter(|agent| {
                agent.phase == AgentPhase::Awake
                    && agent.spec.participation == AgentParticipation::PrimaryCandidate
                    && agent.spec.accepts.iter().any(|rule| rule.matches(envelope))
            })
            .collect();
        candidates.sort_by(|a, b| {
            b.spec
                .priority
                .cmp(&a.spec.priority)
                .then_with(|| a.spec.agent_id.cmp(&b.spec.agent_id))
        });
        candidates.first().map(|agent| agent.spec.agent_id.clone())
    }

    pub fn tick_once<B: RuntimeBackend>(
        &mut self,
        agent_id: &str,
        backend: &mut B,
    ) -> RuntimeResult<StrategyResult> {
        let envelope = {
            let agent = self.agent_mut(agent_id)?;
            agent.inbox.pop_front()
        };
        let result = match envelope {
            Some(envelope) => {
                self.trace
                    .record(agent_id, "agent.input", None, SpanStatus::Ok);
                backend.on_input(agent_id, &envelope)?
            }
            None => backend.next_step(agent_id)?,
        };
        if result.error.is_some() {
            self.trace
                .record(agent_id, "agent.strategy", None, SpanStatus::Error);
        } else {
            self.trace
                .record(agent_id, "agent.strategy", None, SpanStatus::Ok);
        }
        for emitted in &result.emitted {
            self.publish(emitted.clone())?;
        }
        Ok(result)
    }

    pub fn refresh_operations<B: OperationBackend>(
        &mut self,
        agent_id: &str,
        backend: &B,
    ) -> RuntimeResult<()> {
        self.agent(agent_id)?;
        let snapshots = backend.list_operations(agent_id)?;
        self.operation_registry.insert(
            agent_id.to_string(),
            snapshots
                .into_iter()
                .map(|snapshot| (snapshot.descriptor.op_id.clone(), snapshot))
                .collect(),
        );
        Ok(())
    }

    pub fn invoke_operation<B: OperationBackend>(
        &mut self,
        agent_id: &str,
        op_id: &str,
        payload: Value,
        backend: &mut B,
    ) -> RuntimeResult<BackendPayload> {
        let registry = self.operation_registry.get(agent_id).ok_or_else(|| {
            RuntimeFailure::new(error(
                ERR_OPERATION_NOT_FOUND,
                "runtime.operation_registry",
                format!("runtime.invoke.{agent_id}.{op_id}"),
            ))
        })?;
        let snapshot = registry.get(op_id).ok_or_else(|| {
            RuntimeFailure::new(error(
                ERR_OPERATION_NOT_FOUND,
                "runtime.operation_registry",
                format!("runtime.invoke.{agent_id}.{op_id}"),
            ))
        })?;
        if snapshot.status != OperationStatus::Active {
            let mut err = error(
                ERR_RUNTIME_BACKEND_FAILED,
                &snapshot.descriptor.plugin_id,
                format!("runtime.invoke.{agent_id}.{op_id}"),
            );
            err.evidence.insert(
                "operation_status".into(),
                ScalarValue::String(format!("{:?}", snapshot.status)),
            );
            return Err(RuntimeFailure::new(err));
        }
        self.trace
            .record(agent_id, "operation.invoke", None, SpanStatus::Ok);
        backend.invoke(agent_id, &snapshot.key, payload)
    }

    pub fn ingest_sources(&mut self, agent_id: &str, sources: Vec<SourceSnapshot>) {
        self.source_registry.insert(agent_id.to_string(), sources);
    }

    pub fn phase(&self, agent_id: &str) -> Option<&AgentPhase> {
        self.agents.get(agent_id).map(|agent| &agent.phase)
    }

    pub fn inbox_len(&self, agent_id: &str) -> Option<usize> {
        self.agents.get(agent_id).map(|agent| agent.inbox.len())
    }

    pub fn operation_snapshot(&self, agent_id: &str, op_id: &str) -> Option<&OperationSnapshot> {
        self.operation_registry.get(agent_id)?.get(op_id)
    }

    pub fn trace_spans(&self) -> &[TraceSpan] {
        self.trace.spans()
    }

    pub fn resources(&self) -> &ResourceGate {
        &self.resource_gate
    }

    pub fn resources_mut(&mut self) -> &mut ResourceGate {
        &mut self.resource_gate
    }

    fn agent(&self, agent_id: &str) -> RuntimeResult<&AgentState> {
        self.agents.get(agent_id).ok_or_else(|| {
            RuntimeFailure::new(error(
                ERR_AGENT_NOT_FOUND,
                "runtime.agent",
                format!("runtime.agent.{agent_id}"),
            ))
        })
    }

    fn agent_mut(&mut self, agent_id: &str) -> RuntimeResult<&mut AgentState> {
        self.agents.get_mut(agent_id).ok_or_else(|| {
            RuntimeFailure::new(error(
                ERR_AGENT_NOT_FOUND,
                "runtime.agent",
                format!("runtime.agent.{agent_id}"),
            ))
        })
    }
}

fn error(
    code: impl Into<String>,
    source: impl Into<String>,
    route: impl Into<String>,
) -> RuntimeError {
    RuntimeError {
        code: code.into(),
        source: source.into(),
        route: route.into(),
        lost_capability: None,
        recovery: None,
        cause: None,
        evidence: BTreeMap::new(),
    }
}

pub fn scope_no_match_error() -> RuntimeError {
    error(ERR_SCOPE_NO_MATCH, "runtime.route", "runtime.publish")
}

#[cfg(test)]
mod tests {
    use mutsuki_runtime_contracts::{
        OperationDescriptor, ScopeRuleSpec, SourceRef, StrategyResultStatus,
    };
    use serde_json::json;

    use super::*;

    #[derive(Default)]
    struct NativeBackend {
        awake: usize,
        stopped: usize,
        inputs: usize,
        invocations: usize,
        operations: Vec<OperationSnapshot>,
        fail_list_operations: bool,
        fail_awake: bool,
    }

    impl StrategyBackend for NativeBackend {
        fn on_awake(&mut self, _agent_id: &str) -> RuntimeResult<()> {
            if self.fail_awake {
                return Err(RuntimeFailure::new(error(
                    ERR_RUNTIME_BACKEND_FAILED,
                    "native",
                    "native.on_awake",
                )));
            }
            self.awake += 1;
            Ok(())
        }

        fn on_input(
            &mut self,
            _agent_id: &str,
            _envelope: &Envelope,
        ) -> RuntimeResult<StrategyResult> {
            self.inputs += 1;
            Ok(StrategyResult {
                status: StrategyResultStatus::Completed,
                decision: None,
                emitted: Vec::new(),
                error: None,
            })
        }

        fn next_step(&mut self, _agent_id: &str) -> RuntimeResult<StrategyResult> {
            Ok(StrategyResult::wait_input())
        }

        fn on_stop(&mut self, _agent_id: &str) -> RuntimeResult<()> {
            self.stopped += 1;
            Ok(())
        }
    }

    impl OperationBackend for NativeBackend {
        fn list_operations(&self, _agent_id: &str) -> RuntimeResult<Vec<OperationSnapshot>> {
            if self.fail_list_operations {
                return Err(RuntimeFailure::new(error(
                    ERR_RUNTIME_BACKEND_FAILED,
                    "native",
                    "native.list_operations",
                )));
            }
            Ok(self.operations.clone())
        }

        fn invoke(
            &mut self,
            _agent_id: &str,
            _key: &OperationHandlerKey,
            payload: Value,
        ) -> RuntimeResult<BackendPayload> {
            self.invocations += 1;
            Ok(BackendPayload::Json(payload))
        }

        fn operation_status(&self, _agent_id: &str, _key: &OperationHandlerKey) -> OperationStatus {
            OperationStatus::Active
        }
    }

    fn envelope() -> Envelope {
        Envelope {
            id: "env-1".into(),
            timestamp: 0.0,
            source: SourceRef {
                source_id: "source:default".into(),
                kind: "test".into(),
                metadata: BTreeMap::new(),
            },
            payload_schema_id: "test.input".into(),
            capabilities_required: vec!["test.cap".into()],
            payload: Value::Null,
        }
    }

    fn agent(agent_id: &str, priority: i64) -> AgentSpec {
        AgentSpec {
            agent_id: agent_id.into(),
            owner: None,
            priority,
            participation: AgentParticipation::PrimaryCandidate,
            accepts: vec![ScopeRuleSpec::BySchemaPrefix {
                prefix: "test.".into(),
            }],
            strategy_id: "native".into(),
            side_effect_policy: mutsuki_runtime_contracts::SideEffectPolicy::ReadOnly,
        }
    }

    #[test]
    fn runtime_routes_and_ticks_agent_input() {
        let mut runtime = AgentRuntime::new();
        let mut backend = NativeBackend::default();
        runtime.register_agent(agent("agent-a", 0)).unwrap();
        runtime.start_agent("agent-a", &mut backend).unwrap();

        let matched = runtime.publish(envelope()).unwrap();
        assert_eq!(matched, vec!["agent-a".to_string()]);
        assert_eq!(runtime.inbox_len("agent-a"), Some(1));

        let result = runtime.tick_once("agent-a", &mut backend).unwrap();
        assert_eq!(result.status, StrategyResultStatus::Completed);
        assert_eq!(backend.inputs, 1);
        assert!(
            runtime
                .trace_spans()
                .iter()
                .any(|s| s.name == "agent.input")
        );
    }

    #[test]
    fn runtime_selects_primary_candidate_by_priority_then_id() {
        let mut runtime = AgentRuntime::new();
        let mut backend = NativeBackend::default();
        runtime.register_agent(agent("agent-b", 1)).unwrap();
        runtime.register_agent(agent("agent-a", 1)).unwrap();
        runtime.start_agent("agent-a", &mut backend).unwrap();
        runtime.start_agent("agent-b", &mut backend).unwrap();

        assert_eq!(
            runtime.select_accepting(&envelope()),
            Some("agent-a".into())
        );
    }

    #[test]
    fn runtime_invokes_operation_through_backend_key() {
        let mut runtime = AgentRuntime::new();
        let mut backend = NativeBackend::default();
        backend.operations.push(OperationSnapshot {
            descriptor: OperationDescriptor {
                op_id: "test.noop".into(),
                name: "noop".into(),
                description: String::new(),
                plugin_id: "test".into(),
                func_qualname: String::new(),
                parameters_schema: json!({}),
                return_schema: json!({}),
                perms_rule_id: None,
                requires_capabilities: Vec::new(),
                is_tool: true,
            },
            status: OperationStatus::Active,
            key: OperationHandlerKey {
                plugin_id: "test".into(),
                plugin_generation: 0,
                op_id: "test.noop".into(),
                handler_id: "test:test.noop:0".into(),
            },
        });
        runtime.register_agent(agent("agent-a", 0)).unwrap();
        runtime.start_agent("agent-a", &mut backend).unwrap();

        let result = runtime
            .invoke_operation("agent-a", "test.noop", json!({"ok": true}), &mut backend)
            .unwrap();
        assert_eq!(result, BackendPayload::Json(json!({"ok": true})));
        assert_eq!(backend.invocations, 1);
    }

    #[test]
    fn runtime_native_backend_smoke_without_external_plugin_host() {
        let mut runtime = AgentRuntime::new();
        let mut backend = NativeBackend::default();
        runtime.register_agent(agent("native-agent", 0)).unwrap();
        runtime.start_agent("native-agent", &mut backend).unwrap();
        let result = runtime.tick_once("native-agent", &mut backend).unwrap();
        assert_eq!(result.status, StrategyResultStatus::WaitInput);
        runtime.stop_agent("native-agent", &mut backend).unwrap();
        assert_eq!(runtime.phase("native-agent"), Some(&AgentPhase::Stop));
        assert_eq!(backend.awake, 1);
        assert_eq!(backend.stopped, 1);
    }

    #[test]
    fn resource_gate_tracks_descriptor_leases_without_handles() {
        let mut gate = ResourceGate::new();
        let ref_id = gate.register(
            RefDescriptor {
                ref_id: "ref-1".into(),
                kind: "domain.resource".into(),
                schema_id_target: "domain.resource".into(),
                schema_version_target: "1.0.0".into(),
                attributes: BTreeMap::new(),
                lineage: Vec::new(),
            },
            "backend",
        );

        let lease = gate.acquire(&ref_id, "agent-a").unwrap();
        assert_eq!(lease.ref_id, "ref-1");
        assert_eq!(gate.list_records()[0].lease_count, 1);
        assert!(!lease.token_id.starts_with("lease-1"));

        gate.release(&lease).unwrap();
        assert_eq!(gate.list_records()[0].lease_count, 0);
    }

    #[test]
    fn failed_awake_does_not_commit_agent_to_routing() {
        let mut runtime = AgentRuntime::new();
        let mut backend = NativeBackend {
            fail_awake: true,
            ..NativeBackend::default()
        };
        runtime.register_agent(agent("agent-a", 0)).unwrap();

        let err = runtime.start_agent("agent-a", &mut backend).unwrap_err();
        assert_eq!(err.error().code, ERR_RUNTIME_BACKEND_FAILED);
        assert_eq!(runtime.phase("agent-a"), Some(&AgentPhase::Spawn));
        assert_eq!(runtime.publish(envelope()).unwrap(), Vec::<String>::new());
        assert_eq!(runtime.inbox_len("agent-a"), Some(0));
        assert!(
            runtime
                .trace_spans()
                .iter()
                .any(|span| span.name == "agent.awake" && span.status == SpanStatus::Error)
        );
    }

    #[test]
    fn failed_operation_refresh_does_not_commit_agent_to_routing() {
        let mut runtime = AgentRuntime::new();
        let mut backend = NativeBackend {
            fail_list_operations: true,
            ..NativeBackend::default()
        };
        runtime.register_agent(agent("agent-a", 0)).unwrap();

        let err = runtime.start_agent("agent-a", &mut backend).unwrap_err();
        assert_eq!(err.error().code, ERR_RUNTIME_BACKEND_FAILED);
        assert_eq!(runtime.phase("agent-a"), Some(&AgentPhase::Spawn));
        assert_eq!(runtime.publish(envelope()).unwrap(), Vec::<String>::new());
        assert_eq!(runtime.inbox_len("agent-a"), Some(0));
        assert!(runtime.operation_snapshot("agent-a", "test.noop").is_none());
        assert!(
            runtime
                .trace_spans()
                .iter()
                .any(|span| span.name == "agent.awake" && span.status == SpanStatus::Error)
        );
    }

    #[test]
    fn resource_gate_rejects_forged_lease_token_without_releasing() {
        let mut gate = ResourceGate::new();
        let ref_id = gate.register(
            RefDescriptor {
                ref_id: "ref-1".into(),
                kind: "domain.resource".into(),
                schema_id_target: "domain.resource".into(),
                schema_version_target: "1.0.0".into(),
                attributes: BTreeMap::new(),
                lineage: Vec::new(),
            },
            "backend",
        );

        let lease = gate.acquire(&ref_id, "agent-a").unwrap();
        let forged = LeaseToken {
            token_id: lease.token_id.clone(),
            ref_id: "ref-other".into(),
            owner: "agent-b".into(),
        };

        let err = gate.release(&forged).unwrap_err();
        assert_eq!(err.error().code, "ref.not_found");
        assert_eq!(
            err.error().evidence.get("reason"),
            Some(&ScalarValue::String("lease_token_mismatch".into()))
        );
        assert_eq!(gate.list_records()[0].lease_count, 1);

        gate.release(&lease).unwrap();
        assert_eq!(gate.list_records()[0].lease_count, 0);
    }
}
