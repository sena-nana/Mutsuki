use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use mutsuki_agent_plugin_conformance::{
    PLUGIN_ID, PROTOCOL_ID, dynamic_library_file_name, plugin_builder,
};
use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    ArtifactType, BatchEntry, BatchPayload, CancelPolicy, CommandPlan, DispatchLane, ExportPlan,
    OrderingRequirement, PlanReceipt, ReadPlan, RunnerContext, SnapshotDescriptor, StreamPlan,
    Task, TaskBatch, TaskHandle, TaskLease, TaskOutcome, WorkBatch, WorkResourcePlan, WritePlan,
};
use mutsuki_runtime_core::{Runner, RuntimeFailure, RuntimeResult};
use mutsuki_runtime_host::{AbiPluginLoadRequest, load_abi_plugin_v2};
use mutsuki_runtime_sdk::{ResourcePlanGateway, TaskSubmitter};
use serde_json::{Value, json};

#[derive(Default)]
struct NoopHost;

impl TaskSubmitter for NoopHost {
    fn submit_batch(&self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        Ok(batch
            .tasks
            .into_iter()
            .map(|task| TaskHandle {
                task_id: task.task_id,
                protocol_id: task.protocol_id,
                target_binding_id: task.target_binding_id,
                cancel_policy: CancelPolicy::Cascade,
                trace_id: task.trace_id,
                correlation_id: task.correlation_id,
            })
            .collect())
    }

    fn cancel_task(&self, _handle: &TaskHandle) -> RuntimeResult<()> {
        Ok(())
    }

    fn task_outcome(&self, _handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        Ok(None)
    }
}

impl ResourcePlanGateway for NoopHost {
    fn collect_read_plan(&self, _plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        Err(unsupported("read"))
    }

    fn snapshot_read_plan(
        &self,
        _plan: &ReadPlan,
        _kind_id: &str,
        _schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        Err(unsupported("snapshot"))
    }

    fn open_stream_plan(&self, _plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        Err(unsupported("stream"))
    }

    fn execute_export_plan(&self, _plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        Err(unsupported("export"))
    }

    fn commit_write_plan(&self, _plan: &WritePlan, _bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        Err(unsupported("write"))
    }

    fn execute_command_plan(&self, _plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        Err(unsupported("command"))
    }

    fn execute_command_batch(&self, _batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        Err(unsupported("command_batch"))
    }

    fn execute_saga_plan(&self, _saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        Err(unsupported("saga"))
    }
}

fn unsupported(route: &str) -> RuntimeFailure {
    RuntimeFailure::new(mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RESOURCE_UNSUPPORTED,
        "agent.conformance.test",
        route,
    ))
}

fn work_batch(task: Task) -> (RunnerContext, WorkBatch) {
    let lease_id = format!("lease:{}", task.task_id);
    let task_id = task.task_id.clone();
    (
        RunnerContext::new(
            1,
            1,
            "executor:conformance",
            Some(lease_id.clone()),
            "invocation:conformance",
        ),
        WorkBatch {
            batch_id: "batch:conformance".into(),
            tick_id: "tick:conformance".into(),
            batch_key: "agent.conformance".into(),
            entries: vec![BatchEntry {
                entry_id: task_id.clone(),
                task_id: task_id.clone(),
                trace_id: None,
                parent_id: None,
                payload_index: 0,
                resource_requirement_indices: Vec::new(),
                cancel_index: Some(0),
                deadline_tick: None,
                priority: 0,
                lane: DispatchLane::Normal,
                ordering: OrderingRequirement::None,
            }],
            payload: BatchPayload::from_local_tasks(vec![task]),
            resource_plan: WorkResourcePlan::empty(),
            task_leases: vec![TaskLease {
                lease_id,
                task_id,
                attempt_generation: 1,
                runner_id: "agent.conformance.runner".into(),
                executor_id: "executor:conformance".into(),
                registry_generation: 1,
                acquired_at_step: 1,
                expires_at_step: None,
            }],
        },
    )
}

fn invoke(runner: &mut dyn Runner) -> Value {
    let (context, batch) = work_batch(Task::new(
        "agent-conformance",
        PROTOCOL_ID,
        json!({"value": "round-trip"}),
    ));
    runner
        .run_batch(context, batch)
        .unwrap()
        .results
        .remove(0)
        .result
        .unwrap()
        .output
        .unwrap()
        .into()
}

#[test]
fn builtin_and_real_abi_v2_paths_share_manifest_and_runner_semantics() {
    let mut builtin = plugin_builder(7, ArtifactType::Native).unwrap().build();
    let builtin_descriptor = builtin
        .manifest
        .metadata
        .get("agentkit.descriptor")
        .cloned();
    let builtin_output = invoke(builtin.runners[0].as_mut());

    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "--locked",
            "-p",
            "mutsuki-agent-plugin-conformance",
            "--lib",
        ])
        .status()
        .expect("build Agent conformance ABI library");
    assert!(status.success());
    let library_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .unwrap()
        .join("target")
        .join("debug")
        .join(dynamic_library_file_name());
    assert!(library_path.is_file(), "{}", library_path.display());

    let expected = plugin_builder(7, ArtifactType::Abi)
        .unwrap()
        .build()
        .manifest;
    let host = Arc::new(NoopHost);
    let task_submitter: Arc<dyn TaskSubmitter> = host.clone();
    let resource_gateway: Arc<dyn ResourcePlanGateway> = host;
    let mut external = load_abi_plugin_v2(AbiPluginLoadRequest {
        library_path,
        expected_manifest: expected,
        config: Some(json!({"generation": 7})),
        task_submitter,
        resource_gateway,
    })
    .unwrap();
    assert_eq!(external.manifest.plugin_id, PLUGIN_ID);
    assert_eq!(
        external.manifest.metadata.get("agentkit.descriptor"),
        builtin_descriptor.as_ref()
    );
    assert_eq!(invoke(external.runners[0].as_mut()), builtin_output);
}
