use std::collections::HashMap;

use mutsuki_runtime_contracts::{
    AgentSpec, ERR_OPERATION_NOT_FOUND, ERR_RUNTIME_BACKEND_GENERATION_MISMATCH, Envelope,
    OperationHandlerKey, OperationSnapshot, OperationStatus, PluginDescriptor, PluginSnapshot,
    PluginStatus, RuntimeError, SourceSnapshot, StrategyResult, StrategyResultStatus,
};
use mutsuki_runtime_core::{
    AgentRuntime, BackendEventSink, BackendPayload, OperationBackend, RuntimeFailure,
    RuntimeResult, StrategyBackend,
};
use serde_json::Value;

use crate::NativeOperation;

#[derive(Default)]
pub struct NativeRuntimeHost {
    operations: HashMap<String, NativeOperation>,
    plugins: HashMap<String, PluginSnapshot>,
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
        let plugin_id = operation.snapshot.key.plugin_id.clone();
        let generation = operation.snapshot.key.plugin_generation;
        self.ensure_plugin(&plugin_id, generation);
        self.operations
            .insert(operation.snapshot.descriptor.op_id.clone(), operation);
    }

    pub fn register_plugin(&mut self, plugin: PluginSnapshot) {
        self.plugins
            .insert(plugin.descriptor.plugin_id.clone(), plugin);
    }

    pub fn register_source(&mut self, source: SourceSnapshot) {
        self.ensure_plugin(&source.plugin_id, source.plugin_generation);
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

    fn ensure_plugin(&mut self, plugin_id: &str, generation: u64) {
        self.plugins
            .entry(plugin_id.to_string())
            .or_insert_with(|| PluginSnapshot {
                descriptor: PluginDescriptor {
                    plugin_id: plugin_id.to_string(),
                    generation,
                    name: plugin_id.to_string(),
                    description: String::new(),
                    version: String::new(),
                    capabilities: Vec::new(),
                    metadata: Default::default(),
                },
                status: PluginStatus::Enabled,
            });
    }

    fn plugin_enabled(&self, plugin_id: &str) -> bool {
        self.plugins
            .get(plugin_id)
            .is_some_and(|plugin| plugin.status == PluginStatus::Enabled)
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
    fn list_plugins(&self) -> RuntimeResult<Vec<PluginSnapshot>> {
        Ok(self.plugins.values().cloned().collect())
    }

    fn list_operations(&self, _agent_id: &str) -> RuntimeResult<Vec<OperationSnapshot>> {
        Ok(self
            .operations
            .values()
            .filter(|operation| self.plugin_enabled(&operation.snapshot.key.plugin_id))
            .map(|operation| operation.snapshot.clone())
            .collect())
    }

    fn list_sources(&self, _agent_id: &str) -> RuntimeResult<Vec<SourceSnapshot>> {
        Ok(self
            .sources
            .iter()
            .filter(|source| self.plugin_enabled(&source.plugin_id))
            .cloned()
            .collect())
    }

    fn invoke(
        &mut self,
        _agent_id: &str,
        key: &OperationHandlerKey,
        payload: Value,
        _events: &mut dyn BackendEventSink,
    ) -> RuntimeResult<BackendPayload> {
        let operation = self.operations.get_mut(&key.op_id).ok_or_else(|| {
            RuntimeFailure::new(RuntimeError::new(
                ERR_OPERATION_NOT_FOUND,
                "native_runtime_host",
                format!("native.invoke.{}", key.op_id),
            ))
        })?;
        if operation.snapshot.key != *key {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_RUNTIME_BACKEND_GENERATION_MISMATCH,
                "native_runtime_host",
                format!("native.invoke.{}", key.op_id),
            )));
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
