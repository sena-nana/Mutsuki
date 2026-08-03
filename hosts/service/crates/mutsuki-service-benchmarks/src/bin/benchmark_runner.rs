use std::io::{self, Write};

use mutsuki_runtime_wire::{
    AnyWireRequest, BINARY_CODEC_ID, DEFAULT_WIRE_LIMITS, decode_binary_any_request,
    encode_binary_response, read_binary_frame_bytes,
};

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    let mut stdin = stdin.lock();
    while let Some(frame) =
        read_binary_frame_bytes(&mut stdin, DEFAULT_WIRE_LIMITS).expect("read Runtime Wire request")
    {
        let decoded = decode_binary_any_request(&frame, DEFAULT_WIRE_LIMITS)
            .expect("decode Runtime Wire request");
        let opcode = decoded.request.opcode();
        let response = match decoded.request {
            AnyWireRequest::Initialize(request) => {
                let ack = request
                    .hello
                    .accept(BINARY_CODEC_ID, None)
                    .expect("accept Runtime Wire hello");
                encode_binary_response(decoded.request_id, opcode, Ok(&ack), DEFAULT_WIRE_LIMITS)
            }
            AnyWireRequest::RunBatch(request) => {
                let mut runner =
                    mutsuki_service_benchmarks::FixtureRunner::new(fixture_descriptor());
                let result = mutsuki_runtime_core::Runner::run_batch(
                    &mut runner,
                    request.ctx,
                    request.batch,
                );
                match result {
                    Ok(batch) => encode_binary_response(
                        decoded.request_id,
                        opcode,
                        Ok(&batch),
                        DEFAULT_WIRE_LIMITS,
                    ),
                    Err(error) => {
                        encode_binary_response::<mutsuki_runtime_contracts::CompletionBatch>(
                            decoded.request_id,
                            opcode,
                            Err(error.error()),
                            DEFAULT_WIRE_LIMITS,
                        )
                    }
                }
            }
            AnyWireRequest::CancelRunner(_) | AnyWireRequest::DisposeRunner(_) => {
                encode_binary_response(decoded.request_id, opcode, Ok(&()), DEFAULT_WIRE_LIMITS)
            }
            _ => panic!("unsupported benchmark Runner operation {opcode:?}"),
        }
        .expect("encode Runtime Wire response");
        stdout.write_all(&response).expect("write response");
        stdout.flush().expect("flush response");
    }
}

fn fixture_descriptor() -> mutsuki_runtime_contracts::RunnerDescriptor {
    mutsuki_service_abi_fixture::benchmark_manifest("benchmark", "sha256:benchmark")
        .provides
        .runners
        .into_iter()
        .next()
        .expect("fixture runner descriptor")
}
