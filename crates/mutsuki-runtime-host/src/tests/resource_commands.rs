use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::{HostRuntimeCommand, HostRuntimeReply};

use super::helpers::{host_with_echo_runner, runtime_profile};

#[test]
fn host_runtime_executes_resource_plan_commands() {
    let mut runtime = host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();

    let HostRuntimeReply::ResourceCreated(resource) = runtime
        .dispatch(HostRuntimeCommand::CreateBlobResource {
            schema: "text.v1".into(),
            bytes: b"hello".to_vec(),
        })
        .unwrap()
    else {
        panic!("expected resource creation reply");
    };
    let export = ExportPlan {
        plan_id: "export:1".into(),
        resource,
        target: "inline_utf8".into(),
        args: json!(null),
    };

    let HostRuntimeReply::PlanReceipt(export_receipt) = runtime
        .dispatch(HostRuntimeCommand::ExecuteExportPlan(Box::new(export)))
        .unwrap()
    else {
        panic!("expected export receipt");
    };
    assert_eq!(export_receipt.status, "exported");
    assert_eq!(export_receipt.output, json!("hello"));

    let HostRuntimeReply::ResourceCreated(capability) = runtime
        .dispatch(HostRuntimeCommand::CreateCapabilityResource {
            kind_id: "db_pool".into(),
            schema: "db.pool.v1".into(),
        })
        .unwrap()
    else {
        panic!("expected capability creation reply");
    };
    let command = CommandPlan {
        plan_id: "command:1".into(),
        capability,
        operation: "query".into(),
        args: json!({"sql": "select 1"}),
        idempotency_key: Some("query:1".into()),
    };

    let HostRuntimeReply::PlanReceipt(command_receipt) = runtime
        .dispatch(HostRuntimeCommand::ExecuteCommandPlan(Box::new(
            command.clone(),
        )))
        .unwrap()
    else {
        panic!("expected command receipt");
    };
    assert_eq!(command_receipt.status, "commanded");
    assert_eq!(command_receipt.output["operation"], "query");

    let HostRuntimeReply::PlanReceipts(batch_receipts) = runtime
        .dispatch(HostRuntimeCommand::ExecuteCommandBatch(Box::new(
            CommandBatch {
                batch_id: "batch:1".into(),
                commands: vec![command.clone()],
                rollback_guarantee: false,
            },
        )))
        .unwrap()
    else {
        panic!("expected batch receipts");
    };
    assert_eq!(batch_receipts.len(), 1);

    let HostRuntimeReply::PlanReceipts(saga_receipts) = runtime
        .dispatch(HostRuntimeCommand::ExecuteSagaPlan(Box::new(SagaPlan {
            saga_id: "saga:1".into(),
            steps: vec![command.clone()],
            compensations: vec![command],
        })))
        .unwrap()
    else {
        panic!("expected saga receipts");
    };
    assert_eq!(saga_receipts.len(), 1);
}
