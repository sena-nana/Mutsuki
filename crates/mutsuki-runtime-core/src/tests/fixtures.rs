use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    AgentParticipation, AgentSpec, ERR_RUNTIME_BACKEND_FAILED, Envelope, OperationDescriptor,
    OperationHandlerKey, OperationSnapshot, OperationStatus, PluginDescriptor, PluginSnapshot,
    PluginStatus, RefDescriptor, RuntimeError, ScopeRuleSpec, SourceDescriptor, SourceRef,
    SourceSnapshot, StrategyResult, StrategyResultStatus,
};
use serde_json::{Value, json};

use crate::*;

#[derive(Default)]
pub(crate) struct TestBackend {
    pub(crate) awake: usize,
    pub(crate) stopped: usize,
    pub(crate) inputs: usize,
    pub(crate) invocations: usize,
    pub(crate) operations: Vec<OperationSnapshot>,
    pub(crate) sources: Vec<SourceSnapshot>,
    pub(crate) plugins: Vec<PluginSnapshot>,
    pub(crate) fail_list_operations: bool,
    pub(crate) fail_list_sources: bool,
    pub(crate) fail_awake: bool,
    pub(crate) fail_input: bool,
    pub(crate) fail_next_step: bool,
    pub(crate) input_result_error: bool,
    pub(crate) fail_stop: bool,
}

impl StrategyBackend for TestBackend {
    fn on_awake(&mut self, _agent_id: &str) -> RuntimeResult<()> {
        if self.fail_awake {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_RUNTIME_BACKEND_FAILED,
                "native",
                "native.on_awake",
            )));
        }
        self.awake += 1;
        Ok(())
    }

    fn on_input(&mut self, _agent_id: &str, _envelope: &Envelope) -> RuntimeResult<StrategyResult> {
        if self.fail_input {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_RUNTIME_BACKEND_FAILED,
                "native",
                "native.on_input",
            )));
        }
        self.inputs += 1;
        let error = self.input_result_error.then(|| {
            RuntimeError::new(
                ERR_RUNTIME_BACKEND_FAILED,
                "native",
                "native.on_input.result",
            )
        });
        let status = if error.is_some() {
            StrategyResultStatus::Failed
        } else {
            StrategyResultStatus::Completed
        };
        Ok(StrategyResult {
            status,
            decision: None,
            emitted: Vec::new(),
            error,
        })
    }

    fn next_step(&mut self, _agent_id: &str) -> RuntimeResult<StrategyResult> {
        if self.fail_next_step {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_RUNTIME_BACKEND_FAILED,
                "native",
                "native.next_step",
            )));
        }
        Ok(StrategyResult::wait_input())
    }

    fn on_stop(&mut self, _agent_id: &str) -> RuntimeResult<()> {
        if self.fail_stop {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_RUNTIME_BACKEND_FAILED,
                "native",
                "native.on_stop",
            )));
        }
        self.stopped += 1;
        Ok(())
    }
}

impl OperationBackend for TestBackend {
    fn list_plugins(&self) -> RuntimeResult<Vec<PluginSnapshot>> {
        let mut plugins: BTreeMap<String, u64> = BTreeMap::new();
        for source in &self.sources {
            plugins.insert(source.plugin_id.clone(), source.plugin_generation);
        }
        for operation in &self.operations {
            plugins.insert(
                operation.key.plugin_id.clone(),
                operation.key.plugin_generation,
            );
        }
        let mut snapshots: Vec<PluginSnapshot> = plugins
            .into_iter()
            .map(|(plugin_id, generation)| PluginSnapshot {
                descriptor: PluginDescriptor {
                    plugin_id: plugin_id.clone(),
                    generation,
                    name: plugin_id,
                    description: String::new(),
                    version: String::new(),
                    capabilities: Vec::new(),
                    metadata: BTreeMap::new(),
                },
                status: PluginStatus::Enabled,
            })
            .collect();
        snapshots.extend(self.plugins.iter().cloned());
        Ok(snapshots)
    }

