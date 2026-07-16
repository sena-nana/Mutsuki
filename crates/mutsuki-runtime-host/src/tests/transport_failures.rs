use std::io::Cursor;

use mutsuki_runtime_contracts::ScalarValue;
use mutsuki_runtime_wire::{
    DEFAULT_WIRE_LIMITS, DisposeRunnerRequest, Opcode, ProtocolHello, encode_jsonl_response,
};

use crate::JsonlTransport;

#[test]
fn eof_completes_every_pending_request() {
    let transport = JsonlTransport::new(
        Cursor::new(initialized_responses()),
        Cursor::new(Vec::<u8>::new()),
    );
    initialize(&transport);
    let errors = std::thread::scope(|scope| {
        let mut requests = Vec::new();
        for _ in 0..2 {
            let transport = transport.clone();
            requests.push(scope.spawn(move || {
                transport
                    .request(&dispose_request())
                    .expect_err("EOF must fail pending request")
            }));
        }
        requests
            .into_iter()
            .map(|request| request.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(errors.iter().all(|error| reason(error).contains("EOF")));
    let _ = transport.into_inner();
}

#[test]
fn stdout_pollution_fails_transport_with_bounded_diagnostic() {
    let mut responses = initialized_responses();
    responses.extend_from_slice(b"this is a log line, not a protocol frame\n");
    let transport = JsonlTransport::new(Cursor::new(responses), Cursor::new(Vec::<u8>::new()));
    initialize(&transport);

    let error = transport.request(&dispose_request()).unwrap_err();

    assert!(reason(&error).contains("malformed JSONL response"));
    let _ = transport.into_inner();
}

#[test]
fn unknown_or_late_response_id_fails_transport() {
    let mut responses = initialized_responses();
    responses.extend(
        encode_jsonl_response(99, Opcode::RunnerDispose, Ok(&()), DEFAULT_WIRE_LIMITS).unwrap(),
    );
    let transport = JsonlTransport::new(Cursor::new(responses), Cursor::new(Vec::<u8>::new()));
    initialize(&transport);

    let error = transport.request(&dispose_request()).unwrap_err();

    assert!(reason(&error).contains("unknown, duplicate or late response id"));
    let _ = transport.into_inner();
}

#[test]
fn typed_response_decode_failure_poison_future_requests() {
    let mut responses = initialized_responses();
    responses.extend(
        encode_jsonl_response(3, Opcode::RunnerCancel, Ok(&()), DEFAULT_WIRE_LIMITS).unwrap(),
    );
    let transport = JsonlTransport::new(Cursor::new(responses), Cursor::new(Vec::<u8>::new()));
    initialize(&transport);

    let first = transport.request(&dispose_request()).unwrap_err();
    let second = transport.request(&dispose_request()).unwrap_err();

    assert_eq!(first.error(), second.error());
    let _ = transport.into_inner();
}

fn initialized_responses() -> Vec<u8> {
    let hello = ProtocolHello::debug_jsonl();
    let ack = hello
        .accept(mutsuki_runtime_wire::DEBUG_JSONL_CODEC_ID, None)
        .unwrap();
    let mut responses =
        encode_jsonl_response(1, Opcode::PluginInitialize, Ok(&ack), DEFAULT_WIRE_LIMITS).unwrap();
    responses.extend(
        encode_jsonl_response(2, Opcode::RunnerDispose, Ok(&()), DEFAULT_WIRE_LIMITS).unwrap(),
    );
    responses
}

fn initialize<R, W>(transport: &JsonlTransport<R, W>)
where
    R: std::io::BufRead + Send + 'static,
    W: std::io::Write + Send + 'static,
{
    transport.request(&dispose_request()).unwrap();
}

fn dispose_request() -> DisposeRunnerRequest {
    DisposeRunnerRequest {
        runner_id: "jsonl.runner".into(),
    }
}

fn reason(error: &mutsuki_runtime_core::RuntimeFailure) -> &str {
    match error.error().evidence.get("reason") {
        Some(ScalarValue::String(reason)) => reason,
        _ => "",
    }
}
