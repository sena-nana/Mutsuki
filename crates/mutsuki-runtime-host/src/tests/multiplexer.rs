use std::io::{BufRead, Cursor, Read, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use mutsuki_runtime_contracts::{
    BatchEntry, BatchPayload, CompletionBatch, ERR_RUNTIME_HOST_FAILED, EntryCompletion,
    OrderingRequirement, RunnerContext, RunnerResult, Task, TaskLease, WorkBatch, WorkResourcePlan,
};
use mutsuki_runtime_wire::{
    BINARY_CODEC_ID, CancelRunnerRequest, DEFAULT_WIRE_LIMITS, DisposeRunnerRequest, Opcode,
    ProtocolHello, RunBatchRequest, WireLimits, encode_binary_response,
};
use serde_json::json;

use crate::BinaryTransport;

#[test]
fn cancel_and_dispose_survive_work_saturation_and_out_of_order_response() {
    let run = run_request();
    let limits = WireLimits {
        max_in_flight_requests: 3,
        management_reserved_requests: 2,
        ..DEFAULT_WIRE_LIMITS
    };
    let hello = ProtocolHello::binary_with_limits(limits).unwrap();
    let ack = hello.accept(BINARY_CODEC_ID, None).unwrap();
    let completion = completion();
    let mut responses =
        encode_binary_response(1, Opcode::PluginInitialize, Ok(&ack), DEFAULT_WIRE_LIMITS).unwrap();
    let gate_after = responses.len();
    responses.extend(
        encode_binary_response(5, Opcode::RunnerDispose, Ok(&()), DEFAULT_WIRE_LIMITS).unwrap(),
    );
    responses.extend(
        encode_binary_response(4, Opcode::RunnerCancel, Ok(&()), DEFAULT_WIRE_LIMITS).unwrap(),
    );
    responses.extend(
        encode_binary_response(
            2,
            Opcode::RunnerRunBatch,
            Ok(&completion),
            DEFAULT_WIRE_LIMITS,
        )
        .unwrap(),
    );

    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let writer_state = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
    let reader = GatedReader {
        cursor: Cursor::new(responses),
        gate_after,
        gate: gate.clone(),
    };
    let writer = SharedWriter(writer_state.clone());
    let bridge =
        BinaryTransport::with_limits(reader, writer, limits, Duration::from_secs(2)).unwrap();

    let run_thread = {
        let bridge = bridge.clone();
        let run = run.clone();
        std::thread::spawn(move || bridge.request(&run))
    };
    wait_for_frames(&writer_state, 2);

    let saturated = bridge.request(&run).unwrap_err();
    assert_eq!(saturated.error().code, ERR_RUNTIME_HOST_FAILED);
    assert_eq!(saturated.error().source, "wire_multiplexer");
    assert_eq!(saturated.error().route, "wire.transport");

    let cancel_thread = {
        let bridge = bridge.clone();
        std::thread::spawn(move || {
            bridge.request(&CancelRunnerRequest {
                runner_id: "binary.runner".into(),
                invocation_id: "invocation:test".into(),
            })
        })
    };
    wait_for_frames(&writer_state, 3);
    let dispose_thread = {
        let bridge = bridge.clone();
        std::thread::spawn(move || {
            bridge.request(&DisposeRunnerRequest {
                runner_id: "binary.runner".into(),
            })
        })
    };
    wait_for_frames(&writer_state, 4);
    let (open, wake) = &*gate;
    *open.lock().unwrap() = true;
    wake.notify_all();

    cancel_thread.join().unwrap().unwrap();
    dispose_thread.join().unwrap().unwrap();
    assert_eq!(run_thread.join().unwrap().unwrap(), completion);
}

#[test]
fn duplicate_or_late_response_fails_the_connection_and_pending_request() {
    let hello = ProtocolHello::binary();
    let ack = hello.accept(BINARY_CODEC_ID, None).unwrap();
    let mut responses =
        encode_binary_response(1, Opcode::PluginInitialize, Ok(&ack), DEFAULT_WIRE_LIMITS).unwrap();
    let dispose =
        encode_binary_response(2, Opcode::RunnerDispose, Ok(&()), DEFAULT_WIRE_LIMITS).unwrap();
    responses.extend_from_slice(&dispose);
    responses.extend_from_slice(&dispose);
    let bridge = BinaryTransport::with_limits(
        Cursor::new(responses),
        Vec::new(),
        DEFAULT_WIRE_LIMITS,
        Duration::from_secs(2),
    )
    .unwrap();
    let request = DisposeRunnerRequest {
        runner_id: "binary.runner".into(),
    };

    bridge.request(&request).unwrap();
    let error = bridge.request(&request).unwrap_err();

    assert!(error.error().evidence.values().any(|value| {
        matches!(value, mutsuki_runtime_contracts::ScalarValue::String(reason) if reason.contains("duplicate or late"))
    }));
    assert!(bridge.request(&request).is_err());
}

fn run_request() -> RunBatchRequest {
    let mut task = Task::new("task-1", "raw.input", json!({}));
    task.lease_id = Some("lease-1".into());
    let batch = WorkBatch {
        batch_id: "batch-1".into(),
        tick_id: "tick-1".into(),
        batch_key: "binary.runner".into(),
        entries: vec![BatchEntry {
            entry_id: "task-1".into(),
            task_id: "task-1".into(),
            trace_id: None,
            parent_id: None,
            payload_index: 0,
            resource_requirement_indices: Vec::new(),
            cancel_index: Some(0),
            deadline_tick: None,
            priority: 0,
            lane: mutsuki_runtime_contracts::DispatchLane::Normal,
            ordering: OrderingRequirement::None,
        }],
        payload: BatchPayload::from_tasks(&[task]),
        resource_plan: WorkResourcePlan::empty(),
        task_leases: vec![TaskLease {
            lease_id: "lease-1".into(),
            task_id: "task-1".into(),
            runner_id: "binary.runner".into(),
            attempt_generation: 1,
            executor_id: "executor:test".into(),
            registry_generation: 1,
            acquired_at_step: 1,
            expires_at_step: None,
        }],
    };
    RunBatchRequest {
        runner_id: "binary.runner".into(),
        ctx: RunnerContext::new(
            1,
            1,
            "executor:test",
            Some("lease-1".into()),
            "invocation:test",
        ),
        batch,
    }
}

fn completion() -> CompletionBatch {
    CompletionBatch {
        batch_id: "batch-1".into(),
        tick_id: "tick-1".into(),
        results: vec![EntryCompletion {
            entry_id: "task-1".into(),
            task_id: "task-1".into(),
            result: Some(RunnerResult::completed("task-1")),
            error: None,
        }],
        metadata: Vec::new(),
    }
}

struct GatedReader {
    cursor: Cursor<Vec<u8>>,
    gate_after: usize,
    gate: Arc<(Mutex<bool>, Condvar)>,
}

impl Read for GatedReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let position = self.cursor.position() as usize;
        if position >= self.gate_after {
            self.wait_for_gate();
            return self.cursor.read(buffer);
        }
        let readable = (self.gate_after - position).min(buffer.len());
        self.cursor.read(&mut buffer[..readable])
    }
}

