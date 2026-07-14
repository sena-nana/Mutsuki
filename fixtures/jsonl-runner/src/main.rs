use std::io::{self, BufRead, Write};

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
        let request: Value = serde_json::from_str(&line)?;
        let id = request["id"].clone();
        let method = request["method"].as_str().unwrap_or_default();
        let response = match method {
            "runner.run_batch" if fail => json!({
                "id": id,
                "ok": false,
                "error": {
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
                }
            }),
            "runner.run_batch" => {
                eprintln!("runner stderr token=secret-token");
                let batch = &request["params"]["batch"];
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
                json!({
                    "id": id,
                    "ok": true,
                    "result": {
                        "batch_id": batch["batch_id"],
                        "tick_id": batch["tick_id"],
                        "results": results,
                        "metadata": []
                    }
                })
            }
            "runner.cancel" => json!({ "id": id, "ok": true, "result": null }),
            "runner.dispose" => {
                let response = json!({ "id": id, "ok": true, "result": null });
                writeln!(stdout, "{}", serde_json::to_string(&response)?)?;
                stdout.flush()?;
                break;
            }
            _ => json!({
                "id": id,
                "ok": false,
                "error": {
                    "code": "test.unsupported",
                    "source": "test",
                    "route": method,
                    "lost_capability": null,
                    "recovery": null,
                    "cause": null,
                    "evidence": {}
                }
            }),
        };
        writeln!(stdout, "{}", serde_json::to_string(&response)?)?;
        stdout.flush()?;
    }
    Ok(())
}
