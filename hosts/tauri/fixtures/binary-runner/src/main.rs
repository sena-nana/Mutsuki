use std::io::{self, Write};

use mutsuki_runtime_contracts::RuntimeError;
use mutsuki_runtime_wire::{
    AnyWireRequest, DEFAULT_WIRE_LIMITS, ProtocolHelloAck, decode_binary_any_request,
    encode_binary_response, read_binary_frame_bytes,
};
use serde_json::{Value, json};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fail = std::env::args().any(|argument| argument == "--fail");
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut stdout = io::stdout().lock();

    while let Some(frame) = read_binary_frame_bytes(&mut input, DEFAULT_WIRE_LIMITS)? {
        let request = decode_binary_any_request(&frame, DEFAULT_WIRE_LIMITS)?;
        let opcode = request.request.opcode();
        let response = match request.request {
            AnyWireRequest::Initialize(initialize) => {
                let ack = ProtocolHelloAck::accept(&initialize.hello, None);
                encode_binary_response(request.request_id, opcode, Ok(&ack), DEFAULT_WIRE_LIMITS)?
            }
            AnyWireRequest::RunBatch(_) if fail => {
                let error = runtime_error(
                    "fixture.runner_failed",
                    "fixture.runner",
                    "runner.run_batch",
                    json!({
                        "plugin_id": "fixture.failing_process",
                        "runner_id": "fixture.failing_process.runner"
                    }),
                )?;
                encode_binary_response::<Value>(
                    request.request_id,
                    opcode,
                    Err(&error),
                    DEFAULT_WIRE_LIMITS,
                )?
            }
            AnyWireRequest::RunBatch(run) => {
                eprintln!("runner stderr token=secret-token");
                let results = run
                    .batch
                    .entries
                    .iter()
                    .map(|entry| {
                        json!({
                            "entry_id": entry.entry_id,
                            "task_id": entry.task_id,
                            "result": {
                                "task_id": entry.task_id,
                                "output": null,
                                "deltas": [],
                                "events": [],
                                "tasks": [],
                                "effects": [],
                                "values": [],
                                "resources": [],
                                "task_await": null,
                                "status": "completed"
                            },
                            "error": null
                        })
                    })
                    .collect::<Vec<_>>();
                let completion = json!({
                    "batch_id": run.batch.batch_id,
                    "tick_id": run.batch.tick_id,
                    "results": results,
                    "metadata": []
                });
                encode_binary_response(
                    request.request_id,
                    opcode,
                    Ok(&completion),
                    DEFAULT_WIRE_LIMITS,
                )?
            }
            AnyWireRequest::CancelRunner(_) => {
                encode_binary_response(request.request_id, opcode, Ok(&()), DEFAULT_WIRE_LIMITS)?
            }
            AnyWireRequest::DisposeRunner(_) => {
                let response = encode_binary_response(
                    request.request_id,
                    opcode,
                    Ok(&()),
                    DEFAULT_WIRE_LIMITS,
                )?;
                stdout.write_all(&response)?;
                stdout.flush()?;
                break;
            }
            other => {
                let error = runtime_error(
                    "test.unsupported",
                    "test",
                    format!("opcode.{:#06x}", other.opcode() as u16),
                    json!({}),
                )?;
                encode_binary_response::<Value>(
                    request.request_id,
                    opcode,
                    Err(&error),
                    DEFAULT_WIRE_LIMITS,
                )?
            }
        };
        stdout.write_all(&response)?;
        stdout.flush()?;
    }
    Ok(())
}

fn runtime_error(
    code: &str,
    source: &str,
    route: impl Into<String>,
    evidence: Value,
) -> Result<RuntimeError, serde_json::Error> {
    serde_json::from_value(json!({
        "code": code,
        "source": source,
        "route": route.into(),
        "lost_capability": null,
        "recovery": null,
        "cause": null,
        "evidence": evidence
    }))
}
