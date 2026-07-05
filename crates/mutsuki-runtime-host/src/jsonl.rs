use std::io::{BufRead, Write};
use std::sync::Mutex;

use mutsuki_runtime_contracts::{
    CommandBatch, CommandPlan, CompletionBatch, ExportPlan, PlanReceipt, RunnerDescriptor,
    RuntimeError, SagaPlan, ScalarValue, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeFailure, RuntimeResult};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub struct JsonlRunner<R, W> {
    descriptor: RunnerDescriptor,
    bridge: JsonlBridge<R, W>,
}

pub(crate) struct JsonlBridge<R, W> {
    inner: Mutex<JsonlTransport<R, W>>,
}

struct JsonlTransport<R, W> {
    reader: R,
    writer: W,
    next_request: u64,
}

impl<R, W> JsonlBridge<R, W> {
    pub(crate) fn new(reader: R, writer: W) -> Self {
        Self {
            inner: Mutex::new(JsonlTransport {
                reader,
                writer,
                next_request: 0,
            }),
        }
    }

    pub(crate) fn into_inner(self) -> (R, W) {
        let inner = self
            .inner
            .into_inner()
            .expect("jsonl bridge mutex poisoned");
        (inner.reader, inner.writer)
    }
}

impl<R, W> JsonlRunner<R, W> {
    pub fn new(descriptor: RunnerDescriptor, reader: R, writer: W) -> Self {
        Self {
            descriptor,
            bridge: JsonlBridge::new(reader, writer),
        }
    }

    pub fn into_inner(self) -> (R, W) {
        self.bridge.into_inner()
    }
}

impl<R: BufRead, W: Write> JsonlBridge<R, W> {
    pub(crate) fn request(&self, method: &str, params: Value) -> RuntimeResult<Value> {
        let mut inner = self.inner.lock().expect("jsonl bridge mutex poisoned");
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

    pub(crate) fn request_as<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> RuntimeResult<T> {
        let result = self.request(method, params)?;
        serde_json::from_value(result).map_err(protocol_decode_failure)
    }
}

impl<R: BufRead, W: Write> JsonlRunner<R, W> {
    pub fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        self.bridge
            .request_as("resource.export", json!({ "plan": plan }))
    }

    pub fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        self.bridge
            .request_as("resource.command", json!({ "plan": plan }))
    }

    pub fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        self.bridge
            .request_as("resource.command_batch", json!({ "batch": batch }))
    }

    pub fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        self.bridge
            .request_as("resource.saga", json!({ "saga": saga }))
    }
}

impl<R: BufRead + Send, W: Write + Send> Runner for JsonlRunner<R, W> {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        validate_batch_leases(&ctx, &batch)?;
        self.bridge.request_as(
            "runner.run_batch",
            json!({
                "runner_id": self.descriptor.runner_id,
                "ctx": ctx,
                "batch": batch
            }),
        )
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.bridge.request(
            "runner.cancel",
            json!({"runner_id": self.descriptor.runner_id, "invocation_id": invocation_id}),
        )?;
        Ok(())
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        self.bridge.request(
            "runner.dispose",
            json!({"runner_id": self.descriptor.runner_id}),
        )?;
        Ok(())
    }
}

fn validate_batch_leases(ctx: &RunnerContext, batch: &WorkBatch) -> RuntimeResult<()> {
    let batch_lease_ids = batch
        .task_leases
        .iter()
        .map(|lease| lease.lease_id.clone())
        .collect::<Vec<_>>();
    if batch_lease_ids != ctx.task_lease_ids {
        let mut error = RuntimeError::new(
            mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT,
            "jsonl_runner",
            format!("runner.run_batch.{}", batch.batch_id),
        );
        error.evidence.insert(
            "ctx_task_lease_ids".into(),
            ScalarValue::String(ctx.task_lease_ids.join(",")),
        );
        error.evidence.insert(
            "batch_task_lease_ids".into(),
            ScalarValue::String(batch_lease_ids.join(",")),
        );
        error.evidence.insert(
            "executor_id".into(),
            ScalarValue::String(ctx.executor_id.clone()),
        );
        return Err(RuntimeFailure::new(error));
    }
    Ok(())
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
