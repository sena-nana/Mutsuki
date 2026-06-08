use std::collections::{HashMap, VecDeque};

use mutsuki_runtime_contracts::{
    AgentId, AgentParticipation, AgentPhase, AgentSpec, ERR_AGENT_NOT_FOUND,
    ERR_OPERATION_NOT_FOUND, ERR_RUNTIME_BACKEND_FAILED, Envelope, OperationSnapshot,
    OperationStatus, RuntimeError, ScalarValue, SourceSnapshot, SpanStatus, StrategyResult,
    TraceSpan,
};
use serde_json::Value;

use crate::backend::{BackendPayload, OperationBackend, RuntimeBackend};
use crate::error::{
    RuntimeFailure, RuntimeResult, scope_no_match_failure, source_unregistered_failure,
};
use crate::resource_gate::ResourceGate;
use crate::trace::TraceBook;

#[derive(Clone, Debug)]
pub struct AgentState {
    pub spec: AgentSpec,
    pub phase: AgentPhase,
    pub inbox: VecDeque<Envelope>,
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
            let mut err = RuntimeError::new(
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
}
