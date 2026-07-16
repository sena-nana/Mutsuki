use serde_json::{Value, json};

use crate::binary::encode_binary_request_value;
use crate::{
    DEFAULT_WIRE_LIMITS, Opcode, SCHEMA_REVISION, encode_binary_response, generated_fixtures_value,
};

pub fn generated_binary_golden_value() -> Value {
    let fixtures = generated_fixtures_value();
    let fixture = |name: &str| {
        fixtures["fixtures"]
            .as_array()
            .expect("fixture registry")
            .iter()
            .find(|item| item["type"] == name)
            .unwrap_or_else(|| panic!("missing fixture {name}"))["value"]
            .clone()
    };
    let resource = json!({
        "ref_id": "r",
        "resource_id": {"kind_id": "k", "slot_id": "s", "generation": 1, "version": 1},
        "semantic": "frozen_value",
        "provider_id": "p",
        "resource_kind": "k",
        "schema": "s",
        "version": 1,
        "generation": 1,
        "access": {"type": "blob", "store_id": "s", "key": "k"},
        "size_hint": 3,
        "content_hash": null,
        "lifetime": "persistent",
        "lease": null,
        "seal_state": "sealed",
    });
    let handle = json!({
        "task_id": "t",
        "protocol_id": "p",
        "target_binding_id": null,
        "cancel_policy": "cascade",
        "trace_id": null,
        "correlation_id": null,
    });
    let read_plan = json!({
        "plan_id": "read-golden-1",
        "resource": resource,
        "operation": "collect",
        "args": {"offset": 0, "length": 3},
    });
    let command_plan = json!({
        "plan_id": "command-golden-1",
        "capability": resource,
        "operation": "fixture.execute",
        "args": {"value": 7},
        "idempotency_key": "golden-command-1",
    });
    let receipt = json!({
        "plan_id": "receipt-golden-1",
        "status": "completed",
        "resource_ref": resource,
        "snapshot": null,
        "descriptor_updates": [],
        "new_version": 4,
        "output": {"ok": true},
    });
    let snapshot = json!({
        "snapshot_ref": resource,
        "source_ref": resource,
        "source_version": 3,
        "snapshot_version": 4,
        "is_stale": false,
        "is_latest": true,
    });
    let operations = vec![
        operation(
            Opcode::PluginInitialize,
            fixture("InitializeRequest"),
            fixture("ProtocolHelloAck"),
        ),
        operation(
            Opcode::RunnerRunBatch,
            json!({
                "runner_id": "r",
                "ctx": {
                    "registry_generation": 1,
                    "current_step": 1,
                    "tick_id": "t",
                    "batch_id": "b",
                    "executor_id": "e",
                    "task_lease_ids": [],
                    "entry_count": 0,
                    "invocation_id": "i",
                    "cancel_token": "c",
                    "deadline_tick": null,
                    "cancel_requested": false,
                },
                "batch": {
                    "batch_id": "b",
                    "tick_id": "t",
                    "batch_key": "k",
                    "entries": [],
                    "payload": {"layout": "row", "payload": {"rows": []}},
                    "resource_plan": {
                        "read_views": [],
                        "write_locks": [],
                        "parallel_groups": [],
                        "serial_groups": [],
                        "parallelism_limit": 1,
                        "version_checks": [],
                        "deferred_writes": [],
                        "conflict_entries": [],
                    },
                    "task_leases": [],
                },
            }),
            json!({"batch_id": "b", "tick_id": "t", "results": [], "metadata": []}),
        ),
        operation(
            Opcode::RunnerCancel,
            fixture("CancelRunnerRequest"),
            Value::Null,
        ),
        operation(
            Opcode::RunnerDispose,
            fixture("DisposeRunnerRequest"),
            Value::Null,
        ),
        operation(
            Opcode::TaskSubmitBatch,
            json!({"batch": {"batch_id": "b", "tick_id": null, "tasks": [], "resource_plan": null}}),
            json!([]),
        ),
        operation(Opcode::TaskCancel, json!({"handle": handle}), Value::Null),
        operation(Opcode::TaskOutcome, json!({"handle": handle}), Value::Null),
        operation(
            Opcode::ResourceReadCollect,
            json!({"provider_id": "fixture.provider", "plan": read_plan}),
            json!([1, 2, 3]),
        ),
        operation(
            Opcode::ResourceReadSnapshot,
            json!({
                "provider_id": "fixture.provider",
                "plan": read_plan,
                "kind_id": "fixture.snapshot",
                "schema": "fixture.snapshot.v1",
            }),
            snapshot,
        ),
        operation(
            Opcode::ResourceStreamOpen,
            json!({"provider_id": "fixture.provider", "plan": read_plan}),
            json!({
                "plan_id": "stream-golden-1",
                "resource": resource,
                "operation": "stream",
                "args": {"chunk_bytes": 4096},
            }),
        ),
        operation(
            Opcode::ResourceExport,
            json!({
                "provider_id": "fixture.provider",
                "plan": {
                    "plan_id": "export-golden-1",
                    "resource": resource,
                    "target": "fixture://export",
                    "args": {},
                },
            }),
            receipt.clone(),
        ),
        operation(
            Opcode::ResourceWriteCommit,
            json!({
                "provider_id": "fixture.provider",
                "plan": {
                    "plan_id": "write-golden-1",
                    "resource": resource,
                    "base_version": 3,
                    "conflict_policy": "reject",
                    "patch": {
                        "patch_id": "patch-golden-1",
                        "target_ref": resource,
                        "base_version": 3,
                        "conflict_policy": "reject",
                        "operations": [{"op": "replace", "value": 7}],
                    },
                    "returning": null,
                },
                "bytes": [1, 2, 3],
            }),
            receipt.clone(),
        ),
        operation(
            Opcode::ResourceCommand,
            json!({"provider_id": "fixture.provider", "plan": command_plan}),
            receipt.clone(),
        ),
        operation(
            Opcode::ResourceCommandBatch,
            json!({
                "provider_id": "fixture.provider",
                "batch": {
                    "batch_id": "command-batch-golden-1",
                    "commands": [command_plan],
                    "rollback_guarantee": false,
                },
            }),
            json!([receipt]),
        ),
        operation(
            Opcode::ResourceSaga,
            json!({
                "provider_id": "fixture.provider",
                "saga": {
                    "saga_id": "saga-golden-1",
                    "steps": [command_plan],
                    "compensations": [command_plan],
                },
            }),
            json!([receipt]),
        ),
        operation(
            Opcode::ResourceCreateBlob,
            json!({
                "provider_id": "fixture.provider",
                "schema": "fixture.bytes.v1",
                "bytes": [1, 2, 3],
            }),
            resource.clone(),
        ),
        operation(
            Opcode::ResourceCreateCowState,
            json!({
                "provider_id": "fixture.provider",
                "kind_id": "fixture.state",
                "schema": "fixture.state.v1",
                "bytes": [1, 2, 3],
            }),
            resource.clone(),
        ),
        operation(
            Opcode::ResourceCreateCapability,
            json!({
                "provider_id": "fixture.provider",
                "kind_id": "fixture.capability",
                "schema": "fixture.capability.v1",
            }),
            resource,
        ),
    ];
    json!({
        "schema_revision": SCHEMA_REVISION,
        "operations": operations,
    })
}

pub fn generated_binary_golden_json() -> String {
    let mut output = serde_json::to_string_pretty(&generated_binary_golden_value())
        .expect("runtime wire binary golden vectors must serialize");
    output.push('\n');
    output
}

fn operation(opcode: Opcode, request: Value, response: Value) -> Value {
    let request_id = 0x1000 + opcode as u64;
    let request_frame =
        encode_binary_request_value(request_id, opcode, &request, DEFAULT_WIRE_LIMITS)
            .expect("golden request must encode");
    let response_frame =
        encode_binary_response(request_id, opcode, Ok(&response), DEFAULT_WIRE_LIMITS)
            .expect("golden response must encode");
    let (request_type, response_type) = super::operation_types(opcode);
    json!({
        "opcode": opcode as u16,
        "method": opcode.method(),
        "request_id": request_id,
        "request_type": request_type,
        "response_type": response_type,
        "request_frame_hex": hex(&request_frame),
        "response_frame_hex": hex(&response_frame),
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
