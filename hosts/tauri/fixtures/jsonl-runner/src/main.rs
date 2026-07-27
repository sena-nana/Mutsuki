use std::io::{self, BufRead, Write};

use mutsuki_runtime_contracts::RuntimeError;
use mutsuki_runtime_wire::{
    DEFAULT_WIRE_LIMITS, JsonlRequestEnvelope, Opcode, ProtocolHello, ProtocolHelloAck,
    encode_jsonl_response,
};
use serde_json::{Value, json};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fail = std::env::args().any(|argument| argument == "--fail");
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: JsonlRequestEnvelope = serde_json::from_str(&line)?;
        let opcode = Opcode::from_u16(request.opcode)?;
        let response = match opcode {
            Opcode::PluginInitialize => {
                let hello: ProtocolHello =
                    serde_json::from_value(request.payload["hello"].clone())?;
                let ack = ProtocolHelloAck::accept(&hello, None);
                encode_jsonl_response(request.request_id, opcode, Ok(&ack), DEFAULT_WIRE_LIMITS)?
            }
            Opcode::RunnerRunBatch if fail => {
                let error: RuntimeError = serde_json::from_value(json!({
                    "code": "fixture.runner_failed",
                    "source": "fixture.runner",
                    "route": "runner.run_batch",
                    "lost_capability": null,
                    "recovery": null,
                    "cause": null,
                    "evidence": {
                        "plugin_id": "fixture.failing_process",
                        "runner_id": "fixture.failing_process.runner"
                    }
                }))?;
                encode_jsonl_response::<Value>(
                    request.request_id,
                    opcode,
                    Err(&error),
                    DEFAULT_WIRE_LIMITS,
                )?
            }
            Opcode::RunnerRunBatch => {
                eprintln!("runner stderr token=secret-token");
                let batch = &request.payload["batch"];
                let results = batch["entries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|entry| {
                        json!({
                            "entry_id": entry["entry_id"],
                            "task_id": entry["task_id"],
                            "result": {
                                "task_id": entry["task_id"],
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
                    "batch_id": batch["batch_id"],
                    "tick_id": batch["tick_id"],
                    "results": results,
                    "metadata": []
                });
                encode_jsonl_response(
                    request.request_id,
                    opcode,
                    Ok(&completion),
                    DEFAULT_WIRE_LIMITS,
                )?
            }
            Opcode::RunnerCancel => {
                encode_jsonl_response(request.request_id, opcode, Ok(&()), DEFAULT_WIRE_LIMITS)?
            }
            Opcode::RunnerDispose => {
                let response = encode_jsonl_response(
                    request.request_id,
                    opcode,
                    Ok(&()),
                    DEFAULT_WIRE_LIMITS,
                )?;
                stdout.write_all(&response)?;
                stdout.flush()?;
                break;
            }
            _ => {
                let error: RuntimeError = serde_json::from_value(json!({
                    "code": "test.unsupported",
                    "source": "test",
                    "route": request.method,
                    "lost_capability": null,
                    "recovery": null,
                    "cause": null,
                    "evidence": {}
                }))?;
                encode_jsonl_response::<Value>(
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
