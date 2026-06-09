use std::cell::RefCell;
use std::io::{BufRead, Write};

use mutsuki_runtime_contracts::{
    Envelope, OperationHandlerKey, OperationSnapshot, OperationStatus, RuntimeError,
    SourceSnapshot, StrategyResult,
};
use mutsuki_runtime_core::{
    BackendPayload, OperationBackend, RuntimeFailure, RuntimeResult, StrategyBackend,
};
use serde_json::{Value, json};

pub struct JsonlRuntimeBackend<R, W> {
    inner: RefCell<JsonlTransport<R, W>>,
}

struct JsonlTransport<R, W> {
    reader: R,
    writer: W,
    next_request: u64,
}

impl<R, W> JsonlRuntimeBackend<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self {
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

impl<R: BufRead, W: Write> JsonlRuntimeBackend<R, W> {
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
}

impl<R: BufRead, W: Write> StrategyBackend for JsonlRuntimeBackend<R, W> {
    fn on_awake(&mut self, agent_id: &str) -> RuntimeResult<()> {
        self.request("on_awake", json!({"agent_id": agent_id}))?;
        Ok(())
    }

    fn on_input(&mut self, agent_id: &str, envelope: &Envelope) -> RuntimeResult<StrategyResult> {
        let result = self.request(
            "on_input",
            json!({"agent_id": agent_id, "envelope": envelope}),
        )?;
        serde_json::from_value(result).map_err(protocol_decode_failure)
    }

    fn next_step(&mut self, agent_id: &str) -> RuntimeResult<StrategyResult> {
        let result = self.request("next_step", json!({"agent_id": agent_id}))?;
        serde_json::from_value(result).map_err(protocol_decode_failure)
    }

    fn on_stop(&mut self, agent_id: &str) -> RuntimeResult<()> {
        self.request("on_stop", json!({"agent_id": agent_id}))?;
        Ok(())
    }
}

impl<R: BufRead, W: Write> OperationBackend for JsonlRuntimeBackend<R, W> {
    fn list_operations(&self, _agent_id: &str) -> RuntimeResult<Vec<OperationSnapshot>> {
        let result = self.request("list_operations", json!({"agent_id": _agent_id}))?;
        serde_json::from_value(result).map_err(protocol_decode_failure)
    }

    fn list_sources(&self, _agent_id: &str) -> RuntimeResult<Vec<SourceSnapshot>> {
        let result = self.request("list_sources", json!({"agent_id": _agent_id}))?;
        serde_json::from_value(result).map_err(protocol_decode_failure)
    }

    fn invoke(
        &mut self,
        agent_id: &str,
        key: &OperationHandlerKey,
        payload: Value,
    ) -> RuntimeResult<BackendPayload> {
        let result = self.request(
            "invoke",
            json!({"agent_id": agent_id, "key": key, "payload": payload}),
        )?;
        Ok(BackendPayload::Json(result))
    }

    fn operation_status(&self, _agent_id: &str, _key: &OperationHandlerKey) -> OperationStatus {
        self.request(
            "operation_status",
            json!({"agent_id": _agent_id, "key": _key}),
        )
        .and_then(|result| serde_json::from_value(result).map_err(protocol_decode_failure))
        .unwrap_or(OperationStatus::Unhealthy)
    }
}

fn io_error_failure(err: std::io::Error) -> RuntimeFailure {
    backend_failure_with_evidence("jsonl.io", "exception_repr", err.to_string())
}

fn protocol_decode_failure(err: serde_json::Error) -> RuntimeFailure {
    backend_failure_with_evidence("jsonl.decode", "exception_repr", err.to_string())
}

fn protocol_error(message: &str) -> RuntimeFailure {
    backend_failure_with_evidence("jsonl.protocol", "reason", message)
}

fn backend_failure_with_evidence(
    route: &str,
    evidence_key: &str,
    evidence_value: impl Into<String>,
) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_BACKEND_FAILED,
        "jsonl_runtime_backend",
        route,
    );
    error.evidence.insert(
        evidence_key.into(),
        mutsuki_runtime_contracts::ScalarValue::String(evidence_value.into()),
    );
    RuntimeFailure::new(error)
}