impl BufRead for GatedReader {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.cursor.position() as usize >= self.gate_after {
            self.wait_for_gate();
            return self.cursor.fill_buf();
        }
        let position = self.cursor.position() as usize;
        let readable = self.gate_after - position;
        let bytes = self.cursor.fill_buf()?;
        Ok(&bytes[..bytes.len().min(readable)])
    }

    fn consume(&mut self, amount: usize) {
        self.cursor.consume(amount);
    }
}

impl GatedReader {
    fn wait_for_gate(&self) {
        let (open, wake) = &*self.gate;
        drop(
            wake.wait_while(open.lock().unwrap(), |open| !*open)
                .unwrap(),
        );
    }
}

#[derive(Clone)]
struct SharedWriter(Arc<(Mutex<Vec<u8>>, Condvar)>);

impl Write for SharedWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let (bytes, wake) = &*self.0;
        bytes.lock().unwrap().extend_from_slice(buffer);
        wake.notify_all();
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn wait_for_frames(state: &Arc<(Mutex<Vec<u8>>, Condvar)>, expected: usize) {
    let (bytes, wake) = &**state;
    let (bytes, timeout) = wake
        .wait_timeout_while(bytes.lock().unwrap(), Duration::from_secs(2), |bytes| {
            count_binary_frames(bytes) < expected
        })
        .unwrap();
    assert!(
        !timeout.timed_out(),
        "writer did not emit {expected} frames"
    );
    assert_eq!(count_binary_frames(&bytes), expected);
}

fn count_binary_frames(bytes: &[u8]) -> usize {
    let mut offset = 0;
    let mut frames = 0;
    while offset + 4 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        if offset + 4 + len > bytes.len() {
            break;
        }
        frames += 1;
        offset += 4 + len;
    }
    frames
}