    fn list_operations(
        &self,
        enabled_plugin_ids: &[String],
    ) -> RuntimeResult<Vec<OperationSnapshot>> {
        if self.fail_list_operations {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_RUNTIME_BACKEND_FAILED,
                "native",
                "native.list_operations",
            )));
        }
        Ok(self
            .operations
            .iter()
            .filter(|operation| enabled_plugin_ids.contains(&operation.key.plugin_id))
            .cloned()
            .collect())
    }

    fn list_sources(&self, enabled_plugin_ids: &[String]) -> RuntimeResult<Vec<SourceSnapshot>> {
        if self.fail_list_sources {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_RUNTIME_BACKEND_FAILED,
                "native",
                "native.list_sources",
            )));
        }
        Ok(self
            .sources
            .iter()
            .filter(|source| enabled_plugin_ids.contains(&source.plugin_id))
            .cloned()
            .collect())
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

pub(crate) fn envelope() -> Envelope {
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

pub(crate) fn agent(agent_id: &str, priority: i64) -> AgentSpec {
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

pub(crate) fn backend() -> TestBackend {
    TestBackend {
        sources: vec![source_snapshot("source:default")],
        ..TestBackend::default()
    }
}

pub(crate) fn source_snapshot(source_id: &str) -> SourceSnapshot {
    SourceSnapshot {
        descriptor: SourceDescriptor {
            source_id: source_id.into(),
            kind: "test".into(),
            capabilities: Vec::new(),
            description: String::new(),
        },
        plugin_id: "native".into(),
        plugin_generation: 0,
    }
}

pub(crate) fn operation_descriptor(op_id: &str) -> OperationDescriptor {
    OperationDescriptor {
        op_id: op_id.into(),
        name: op_id.rsplit('.').next().unwrap_or(op_id).into(),
        description: String::new(),
        plugin_id: "test".into(),
        func_qualname: String::new(),
        parameters_schema: json!({}),
        return_schema: json!({}),
        perms_rule_id: None,
        requires_capabilities: Vec::new(),
        is_tool: true,
    }
}

pub(crate) fn operation_key(op_id: &str) -> OperationHandlerKey {
    OperationHandlerKey {
        plugin_id: "test".into(),
        plugin_generation: 0,
        op_id: op_id.into(),
        handler_id: format!("test:{op_id}:0"),
    }
}

pub(crate) fn operation_snapshot(op_id: &str, status: OperationStatus) -> OperationSnapshot {
    OperationSnapshot {
        descriptor: operation_descriptor(op_id),
        status,
        key: operation_key(op_id),
    }
}

pub(crate) fn operation_snapshot_for_plugin(
    plugin_id: &str,
    op_id: &str,
    status: OperationStatus,
) -> OperationSnapshot {
    let mut descriptor = operation_descriptor(op_id);
    descriptor.plugin_id = plugin_id.into();
    OperationSnapshot {
        descriptor,
        status,
        key: OperationHandlerKey {
            plugin_id: plugin_id.into(),
            plugin_generation: 0,
            op_id: op_id.into(),
            handler_id: format!("{plugin_id}:{op_id}:0"),
        },
    }
}

pub(crate) fn source_snapshot_for_plugin(plugin_id: &str, source_id: &str) -> SourceSnapshot {
    let mut snapshot = source_snapshot(source_id);
    snapshot.plugin_id = plugin_id.into();
    snapshot
}

pub(crate) fn ref_descriptor(ref_id: &str, kind: &str) -> RefDescriptor {
    RefDescriptor {
        ref_id: ref_id.into(),
        kind: kind.into(),
        schema_id_target: kind.into(),
        schema_version_target: "1.0.0".into(),
        attributes: BTreeMap::new(),
        lineage: Vec::new(),
    }
}
