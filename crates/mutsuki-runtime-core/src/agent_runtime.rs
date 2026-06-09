use std::collections::{BTreeMap, HashMap, VecDeque};

use mutsuki_runtime_contracts::{
    AgentId, AgentParticipation, AgentPhase, AgentSpec, ERR_AGENT_NOT_FOUND,
    ERR_OPERATION_NOT_FOUND, Envelope, OperationSnapshot, OperationStatus, RuntimeError,
    RuntimeEvent, RuntimeEventKind, ScalarValue, SourceSnapshot, SpanStatus, StrategyResult,
    TraceSpan,
};
use serde_json::Value;

use crate::backend::{BackendPayload, OperationBackend, RuntimeBackend};
use crate::election::{ElectionCandidate, ElectionPolicy, PriorityElectionPolicy};
use crate::error::{
    RuntimeFailure, RuntimeResult, operation_not_active_failure, scope_no_match_failure,
    source_unregistered_failure,
};
use crate::event::EventBook;
use crate::resource_gate::ResourceGate;
use crate::trace::TraceBook;

#[derive(Clone, Debug)]
pub struct AgentState {
    pub spec: AgentSpec,
    pub phase: AgentPhase,
    pub inbox: VecDeque<Envelope>,
}

#[derive(Clone, Debug)]
pub struct AgentRuntime {
    agents: HashMap<AgentId, AgentState>,
    operation_registry: HashMap<AgentId, HashMap<String, OperationSnapshot>>,
    source_registry: HashMap<AgentId, Vec<SourceSnapshot>>,
    resource_gate: ResourceGate,
    trace: TraceBook,
    events: EventBook,
}

impl Default for AgentRuntime {
    fn default() -> Self {
        Self {
            agents: HashMap::new(),
            operation_registry: HashMap::new(),
            source_registry: HashMap::new(),
            resource_gate: ResourceGate::with_runtime_event_drafts(),
            trace: TraceBook::default(),
            events: EventBook::default(),
        }
    }
}

