use std::collections::HashMap;

use mutsuki_runtime_contracts::{
    AgentSpec, Envelope, OperationDescriptor, OperationHandlerKey, OperationSnapshot,
    OperationStatus, SourceSnapshot, StrategyResult, StrategyResultStatus,
};
use mutsuki_runtime_core::{
    AgentRuntime, BackendPayload, OperationBackend, RuntimeFailure, RuntimeResult, StrategyBackend,
};
use serde_json::Value;

type NativeHandler = Box<dyn FnMut(Value) -> RuntimeResult<BackendPayload>>;

pub struct NativeOperation {
    snapshot: OperationSnapshot,
    handler: NativeHandler,
}

impl NativeOperation {
    pub fn new(
        descriptor: OperationDescriptor,
        handler: impl FnMut(Value) -> RuntimeResult<BackendPayload> + 'static,
    ) -> Self {
        let key = OperationHandlerKey {
            plugin_id: descriptor.plugin_id.clone(),
            plugin_generation: 0,
            op_id: descriptor.op_id.clone(),
            handler_id: format!("{}:{}:0", descriptor.plugin_id, descriptor.op_id),
        };
        Self {
            snapshot: OperationSnapshot {
                descriptor,
                status: OperationStatus::Active,
                key,
            },
            handler: Box::new(handler),
        }
    }
}

#[derive(Default)]
pub struct NativeRuntimeHost {
    operations: HashMap<String, NativeOperation>,
    sources: Vec<SourceSnapshot>,
    inputs: Vec<Envelope>,
    awake_count: usize,
    stop_count: usize,
}

impl NativeRuntimeHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_operation(&mut self, operation: NativeOperation) {
        self.operations
            .insert(operation.snapshot.descriptor.op_id.clone(), operation);
    }

    pub fn register_source(&mut self, source: SourceSnapshot) {
        self.sources.push(source);
    }

    pub fn received_inputs(&self) -> &[Envelope] {
        &self.inputs
    }

    pub fn awake_count(&self) -> usize {
        self.awake_count
    }

    pub fn stop_count(&self) -> usize {
        self.stop_count
    }

    pub fn set_operation_generation(&mut self, op_id: &str, generation: u64) {
        if let Some(operation) = self.operations.get_mut(op_id) {
            operation.snapshot.key.plugin_generation = generation;
            operation.snapshot.key.handler_id = format!(
                "{}:{}:{}",
                operation.snapshot.key.plugin_id, operation.snapshot.key.op_id, generation
            );
        }
    }

    pub fn start_agent(
        &mut self,
        runtime: &mut AgentRuntime,
        spec: AgentSpec,
    ) -> RuntimeResult<()> {
        let agent_id = spec.agent_id.clone();
        runtime.register_agent(spec)?;
        runtime.start_agent(&agent_id, self)
    }
}

impl StrategyBackend for NativeRuntimeHost {
    fn on_awake(&mut self, _agent_id: &str) -> RuntimeResult<()> {
        self.awake_count += 1;
        Ok(())
    }

    fn on_input(&mut self, _agent_id: &str, envelope: &Envelope) -> RuntimeResult<StrategyResult> {
        self.inputs.push(envelope.clone());
        Ok(StrategyResult {
            status: StrategyResultStatus::WaitInput,
            decision: None,
            emitted: Vec::new(),
            error: None,
        })
    }

    fn next_step(&mut self, _agent_id: &str) -> RuntimeResult<StrategyResult> {
        Ok(StrategyResult::wait_input())
    }

    fn on_stop(&mut self, _agent_id: &str) -> RuntimeResult<()> {
        self.stop_count += 1;
        Ok(())
    }
}

impl OperationBackend for NativeRuntimeHost {
    fn list_operations(&self, _agent_id: &str) -> RuntimeResult<Vec<OperationSnapshot>> {
        Ok(self
            .operations
            .values()
            .map(|operation| operation.snapshot.clone())
            .collect())
    }

    fn list_sources(&self, _agent_id: &str) -> RuntimeResult<Vec<SourceSnapshot>> {
        Ok(self.sources.clone())
    }

    fn invoke(
        &mut self,
        _agent_id: &str,
        key: &OperationHandlerKey,
        payload: Value,
    ) -> RuntimeResult<BackendPayload> {
        let operation = self.operations.get_mut(&key.op_id).ok_or_else(|| {
            RuntimeFailure::new(mutsuki_runtime_contracts::RuntimeError::new(
                mutsuki_runtime_contracts::ERR_OPERATION_NOT_FOUND,
                "native_runtime_host",
                format!("native.invoke.{}", key.op_id),
            ))
        })?;
        if operation.snapshot.key != *key {
            return Err(RuntimeFailure::new(
                mutsuki_runtime_contracts::RuntimeError::new(
                    mutsuki_runtime_contracts::ERR_RUNTIME_BACKEND_GENERATION_MISMATCH,
                    "native_runtime_host",
                    format!("native.invoke.{}", key.op_id),
                ),
            ));
        }
        (operation.handler)(payload)
    }

