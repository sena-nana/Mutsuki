use std::cell::RefCell;
use std::io::{BufRead, Write};

use mutsuki_runtime_contracts::{RunnerDescriptor, RunnerResult, RuntimeError, Task};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeFailure, RuntimeResult};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub struct JsonlRunner<R, W> {
    descriptor: RunnerDescriptor,
    inner: RefCell<JsonlTransport<R, W>>,
}

struct JsonlTransport<R, W> {
    reader: R,
    writer: W,
    next_request: u64,
}

impl<R, W> JsonlRunner<R, W> {
    pub fn new(descriptor: RunnerDescriptor, reader: R, writer: W) -> Self {
        Self {
            descriptor,
            inner: RefCell::new(JsonlTransport {
                reader,
                writer,
                next_request: 0,
            }),
        }
    }

    pub fn into_inner(self) -> (R, W) {
        let inner = self.inner.into_inner();
        (inner.reader, inner.writer)
    }
}

impl<R: BufRead, W: Write> JsonlRunner<R, W> {
    fn request(&self, method: &str, params: Value) -> RuntimeResult<Value> {
        let mut inner = self.inner.borrow_mut();
        inner.next_request += 1;
        let id = format!("req-{}", inner.next_request);
        let request = json!({"id": id, "method": method, "params": params});
        serde_json::to_writer(&mut inner.writer, &request).map_err(protocol_decode_failure)?;
        inner.writer.write_all(b"\n").map_err(io_error_failure)?;
        inner.writer.flush().map_err(io_error_failure)?;

        let mut line = String::new();
        inner
            .reader
            .read_line(&mut line)
            .map_err(io_error_failure)?;
        if line.trim().is_empty() {
            return Err(protocol_error("empty response"));
        }
        let response: Value = serde_json::from_str(&line).map_err(protocol_decode_failure)?;
        if response.get("id") != Some(&Value::String(id)) {
            return Err(protocol_error("response id mismatch"));
        }
        match response.get("ok").and_then(Value::as_bool) {
            Some(true) => Ok(response.get("result").cloned().unwrap_or(Value::Null)),
            Some(false) => {
                let error_value = response
                    .get("error")
                    .cloned()
                    .ok_or_else(|| protocol_error("error response missing error"))?;
                let error = serde_json::from_value(error_value).map_err(protocol_decode_failure)?;
                Err(RuntimeFailure::new(error))
            }
            None => Err(protocol_error("response missing ok flag")),
        }
    }

    fn request_as<T: DeserializeOwned>(&self, method: &str, params: Value) -> RuntimeResult<T> {
        let result = self.request(method, params)?;
        serde_json::from_value(result).map_err(protocol_decode_failure)
    }
}

impl<R: BufRead, W: Write> Runner for JsonlRunner<R, W> {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn step(&mut self, ctx: RunnerContext, tasks: Vec<Task>) -> RuntimeResult<Vec<RunnerResult>> {
        self.request_as(
            "runner.step",
            json!({
                "runner_id": self.descriptor.runner_id,
                "ctx": {
                    "registry_generation": ctx.registry_generation,
                    "current_step": ctx.current_step,
                    "executor_id": ctx.executor_id,
                    "task_lease_id": ctx.task_lease_id
                },
                "tasks": tasks
            }),
        )
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.request(
            "runner.cancel",
            json!({"runner_id": self.descriptor.runner_id, "invocation_id": invocation_id}),
        )?;
        Ok(())
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        self.request(
            "runner.dispose",
            json!({"runner_id": self.descriptor.runner_id}),
        )?;
        Ok(())
    }
}

fn io_error_failure(err: std::io::Error) -> RuntimeFailure {
    host_failure_with_evidence("jsonl.io", "exception_repr", err.to_string())
}

fn protocol_decode_failure(err: serde_json::Error) -> RuntimeFailure {
    host_failure_with_evidence("jsonl.decode", "exception_repr", err.to_string())
}

fn protocol_error(message: &str) -> RuntimeFailure {
    host_failure_with_evidence("jsonl.protocol", "reason", message)
}

fn host_failure_with_evidence(
    route: &str,
    evidence_key: &str,
    evidence_value: impl Into<String>,
) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        "jsonl_runner",
        route,
    );
    error.evidence.insert(
        evidence_key.into(),
        mutsuki_runtime_contracts::ScalarValue::String(evidence_value.into()),
    );
    RuntimeFailure::new(error)
}
