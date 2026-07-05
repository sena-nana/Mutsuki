use std::io::Cursor;
use std::sync::{Arc, Mutex};

use mutsuki_plugin_resource_memory::PROVIDER_ID;
use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::{
    AbiTaskClient, HostExtension, LocalResourceClient, LocalTaskClient, PluginBackend, TaskClient,
};

use super::helpers::{host_with_echo_runner, runtime_profile, std_memory_provider};

#[test]
fn host_task_clients_share_task_contract_across_local_and_abi_backends() {
    let runtime = Arc::new(Mutex::new(
        host_with_echo_runner()
            .into_runtime(runtime_profile())
            .unwrap(),
    ));
    let local = LocalTaskClient::new(runtime);
    let mut local_task = Task::new("local-client-task", "raw.input", json!({"source": "local"}));
    local_task.trace_id = Some("trace-local".into());
    local_task.correlation_id = Some("corr-local".into());

    let local_handle = local.submit_task(local_task).unwrap();

    assert_eq!(local_handle.task_id, "local-client-task");
    assert_eq!(local_handle.protocol_id, "raw.input");
    assert_eq!(local_handle.trace_id.as_deref(), Some("trace-local"));
    assert_eq!(local_handle.correlation_id.as_deref(), Some("corr-local"));

    local.cancel_task("local-client-task").unwrap();
    assert!(matches!(
        local.task_outcome("local-client-task").unwrap(),
        Some(TaskOutcome::Cancelled { task_id, .. }) if task_id == "local-client-task"
    ));

    let mut abi_task = Task::new("abi-client-task", "raw.input", json!({"source": "abi"}));
    abi_task.trace_id = Some("trace-abi".into());
    abi_task.correlation_id = Some("corr-abi".into());
    let abi_handle = TaskHandle {
        task_id: abi_task.task_id.clone(),
        protocol_id: abi_task.protocol_id.clone(),
        target_binding_id: None,
        cancel_policy: CancelPolicy::Cascade,
        trace_id: abi_task.trace_id.clone(),
        correlation_id: abi_task.correlation_id.clone(),
    };
    let abi_outcome = TaskOutcome::Cancelled {
        task_id: abi_task.task_id.clone(),
        reason: Some("test.cancel".into()),
    };
    let response = format!(
        "{}\n{}\n{}\n",
        json!({"id": "req-1", "ok": true, "result": abi_handle}),
        json!({"id": "req-2", "ok": true, "result": null}),
        json!({"id": "req-3", "ok": true, "result": abi_outcome}),
    );
    let abi = AbiTaskClient::new(
        Cursor::new(response.into_bytes()),
        Cursor::new(Vec::<u8>::new()),
    );

    let submitted = abi.submit_task(abi_task).unwrap();
    abi.cancel_task("abi-client-task").unwrap();
    let outcome = abi.task_outcome("abi-client-task").unwrap();
    let (_reader, writer) = abi.into_inner();
    let request = String::from_utf8(writer.into_inner()).unwrap();

    assert_eq!(submitted.task_id, "abi-client-task");
    assert_eq!(submitted.trace_id.as_deref(), Some("trace-abi"));
    assert!(matches!(
        outcome,
        Some(TaskOutcome::Cancelled { task_id, .. }) if task_id == "abi-client-task"
    ));
    assert!(request.contains("\"method\":\"task.submit\""));
    assert!(request.contains("\"method\":\"task.cancel\""));
    assert!(request.contains("\"method\":\"task.outcome\""));
    assert!(request.contains("\"trace_id\":\"trace-abi\""));
}

#[test]
fn task_clients_implement_sdk_task_submitter_boundary() {
    let runtime = Arc::new(Mutex::new(
        host_with_echo_runner()
            .into_runtime(runtime_profile())
            .unwrap(),
    ));
    let local = LocalTaskClient::new(runtime);
    let local_handle = mutsuki_runtime_sdk::TaskSubmitter::submit_task(
        &local,
        Task::new("sdk-local-task", "raw.input", json!({})),
    )
    .unwrap();
    mutsuki_runtime_sdk::TaskSubmitter::cancel_task(&local, "sdk-local-task").unwrap();

    assert_eq!(local_handle.task_id, "sdk-local-task");
    assert!(matches!(
        mutsuki_runtime_sdk::TaskSubmitter::task_outcome(&local, "sdk-local-task").unwrap(),
        Some(TaskOutcome::Cancelled { task_id, .. }) if task_id == "sdk-local-task"
    ));

    let abi_task = Task::new("sdk-abi-task", "raw.input", json!({}));
    let abi_handle = TaskHandle {
        task_id: abi_task.task_id.clone(),
        protocol_id: abi_task.protocol_id.clone(),
        target_binding_id: None,
        cancel_policy: CancelPolicy::Cascade,
        trace_id: None,
        correlation_id: None,
    };
    let response = format!(
        "{}\n{}\n{}\n",
        json!({"id": "req-1", "ok": true, "result": abi_handle}),
        json!({"id": "req-2", "ok": true, "result": null}),
        json!({"id": "req-3", "ok": true, "result": null}),
    );
    let abi = AbiTaskClient::new(
        Cursor::new(response.into_bytes()),
        Cursor::new(Vec::<u8>::new()),
    );

    assert_eq!(
        mutsuki_runtime_sdk::TaskSubmitter::submit_task(&abi, abi_task)
            .unwrap()
            .task_id,
        "sdk-abi-task"
    );
    mutsuki_runtime_sdk::TaskSubmitter::cancel_task(&abi, "sdk-abi-task").unwrap();
    assert!(
        mutsuki_runtime_sdk::TaskSubmitter::task_outcome(&abi, "sdk-abi-task")
            .unwrap()
            .is_none()
    );
}

#[test]
fn plugin_backend_groups_task_and_resource_clients_behind_deployment_boundary() {
    let runtime = Arc::new(Mutex::new(
        host_with_echo_runner()
            .into_runtime(runtime_profile())
            .unwrap(),
    ));
    let backend_descriptor = HostExtensionDescriptor {
        extension_id: "host.extension.builtin".into(),
        kind: HostExtensionKind::PluginBackend,
        supported_deployments: vec![PluginDeploymentKind::Builtin],
        reload_policy: "drain_and_swap".into(),
        drain_required: true,
    };
    let host_extension = HostExtension::new(backend_descriptor);
    assert!(host_extension.supports_deployment(&PluginDeploymentKind::Builtin));
    assert!(!host_extension.supports_deployment(&PluginDeploymentKind::Abi));

    let plugin_backend = PluginBackend::new(
        PluginBackendDescriptor {
            backend_id: "plugin.backend.builtin".into(),
            deployment_kind: PluginDeploymentKind::Builtin,
            task_client_protocol: "mutsuki.task.v1".into(),
            resource_client_protocol: "mutsuki.resource-plan.v1".into(),
            codec_id: None,
            bridge_id: None,
        },
        LocalTaskClient::new(runtime.clone()),
        LocalResourceClient::from_provider(PROVIDER_ID, std_memory_provider()),
    );
    let submitted = plugin_backend
        .task_client()
        .submit_task(Task::new("backend-task", "raw.input", json!({})))
        .unwrap();

    assert_eq!(
        plugin_backend.deployment_kind(),
        &PluginDeploymentKind::Builtin
    );
    assert_eq!(submitted.task_id, "backend-task");
}
