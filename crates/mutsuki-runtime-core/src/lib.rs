use std::collections::{BTreeMap, HashMap, VecDeque};

use mutsuki_runtime_contracts::{
    AgentId, AgentParticipation, AgentPhase, AgentSpec, ERR_AGENT_NOT_FOUND,
    ERR_OPERATION_NOT_FOUND, ERR_RUNTIME_BACKEND_FAILED, ERR_SCOPE_NO_MATCH,
    ERR_SOURCE_UNREGISTERED, Envelope, LeaseToken, OperationHandlerKey, OperationSnapshot,
    OperationStatus, RefDescriptor, ResourceRecord, RuntimeError, ScalarValue, SourceSnapshot,
    SpanStatus, StrategyResult, TraceSpan,
};
use serde_json::Value;
use thiserror::Error;

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
    fn list_sources(&self, agent_id: &str) -> RuntimeResult<Vec<SourceSnapshot>>;
    fn invoke(
        &mut self,
        agent_id: &str,
        key: &OperationHandlerKey,
        payload: Value,
    ) -> RuntimeResult<BackendPayload>;
    fn operation_status(&self, agent_id: &str, key: &OperationHandlerKey) -> OperationStatus;
}

pub trait ResourceBackend {
    fn register_resource(
        &mut self,
        descriptor: RefDescriptor,
        owner: &str,
    ) -> RuntimeResult<String>;
    fn acquire_resource(&mut self, ref_id: &str, requester: &str) -> RuntimeResult<LeaseToken>;
    fn release_resource(&mut self, token: &LeaseToken) -> RuntimeResult<()>;
    fn list_records(&self, owner: Option<&str>) -> Vec<ResourceRecord>;
}

pub trait RuntimeBackend: StrategyBackend + OperationBackend {}

impl<T> RuntimeBackend for T where T: StrategyBackend + OperationBackend {}

pub trait IdSource {
    fn next_id(&mut self, prefix: &str) -> String;
}

#[derive(Clone, Debug, Default)]
pub struct SequentialIdSource {
    next: u64,
}

impl SequentialIdSource {
    pub fn new() -> Self {
        Self::default()
    }
}

impl IdSource for SequentialIdSource {
    fn next_id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}-{:026}", self.next)
    }
}

#[derive(Clone, Debug)]
pub struct ResourceGate {
    records: HashMap<String, ResourceRecord>,
    leases: HashMap<String, LeaseToken>,
    id_source: SequentialIdSource,
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
            id_source: SequentialIdSource::new(),
        }
    }

    pub fn with_id_source(id_source: SequentialIdSource) -> Self {
        Self {
            records: HashMap::new(),
            leases: HashMap::new(),
            id_source,
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
            token_id: self.id_source.next_id("lease"),
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
        self.list_records_for(None)
    }

    pub fn list_records_for(&self, owner: Option<&str>) -> Vec<ResourceRecord> {
        let mut records: Vec<ResourceRecord> = self
            .records
            .values()
            .filter(|record| owner.is_none_or(|target| record.owner == target))
            .cloned()
            .collect();
        records.sort_by(|a, b| a.descriptor.ref_id.cmp(&b.descriptor.ref_id));
        records
    }
}

impl ResourceBackend for ResourceGate {
    fn register_resource(
        &mut self,
        descriptor: RefDescriptor,
        owner: &str,
    ) -> RuntimeResult<String> {
        Ok(self.register(descriptor, owner))
    }

    fn acquire_resource(&mut self, ref_id: &str, requester: &str) -> RuntimeResult<LeaseToken> {
        self.acquire(ref_id, requester)
    }

    fn release_resource(&mut self, token: &LeaseToken) -> RuntimeResult<()> {
        self.release(token)
    }