    fn operation_status(&self, _agent_id: &str, key: &OperationHandlerKey) -> OperationStatus {
        self.operations
            .get(&key.op_id)
            .filter(|operation| operation.snapshot.key == *key)
            .map(|operation| operation.snapshot.status.clone())
            .unwrap_or(OperationStatus::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mutsuki_runtime_contracts::{
        AgentParticipation, ScopeRuleSpec, SideEffectPolicy, SourceDescriptor, SourceRef,
    };
    use serde_json::json;

    use super::*;

    fn agent() -> AgentSpec {
        AgentSpec {
            agent_id: "native-agent".into(),
            owner: None,
            priority: 0,
            participation: AgentParticipation::PrimaryCandidate,
            accepts: vec![ScopeRuleSpec::BySchema {
                schema_id: "test.input".into(),
            }],
            strategy_id: "native".into(),
            side_effect_policy: SideEffectPolicy::ReadOnly,
        }
    }

    fn envelope() -> Envelope {
        Envelope {
            id: "env-1".into(),
            timestamp: 1.0,
            source: SourceRef {
                source_id: "source:test".into(),
                kind: "test".into(),
                metadata: BTreeMap::new(),
            },
            payload_schema_id: "test.input".into(),
            capabilities_required: Vec::new(),
            payload: Value::Null,
        }
    }

    #[test]
    fn native_host_runs_agent_input_and_operation_without_python() {
        let mut runtime = AgentRuntime::new();
        let mut host = NativeRuntimeHost::new();
        host.register_source(SourceSnapshot {
            descriptor: SourceDescriptor {
                source_id: "source:test".into(),
                kind: "test".into(),
                capabilities: Vec::new(),
                description: String::new(),
            },
            plugin_id: "native".into(),
            plugin_generation: 0,
        });
        host.register_operation(NativeOperation::new(
            OperationDescriptor {
                op_id: "native.echo".into(),
                name: "echo".into(),
                description: String::new(),
                plugin_id: "native".into(),
                func_qualname: String::new(),
                parameters_schema: json!({}),
                return_schema: json!({}),
                perms_rule_id: None,
                requires_capabilities: Vec::new(),
                is_tool: true,
            },
            |payload| Ok(BackendPayload::Json(payload)),
        ));

        host.start_agent(&mut runtime, agent()).unwrap();
        assert_eq!(host.awake_count(), 1);

        assert_eq!(runtime.publish(envelope()).unwrap(), vec!["native-agent"]);
        runtime.tick_once("native-agent", &mut host).unwrap();
        assert_eq!(host.received_inputs().len(), 1);

        let result = runtime
            .invoke_operation(
                "native-agent",
                "native.echo",
                json!({"value": "ok"}),
                &mut host,
            )
            .unwrap();
        assert_eq!(result, BackendPayload::Json(json!({"value": "ok"})));

        runtime.stop_agent("native-agent", &mut host).unwrap();
        assert_eq!(host.stop_count(), 1);
    }

    #[test]
    fn native_host_rejects_stale_operation_generation() {
        let mut runtime = AgentRuntime::new();
        let mut host = NativeRuntimeHost::new();
        host.register_source(SourceSnapshot {
            descriptor: SourceDescriptor {
                source_id: "source:test".into(),
                kind: "test".into(),
                capabilities: Vec::new(),
                description: String::new(),
            },
            plugin_id: "native".into(),
            plugin_generation: 0,
        });
        host.register_operation(NativeOperation::new(
            OperationDescriptor {
                op_id: "native.echo".into(),
                name: "echo".into(),
                description: String::new(),
                plugin_id: "native".into(),
                func_qualname: String::new(),
                parameters_schema: json!({}),
                return_schema: json!({}),
                perms_rule_id: None,
                requires_capabilities: Vec::new(),
                is_tool: true,
            },
            |payload| Ok(BackendPayload::Json(payload)),
        ));

        host.start_agent(&mut runtime, agent()).unwrap();
        host.set_operation_generation("native.echo", 1);

        let err = runtime
            .invoke_operation("native-agent", "native.echo", json!({}), &mut host)
            .unwrap_err();
        assert_eq!(
            err.error().code,
            mutsuki_runtime_contracts::ERR_RUNTIME_BACKEND_GENERATION_MISMATCH
        );
        assert!(
            runtime
                .trace_spans()
                .iter()
                .any(|span| span.name == "operation.invoke.error")
        );
    }
}