impl AgentRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_agent(&mut self, spec: AgentSpec) -> RuntimeResult<()> {
        let agent_id = spec.agent_id.clone();
        self.agents.insert(
            agent_id.clone(),
            AgentState {
                spec,
                phase: AgentPhase::Spawn,
                inbox: VecDeque::new(),
            },
        );
        self.emit_agent(
            RuntimeEventKind::Lifecycle,
            "agent.register",
            &agent_id,
            BTreeMap::new(),
            None,
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
            self.record_agent_trace(agent_id, "agent.awake", None, SpanStatus::Error);
            self.emit_failure(
                RuntimeEventKind::Lifecycle,
                "agent.awake.error",
                Some(agent_id),
                BTreeMap::new(),
                &err,
            );
            return Err(err);
        }
        let operation_snapshots = match backend.list_operations(agent_id) {
            Ok(snapshots) => snapshots,
            Err(err) => {
                self.record_agent_trace(agent_id, "agent.awake", None, SpanStatus::Error);
                self.emit_failure(
                    RuntimeEventKind::Backend,
                    "backend.list_operations.error",
                    Some(agent_id),
                    BTreeMap::new(),
                    &err,
                );
                return Err(err);
            }
        };
        let source_snapshots = match backend.list_sources(agent_id) {
            Ok(snapshots) => snapshots,
            Err(err) => {
                self.record_agent_trace(agent_id, "agent.awake", None, SpanStatus::Error);
                self.emit_failure(
                    RuntimeEventKind::Backend,
                    "backend.list_sources.error",
                    Some(agent_id),
                    BTreeMap::new(),
                    &err,
                );
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
        self.record_agent_trace(agent_id, "agent.awake", None, SpanStatus::Ok);
        self.emit_agent(
            RuntimeEventKind::Lifecycle,
            "agent.awake",
            agent_id,
            BTreeMap::new(),
            None,
        );
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
        self.record_agent_trace(agent_id, "agent.sleep", None, SpanStatus::Ok);
        self.emit_agent(
            RuntimeEventKind::Lifecycle,
            "agent.sleep",
            agent_id,
            BTreeMap::new(),
            None,
        );
        if let Err(err) = backend.on_stop(agent_id) {
            self.record_agent_trace(agent_id, "agent.stop", None, SpanStatus::Error);
            self.emit_failure(
                RuntimeEventKind::Lifecycle,
                "agent.stop.error",
                Some(agent_id),
                BTreeMap::new(),
                &err,
            );
            return Err(err);
        }
        let agent = self.agent_mut(agent_id)?;
        agent.phase = AgentPhase::Stop;
        self.record_agent_trace(agent_id, "agent.stop", None, SpanStatus::Ok);
        self.emit_agent(
            RuntimeEventKind::Lifecycle,
            "agent.stop",
            agent_id,
            BTreeMap::new(),
            None,
        );
        Ok(())
    }

    pub fn publish(&mut self, envelope: Envelope) -> RuntimeResult<Vec<AgentId>> {
        if !self.has_registered_source(&envelope.source.source_id) {
            self.record_runtime_trace("runtime.source_unregistered", None, SpanStatus::Error);
            let err = source_unregistered_failure(&envelope);
            self.emit_failure(
                RuntimeEventKind::Routing,
                "runtime.source_unregistered",
                None,
                source_attributes(&envelope),
                &err,
            );
            return Err(err);
        }
        let mut matched = Vec::new();
        for (agent_id, agent) in &mut self.agents {
            if Self::agent_accepts(agent, &envelope) {
                agent.inbox.push_back(envelope.clone());
                matched.push(agent_id.clone());
            }
        }
        if matched.is_empty() {
            self.record_runtime_trace("runtime.scope_no_match", None, SpanStatus::Error);
            let err = scope_no_match_failure(&envelope);
            self.emit_failure(
                RuntimeEventKind::Routing,
                "runtime.scope_no_match",
                None,
                source_attributes(&envelope),
                &err,
            );
            return Err(err);
        }
        let mut attributes = source_attributes(&envelope);
        attributes.insert("matched".into(), ScalarValue::Int(matched.len() as i64));
        self.emit(
            RuntimeEventKind::Routing,
            "runtime.publish",
            None,
            attributes,
            None,
        );
        Ok(matched)
    }

    pub fn select_accepting(&self, envelope: &Envelope) -> Option<AgentId> {
        self.select_accepting_with_policy(envelope, &PriorityElectionPolicy)
    }

    pub fn select_accepting_with_policy<P: ElectionPolicy>(
        &self,
        envelope: &Envelope,
        policy: &P,
    ) -> Option<AgentId> {
        if !self.has_registered_source(&envelope.source.source_id) {
            return None;
        }
        let candidates: Vec<ElectionCandidate> = self
            .agents
            .values()
            .filter(|agent| {
                Self::agent_accepts(agent, envelope)
                    && agent.spec.participation == AgentParticipation::PrimaryCandidate
            })
            .map(|agent| ElectionCandidate {
                agent_id: agent.spec.agent_id.clone(),
                priority: agent.spec.priority,
            })
            .collect();
        let selected = policy.select(&candidates)?;
        candidates
            .iter()
            .any(|candidate| candidate.agent_id == selected)
            .then_some(selected)
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
                let input_span =
                    self.record_agent_trace(agent_id, "agent.input", None, SpanStatus::Ok);
                let result = match backend.on_input(agent_id, &envelope) {
                    Ok(result) => result,
                    Err(err) => {
                        self.record_agent_trace(
                            agent_id,
                            "agent.strategy",
                            Some(input_span.span_id),
                            SpanStatus::Error,
                        );
                        self.emit_failure(
                            RuntimeEventKind::Routing,
                            "agent.input.error",
                            Some(agent_id),
                            source_attributes(&envelope),
                            &err,
                        );
                        return Err(err);
                    }
                };
                let status = if result.error.is_some() {
                    SpanStatus::Error
                } else {
                    SpanStatus::Ok
                };
                self.record_agent_trace(
                    agent_id,
                    "agent.strategy",
                    Some(input_span.span_id),
                    status,
                );
                let error = result.error.clone();
                let name = if error.is_some() {
                    "agent.input.error"
                } else {
                    "agent.input"
                };
                self.emit_agent(
                    RuntimeEventKind::Routing,
                    name,
                    agent_id,
                    source_attributes(&envelope),
                    error,
                );
                result
            }
            None => {
                let tick_span =
                    self.record_agent_trace(agent_id, "agent.next_step", None, SpanStatus::Ok);
                let result = match backend.next_step(agent_id) {
                    Ok(result) => result,
                    Err(err) => {
                        self.record_agent_trace(
                            agent_id,
                            "agent.strategy",
                            Some(tick_span.span_id),
                            SpanStatus::Error,
                        );
                        self.emit_failure(
                            RuntimeEventKind::Lifecycle,
                            "agent.next_step.error",
                            Some(agent_id),
                            BTreeMap::new(),
                            &err,
                        );
                        return Err(err);
                    }
                };
                let status = if result.error.is_some() {
                    SpanStatus::Error
                } else {
                    SpanStatus::Ok
                };
                self.record_agent_trace(
                    agent_id,
                    "agent.strategy",
                    Some(tick_span.span_id),
                    status,
                );
                let error = result.error.clone();
                let name = if error.is_some() {
                    "agent.next_step.error"
                } else {
                    "agent.next_step"
                };
                self.emit_agent(
                    RuntimeEventKind::Lifecycle,
                    name,
                    agent_id,
                    BTreeMap::new(),
                    error,
                );
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
            RuntimeFailure::new(RuntimeError::new(
                ERR_OPERATION_NOT_FOUND,
                "runtime.operation_registry",
                format!("runtime.invoke.{agent_id}.{op_id}"),
            ))
        })?;
        let snapshot = registry.get(op_id).ok_or_else(|| {
            RuntimeFailure::new(RuntimeError::new(
                ERR_OPERATION_NOT_FOUND,
                "runtime.operation_registry",
                format!("runtime.invoke.{agent_id}.{op_id}"),
            ))
        })?;
        if snapshot.status != OperationStatus::Active {
            let err = operation_not_active_failure(snapshot, agent_id, op_id);
            self.emit_agent(
                RuntimeEventKind::Operation,
                "operation.invoke.error",
                agent_id,
                op_status_attributes(op_id, &snapshot.status),
                Some(err.error().clone()),
            );
            return Err(err);
        }
        let key = snapshot.key.clone();
        let invoke_span =
            self.record_agent_trace(agent_id, "operation.invoke", None, SpanStatus::Ok);
        match backend.invoke(agent_id, &key, payload) {
            Ok(result) => {
                self.emit_agent(
                    RuntimeEventKind::Operation,
                    "operation.invoke",
                    agent_id,
                    op_attributes(op_id),
                    None,
                );
                Ok(result)
            }
            Err(err) => {
                self.record_agent_trace(
                    agent_id,
                    "operation.invoke.error",
                    Some(invoke_span.span_id),
                    SpanStatus::Error,
                );
                self.emit_agent(
                    RuntimeEventKind::Operation,
                    "operation.invoke.error",
                    agent_id,
                    op_attributes(op_id),
                    Some(err.error().clone()),
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

    pub fn events(&self) -> Vec<RuntimeEvent> {
        self.events
            .snapshot_with_drafts(self.resource_gate.event_drafts())
    }

    pub fn drain_events(&mut self) -> Vec<RuntimeEvent> {
        self.flush_resource_events();
        self.events.drain()
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
            RuntimeFailure::new(RuntimeError::new(
                ERR_AGENT_NOT_FOUND,
                "runtime.agent",
                format!("runtime.agent.{agent_id}"),
            ))
        })
    }

    fn agent_mut(&mut self, agent_id: &str) -> RuntimeResult<&mut AgentState> {
        self.agents.get_mut(agent_id).ok_or_else(|| {
            RuntimeFailure::new(RuntimeError::new(
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

    fn emit(
        &mut self,
        kind: RuntimeEventKind,
        name: impl Into<String>,
        agent_id: Option<AgentId>,
        attributes: BTreeMap<String, ScalarValue>,
        error: Option<RuntimeError>,
    ) {
        self.flush_resource_events();
        self.events.record(kind, name, agent_id, attributes, error);
    }

    fn emit_agent(
        &mut self,
        kind: RuntimeEventKind,
        name: impl Into<String>,
        agent_id: &str,
        attributes: BTreeMap<String, ScalarValue>,
        error: Option<RuntimeError>,
    ) {
        self.emit(kind, name, Some(agent_id.to_string()), attributes, error);
    }

    fn emit_failure(
        &mut self,
        kind: RuntimeEventKind,
        name: impl Into<String>,
        agent_id: Option<&str>,
        attributes: BTreeMap<String, ScalarValue>,
        failure: &RuntimeFailure,
    ) {
        self.emit(
            kind,
            name,
            agent_id.map(str::to_string),
            attributes,
            Some(failure.error().clone()),
        );
    }

    fn record_agent_trace(
        &mut self,
        agent_id: &str,
        name: impl Into<String>,
        parent_span_id: Option<String>,
        status: SpanStatus,
    ) -> TraceSpan {
        self.record_trace(agent_id, Some(agent_id), name, parent_span_id, status)
    }

    fn record_runtime_trace(
        &mut self,
        name: impl Into<String>,
        parent_span_id: Option<String>,
        status: SpanStatus,
    ) -> TraceSpan {
        self.record_trace("runtime", None, name, parent_span_id, status)
    }

    fn record_trace(
        &mut self,
        trace_actor_id: &str,
        event_agent_id: Option<&str>,
        name: impl Into<String>,
        parent_span_id: Option<String>,
        status: SpanStatus,
    ) -> TraceSpan {
        let span = self
            .trace
            .record(trace_actor_id, name, parent_span_id, status);
        self.emit(
            RuntimeEventKind::Trace,
            "trace.span",
            event_agent_id.map(str::to_string),
            trace_attributes(&span),
            None,
        );
        span
    }

    fn flush_resource_events(&mut self) {
        self.events
            .append_drafts(self.resource_gate.drain_event_drafts());
    }
}

fn source_attributes(envelope: &Envelope) -> BTreeMap<String, ScalarValue> {
    let mut attributes = BTreeMap::new();
    attributes.insert(
        "source_id".into(),
        ScalarValue::String(envelope.source.source_id.clone()),
    );
    attributes.insert(
        "payload_schema_id".into(),
        ScalarValue::String(envelope.payload_schema_id.clone()),
    );
    attributes
}

fn op_attributes(op_id: &str) -> BTreeMap<String, ScalarValue> {
    let mut attributes = BTreeMap::new();
    attributes.insert("op_id".into(), ScalarValue::String(op_id.to_string()));
    attributes
}

fn op_status_attributes(op_id: &str, status: &OperationStatus) -> BTreeMap<String, ScalarValue> {
    let mut attributes = op_attributes(op_id);
    attributes.insert(
        "operation_status".into(),
        ScalarValue::String(format!("{status:?}")),
    );
    attributes
}

fn trace_attributes(span: &TraceSpan) -> BTreeMap<String, ScalarValue> {
    let mut attributes = BTreeMap::new();
    attributes.insert(
        "trace_id".into(),
        ScalarValue::String(span.trace_id.clone()),
    );
    attributes.insert("span_id".into(), ScalarValue::String(span.span_id.clone()));
    if let Some(parent_span_id) = &span.parent_span_id {
        attributes.insert(
            "parent_span_id".into(),
            ScalarValue::String(parent_span_id.clone()),
        );
    }
    attributes.insert("span_name".into(), ScalarValue::String(span.name.clone()));
    attributes.insert("start".into(), ScalarValue::Float(span.start));
    if let Some(end) = span.end {
        attributes.insert("end".into(), ScalarValue::Float(end));
    }
    attributes.insert(
        "status".into(),
        ScalarValue::String(
            match &span.status {
                SpanStatus::Ok => "ok",
                SpanStatus::Error => "error",
            }
            .into(),
        ),
    );
    attributes
}