    fn list_records(&self, owner: Option<&str>) -> Vec<ResourceRecord> {
        self.list_records_for(owner)
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
        let span_id = format!("span-{}", self.next_span);
        let span = TraceSpan {
            trace_id: format!("trace-{agent_id}"),
            span_id,
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
        let operation_snapshots = match backend.list_operations(agent_id) {
            Ok(snapshots) => snapshots,
            Err(err) => {
                self.trace
                    .record(agent_id, "agent.awake", None, SpanStatus::Error);
                return Err(err);
            }
        };
        let source_snapshots = match backend.list_sources(agent_id) {
            Ok(snapshots) => snapshots,
            Err(err) => {
                self.trace
                    .record(agent_id, "agent.awake", None, SpanStatus::Error);
                return Err(err);
            }
        };
        self.operation_registry.insert(
            agent_id.to_string(),
            Self::operation_registry_from(operation_snapshots),
        );
        self.ingest_sources(agent_id, source_snapshots);
        let agent = self.agent_mut(agent_id)?;
        agent.phase = AgentPhase::Awake;
        self.trace
            .record(agent_id, "agent.awake", None, SpanStatus::Ok);
        Ok(())
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
            Self::operation_registry_from(snapshots),
        );
        Ok(())
    }

    pub fn refresh_sources<B: OperationBackend>(
        &mut self,
        agent_id: &str,
        backend: &B,
    ) -> RuntimeResult<()> {
        self.agent(agent_id)?;
        let snapshots = backend.list_sources(agent_id)?;
        self.ingest_sources(agent_id, snapshots);
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
        if !self.has_registered_source(&envelope.source.source_id) {
            self.trace.record(
                "runtime",
                "runtime.source_unregistered",
                None,
                SpanStatus::Error,
            );
            return Err(source_unregistered_failure(&envelope));
        }
        let mut matched = Vec::new();
        for (agent_id, agent) in &mut self.agents {
            if Self::agent_accepts(agent, &envelope) {
                agent.inbox.push_back(envelope.clone());
                matched.push(agent_id.clone());
            }
        }
        if matched.is_empty() {
            self.trace
                .record("runtime", "runtime.scope_no_match", None, SpanStatus::Error);
            return Err(scope_no_match_failure(&envelope));
        }
        Ok(matched)
    }

