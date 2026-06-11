use std::collections::BTreeMap;
use std::io::{BufReader, BufWriter};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use mutsukicore_runtime_contracts::{
    AgentParticipation, AgentPhase, AgentSpec, Envelope, OperationDescriptor, OperationHandlerKey,
    OperationSnapshot, OperationStatus, PluginDescriptor, PluginSnapshot, PluginStatus,
    RefDescriptor, ResourceRecord, RuntimeError, ScopeRuleSpec, SideEffectPolicy, SourceDescriptor,
    SourceRef, SourceSnapshot, StrategyResult,
};
use mutsukicore_runtime_core::AgentRuntime;
use serde_json::{Value, json};

use crate::JsonlRuntimeBackend;

pub(crate) fn agent() -> AgentSpec {
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

pub(crate) fn strategy_agent() -> AgentSpec {
    AgentSpec {
        agent_id: "strategy-agent".into(),
        owner: None,
        priority: 0,
        participation: AgentParticipation::PrimaryCandidate,
        accepts: vec![ScopeRuleSpec::BySourceId {
            source_id: "source:strategy-test".into(),
        }],
        strategy_id: "test-strategy-plugin".into(),
        side_effect_policy: SideEffectPolicy::ReadOnly,
    }
}

pub(crate) fn envelope() -> Envelope {
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

pub(crate) fn strategy_envelope() -> Envelope {
    Envelope {
        id: "env-strategy-1".into(),
        timestamp: 1.0,
        source: SourceRef {
            source_id: "source:strategy-test".into(),
            kind: "test.strategy".into(),
            metadata: BTreeMap::new(),
        },
        payload_schema_id: "test.strategy.input".into(),
        capabilities_required: Vec::new(),
        payload: json!({"prompt": "decide"}),
    }
}

pub(crate) struct FixtureJsonlBackendProcess {
    child: Child,
}

impl FixtureJsonlBackendProcess {
    pub(crate) fn spawn(
        stub_output: &str,
    ) -> (
        Self,
        JsonlRuntimeBackend<BufReader<ChildStdout>, BufWriter<ChildStdin>>,
    ) {
        let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo_root = crate_root
            .parent()
            .and_then(|path| path.parent())
            .expect("host crate must live under crates/");
        let script = repo_root
            .join("crates")
            .join("mutsukicore-runtime-host")
            .join("tests")
            .join("fixtures")
            .join("jsonl_strategy_backend.py");
        assert!(
            script.is_file(),
            "missing JSONL strategy fixture script: {script:?}"
        );
        let mut command = if let Ok(python) = std::env::var("PYTHON") {
            Command::new(python)
        } else {
            let mut command = Command::new("uv");
            command
                .arg("run")
                .arg("--project")
                .arg(repo_root.join("python").join("mutsukicore-runtime-python"))
                .arg("python");
            command
        };
        let mut child = command
            .arg(&script)
            .arg("--agent-id")
            .arg("strategy-agent")
            .arg("--stub-output")
            .arg(stub_output)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn Python JSONL strategy fixture");
        let stdout = child
            .stdout
            .take()
            .expect("Python backend stdout must be piped");
        let stdin = child
            .stdin
            .take()
            .expect("Python backend stdin must be piped");
        (
            Self { child },
            JsonlRuntimeBackend::new(BufReader::new(stdout), BufWriter::new(stdin)),
        )
    }
}

pub(crate) fn tick_strategy_fixture_backend(stub_output: &str) -> (AgentRuntime, StrategyResult) {
    let (_process, mut backend) = FixtureJsonlBackendProcess::spawn(stub_output);
    let mut runtime = AgentRuntime::new();

    runtime.register_agent(strategy_agent()).unwrap();
    runtime.start_agent("strategy-agent", &mut backend).unwrap();
    assert_eq!(runtime.phase("strategy-agent"), Some(&AgentPhase::Awake));
    assert_eq!(
        runtime.source_snapshots("strategy-agent").unwrap()[0]
            .descriptor
            .source_id,
        "source:strategy-test"
    );
    assert_eq!(
        runtime.publish(strategy_envelope()).unwrap(),
        vec!["strategy-agent"]
    );

    let result = runtime.tick_once("strategy-agent", &mut backend).unwrap();
    runtime.stop_agent("strategy-agent", &mut backend).unwrap();
    (runtime, result)
}

impl Drop for FixtureJsonlBackendProcess {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
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

pub(crate) fn plugin_snapshot(plugin_id: &str) -> PluginSnapshot {
    PluginSnapshot {
        descriptor: PluginDescriptor {
            plugin_id: plugin_id.into(),
            generation: 0,
            name: plugin_id.into(),
            description: String::new(),
            version: String::new(),
            capabilities: Vec::new(),
            metadata: BTreeMap::new(),
        },
        status: PluginStatus::Enabled,
    }
}

pub(crate) fn operation_descriptor(op_id: &str) -> OperationDescriptor {
    OperationDescriptor {
        op_id: op_id.into(),
        name: op_id.rsplit('.').next().unwrap_or(op_id).into(),
        description: String::new(),
        plugin_id: op_id.split('.').next().unwrap_or("test").into(),
        func_qualname: String::new(),
        parameters_schema: json!({}),
        return_schema: json!({}),
        perms_rule_id: None,
        requires_capabilities: Vec::new(),
        is_tool: true,
    }
}

pub(crate) fn operation_key(op_id: &str) -> OperationHandlerKey {
    let plugin_id = op_id.split('.').next().unwrap_or("test");
    OperationHandlerKey {
        plugin_id: plugin_id.into(),
        plugin_generation: 0,
        op_id: op_id.into(),
        handler_id: format!("{plugin_id}:{op_id}:0"),
    }
}

pub(crate) fn operation_snapshot(op_id: &str, status: OperationStatus) -> OperationSnapshot {
    OperationSnapshot {
        descriptor: operation_descriptor(op_id),
        status,
        key: operation_key(op_id),
    }
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

pub(crate) fn resource_record(
    ref_id: &str,
    kind: &str,
    owner: &str,
    lease_count: u64,
) -> ResourceRecord {
    ResourceRecord {
        descriptor: ref_descriptor(ref_id, kind),
        owner: owner.into(),
        lease_count,
    }
}

pub(crate) fn jsonl_response(result: serde_json::Value) -> String {
    json!({"id": "req-1", "ok": true, "result": result}).to_string() + "\n"
}

pub(crate) fn jsonl_failure_response(error: RuntimeError) -> String {
    json!({"id": "req-1", "ok": false, "error": error}).to_string() + "\n"
}

pub(crate) fn jsonl_scripted_responses(
    responses: impl IntoIterator<Item = serde_json::Value>,
) -> String {
    responses
        .into_iter()
        .map(|response| response.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

pub(crate) fn written_requests(writer: Vec<u8>) -> Vec<serde_json::Value> {
    String::from_utf8(writer)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect()
}