    pub fn select_accepting(&self, envelope: &Envelope) -> Option<AgentId> {
        if !self.has_registered_source(&envelope.source.source_id) {
            return None;
        }
        let mut candidates: Vec<&AgentState> = self
            .agents
            .values()
            .filter(|agent| {
                Self::agent_accepts(agent, envelope)
                    && agent.spec.participation == AgentParticipation::PrimaryCandidate
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
                let input_span = self
                    .trace
                    .record(agent_id, "agent.input", None, SpanStatus::Ok);
                let result = backend.on_input(agent_id, &envelope)?;
                let status = if result.error.is_some() {
                    SpanStatus::Error
                } else {
                    SpanStatus::Ok
                };
                self.trace
                    .record(agent_id, "agent.strategy", Some(input_span.span_id), status);
                result
            }
            None => {
                let tick_span =
                    self.trace
                        .record(agent_id, "agent.next_step", None, SpanStatus::Ok);
                let result = backend.next_step(agent_id)?;
                let status = if result.error.is_some() {
                    SpanStatus::Error
                } else {
                    SpanStatus::Ok
                };
                self.trace
                    .record(agent_id, "agent.strategy", Some(tick_span.span_id), status);
                result
            }
        };
        for emitted in &result.emitted {
            self.publish(emitted.clone())?;
        }
        Ok(result)
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
        let invoke_span = self
            .trace
            .record(agent_id, "operation.invoke", None, SpanStatus::Ok);
        match backend.invoke(agent_id, &snapshot.key, payload) {
            Ok(result) => Ok(result),
            Err(err) => {
                self.trace.record(
                    agent_id,
                    "operation.invoke.error",
                    Some(invoke_span.span_id),
                    SpanStatus::Error,
                );
                Err(err)
            }
        }
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

    pub fn source_snapshots(&self, agent_id: &str) -> Option<&[SourceSnapshot]> {
        self.source_registry
            .get(agent_id)
            .map(std::vec::Vec::as_slice)
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

    fn has_registered_source(&self, source_id: &str) -> bool {
        self.source_registry.values().any(|sources| {
            sources
                .iter()
                .any(|source| source.descriptor.source_id == source_id)
        })
    }

    fn agent_accepts(agent: &AgentState, envelope: &Envelope) -> bool {
        agent.phase == AgentPhase::Awake
            && agent.spec.accepts.iter().any(|rule| rule.matches(envelope))
    }

    fn operation_registry_from(
        snapshots: Vec<OperationSnapshot>,
    ) -> HashMap<String, OperationSnapshot> {
        snapshots
            .into_iter()
            .map(|snapshot| (snapshot.descriptor.op_id.clone(), snapshot))
            .collect()
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

fn source_unregistered_failure(envelope: &Envelope) -> RuntimeFailure {
    let mut err = error(
        ERR_SOURCE_UNREGISTERED,
        "runtime.source_registry",
        format!("runtime.publish.{}", envelope.source.source_id),
    );
    err.evidence.insert(
        "source_id".into(),
        ScalarValue::String(envelope.source.source_id.clone()),
    );
    RuntimeFailure::new(err)
}

fn scope_no_match_failure(envelope: &Envelope) -> RuntimeFailure {
    let mut err = scope_no_match_error();
    err.route = format!("runtime.publish.{}", envelope.source.source_id);
    err.evidence.insert(
        "source_id".into(),
        ScalarValue::String(envelope.source.source_id.clone()),
    );
    err.evidence.insert(
        "payload_schema_id".into(),
        ScalarValue::String(envelope.payload_schema_id.clone()),
    );
    RuntimeFailure::new(err)
}

#[cfg(test)]
mod tests {
    use mutsuki_runtime_contracts::{
        OperationDescriptor, ScopeRuleSpec, SourceDescriptor, SourceRef, StrategyResultStatus,
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
        sources: Vec<SourceSnapshot>,
        fail_list_operations: bool,
        fail_list_sources: bool,
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

        fn list_sources(&self, _agent_id: &str) -> RuntimeResult<Vec<SourceSnapshot>> {
            if self.fail_list_sources {
                return Err(RuntimeFailure::new(error(
                    ERR_RUNTIME_BACKEND_FAILED,
                    "native",
                    "native.list_sources",
                )));
            }
            Ok(self.sources.clone())
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

    fn backend() -> NativeBackend {
        NativeBackend {
            sources: vec![SourceSnapshot {
                descriptor: SourceDescriptor {
                    source_id: "source:default".into(),
                    kind: "test".into(),
                    capabilities: Vec::new(),
                    description: String::new(),
                },
                plugin_id: "native".into(),
                plugin_generation: 0,
            }],
            ..NativeBackend::default()
        }
    }

    #[test]
    fn runtime_routes_and_ticks_agent_input() {
        let mut runtime = AgentRuntime::new();
        let mut backend = backend();
        runtime.register_agent(agent("agent-a", 0)).unwrap();
        runtime.start_agent("agent-a", &mut backend).unwrap();
        assert_eq!(
            runtime.source_snapshots("agent-a").unwrap()[0]
                .descriptor
                .source_id,
            "source:default"
        );

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
        let input_span = runtime
            .trace_spans()
            .iter()
            .find(|span| span.name == "agent.input")
            .unwrap();
        let strategy_span = runtime
            .trace_spans()
            .iter()
            .find(|span| span.name == "agent.strategy")
            .unwrap();
        assert_eq!(
            strategy_span.parent_span_id.as_deref(),
            Some(input_span.span_id.as_str())
        );
    }

    #[test]
    fn runtime_rejects_unregistered_source_before_routing() {
        let mut runtime = AgentRuntime::new();
        let mut backend = backend();
        runtime.register_agent(agent("agent-a", 0)).unwrap();
        runtime.start_agent("agent-a", &mut backend).unwrap();

        let mut unknown = envelope();
        unknown.source.source_id = "source:unknown".into();

        let err = runtime.publish(unknown).unwrap_err();
        assert_eq!(err.error().code, ERR_SOURCE_UNREGISTERED);
        assert_eq!(runtime.inbox_len("agent-a"), Some(0));
    }

    #[test]
    fn runtime_returns_scope_no_match_for_registered_source_without_accepting_agent() {
        let mut runtime = AgentRuntime::new();
        let mut backend = backend();
        runtime.register_agent(agent("agent-a", 0)).unwrap();
        runtime.start_agent("agent-a", &mut backend).unwrap();

        let mut unmatched = envelope();
        unmatched.payload_schema_id = "other.input".into();

        let err = runtime.publish(unmatched).unwrap_err();
        assert_eq!(err.error().code, ERR_SCOPE_NO_MATCH);
        assert_eq!(runtime.inbox_len("agent-a"), Some(0));
    }

    #[test]
    fn runtime_selects_primary_candidate_by_priority_then_id() {
        let mut runtime = AgentRuntime::new();
        let mut backend = backend();
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
    fn runtime_select_accepting_ignores_unregistered_source() {
        let mut runtime = AgentRuntime::new();
        let mut backend = backend();
        runtime.register_agent(agent("agent-a", 0)).unwrap();
        runtime.start_agent("agent-a", &mut backend).unwrap();

        let mut unknown = envelope();
        unknown.source.source_id = "source:unknown".into();

        assert_eq!(runtime.select_accepting(&unknown), None);
    }

    #[test]
    fn runtime_invokes_operation_through_backend_key() {
        let mut runtime = AgentRuntime::new();
        let mut backend = backend();
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
    fn runtime_returns_structured_error_for_missing_operation() {
        let mut runtime = AgentRuntime::new();
        let mut backend = backend();
        runtime.register_agent(agent("agent-a", 0)).unwrap();
        runtime.start_agent("agent-a", &mut backend).unwrap();

        let err = runtime
            .invoke_operation("agent-a", "test.missing", json!({}), &mut backend)
            .unwrap_err();
        assert_eq!(err.error().code, ERR_OPERATION_NOT_FOUND);
        assert_eq!(backend.invocations, 0);
    }

    #[test]
    fn runtime_native_backend_smoke_without_external_plugin_host() {
        let mut runtime = AgentRuntime::new();
        let mut backend = backend();
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
        assert_eq!(lease.token_id, "lease-00000000000000000000000001");
        assert_eq!(gate.list_records()[0].lease_count, 1);

        gate.release(&lease).unwrap();
        assert_eq!(gate.list_records()[0].lease_count, 0);
    }

    #[test]
    fn resource_backend_filters_records_by_owner() {
        let mut gate = ResourceGate::new();
        gate.register(
            RefDescriptor {
                ref_id: "ref-a".into(),
                kind: "domain.resource".into(),
                schema_id_target: "domain.resource".into(),
                schema_version_target: "1.0.0".into(),
                attributes: BTreeMap::new(),
                lineage: Vec::new(),
            },
            "owner-a",
        );
        gate.register(
            RefDescriptor {
                ref_id: "ref-b".into(),
                kind: "domain.resource".into(),
                schema_id_target: "domain.resource".into(),
                schema_version_target: "1.0.0".into(),
                attributes: BTreeMap::new(),
                lineage: Vec::new(),
            },
            "owner-b",
        );

        let owner_a = <ResourceGate as ResourceBackend>::list_records(&gate, Some("owner-a"));
        assert_eq!(owner_a.len(), 1);
        assert_eq!(owner_a[0].descriptor.ref_id, "ref-a");
    }

    #[test]
    fn failed_awake_does_not_commit_agent_to_routing() {
        let mut runtime = AgentRuntime::new();
        let mut backend = NativeBackend {
            fail_awake: true,
            ..backend()
        };
        runtime.register_agent(agent("agent-a", 0)).unwrap();

        let err = runtime.start_agent("agent-a", &mut backend).unwrap_err();
        assert_eq!(err.error().code, ERR_RUNTIME_BACKEND_FAILED);
        assert_eq!(runtime.phase("agent-a"), Some(&AgentPhase::Spawn));
        let publish_err = runtime.publish(envelope()).unwrap_err();
        assert_eq!(publish_err.error().code, ERR_SOURCE_UNREGISTERED);
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
            ..backend()
        };
        runtime.register_agent(agent("agent-a", 0)).unwrap();

        let err = runtime.start_agent("agent-a", &mut backend).unwrap_err();
        assert_eq!(err.error().code, ERR_RUNTIME_BACKEND_FAILED);
        assert_eq!(runtime.phase("agent-a"), Some(&AgentPhase::Spawn));
        let publish_err = runtime.publish(envelope()).unwrap_err();
        assert_eq!(publish_err.error().code, ERR_SOURCE_UNREGISTERED);
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
    fn failed_source_refresh_does_not_commit_operation_or_source_registry() {
        let mut runtime = AgentRuntime::new();
        let mut backend = backend();
        backend.fail_list_sources = true;
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

        let err = runtime.start_agent("agent-a", &mut backend).unwrap_err();
        assert_eq!(err.error().code, ERR_RUNTIME_BACKEND_FAILED);
        assert_eq!(runtime.phase("agent-a"), Some(&AgentPhase::Spawn));
        assert!(runtime.operation_snapshot("agent-a", "test.noop").is_none());
        assert!(runtime.source_snapshots("agent-a").is_none());
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
