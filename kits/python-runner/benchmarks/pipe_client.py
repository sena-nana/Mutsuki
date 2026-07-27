from __future__ import annotations

import json
import struct
import subprocess
import sys
import tempfile
import time
from dataclasses import replace
from pathlib import Path
from typing import BinaryIO, Self, cast

from mutsuki_runner_kit.contracts.codec import JsonValue, to_json_dict
from mutsuki_runner_kit.contracts.task import Task
from mutsuki_runner_kit.testing.batches import multi_entry_batch, runner_context
from mutsuki_runner_kit.wire.binary import binary_response_payload, encode_binary_request
from mutsuki_runner_kit.wire.generated import Opcode
from mutsuki_runner_kit.wire.jsonl import encode_jsonl_request
from mutsuki_runner_kit.wire.protocol import BINARY_CODEC_ID, DEBUG_JSONL_CODEC_ID, ProtocolHello


class FixtureProcess:
    def __init__(self, root: Path, codec: str) -> None:
        self.codec = codec
        self._diagnostics = tempfile.TemporaryFile()
        self.process = subprocess.Popen(
            [sys.executable, str(root / "benchmarks/fixture_process.py"), "--codec", codec],
            cwd=root,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self._diagnostics,
        )
        if self.process.stdin is None or self.process.stdout is None:
            raise RuntimeError("fixture process pipes are unavailable")
        self.input = cast(BinaryIO, self.process.stdin)
        self.output = cast(BinaryIO, self.process.stdout)
        self.next_request_id = 1

    def initialize(self) -> int:
        started = time.perf_counter_ns()
        codec_id = DEBUG_JSONL_CODEC_ID if self.codec == "python-jsonl" else BINARY_CODEC_ID
        request_id = self.send(
            Opcode.PLUGIN_INITIALIZE,
            {"hello": ProtocolHello.for_codec(codec_id).to_dict()},
        )
        response = self.receive()
        if response[0] != request_id or not response[1]:
            raise RuntimeError(f"fixture negotiation failed: {response}")
        return time.perf_counter_ns() - started

    def send(self, opcode: Opcode, payload: dict[str, object]) -> int:
        request_id = self.next_request_id
        self.next_request_id += 1
        if self.codec == "python-jsonl":
            encoded = encode_jsonl_request(request_id, opcode, payload)
        else:
            encoded = encode_binary_request(request_id, opcode, payload)
        self.input.write(encoded)
        self.input.flush()
        return request_id

    def receive(self) -> tuple[int, bool, JsonValue]:
        if self.codec == "python-jsonl":
            encoded = self.output.readline()
            if not encoded:
                raise RuntimeError("fixture process closed JSONL output")
            response = json.loads(encoded)
            return (
                cast(int, response["request_id"]),
                cast(bool, response["ok"]),
                cast(JsonValue, response["result"] if response["ok"] else response["error"]),
            )
        prefix = self._read_exact(4)
        (body_len,) = struct.unpack(">I", prefix)
        request_id, _, is_error, payload = binary_response_payload(
            prefix + self._read_exact(body_len)
        )
        return request_id, not is_error, cast(JsonValue, payload)

    def dispatch(self, protocol_id: str, payload: JsonValue, sequence: int) -> int:
        lease_id = f"lease-{sequence}"
        task = replace(
            Task.new(f"task-{sequence}", protocol_id, payload),
            lease_id=lease_id,
        )
        runner_name = protocol_id.removeprefix("runner.")
        runner_id = f"mutsuki.test.abi-fixture.{runner_name}"
        batch = multi_entry_batch(
            (task,),
            lease_ids=(lease_id,),
            batch_id=f"batch-{sequence}",
            tick_id=f"tick-{sequence}",
            runner_id=runner_id,
        )
        ctx = runner_context(
            lease_ids=(lease_id,),
            invocation_id=f"invocation-{sequence}",
            batch_id=batch.batch_id,
            tick_id=f"tick-{sequence}",
        )
        return self.send(
            Opcode.RUNNER_RUN_BATCH,
            {
                "runner_id": runner_id,
                "ctx": to_json_dict(ctx),
                "batch": to_json_dict(batch),
            },
        )

    def cancel(self, protocol_id: str, invocation_id: str) -> int:
        return self.send(
            Opcode.RUNNER_CANCEL,
            {
                "runner_id": (f"mutsuki.test.abi-fixture.{protocol_id.removeprefix('runner.')}"),
                "invocation_id": invocation_id,
            },
        )

    def close(self, timeout: float = 5.0) -> tuple[int, int]:
        started = time.perf_counter_ns()
        if not self.input.closed:
            self.input.close()
        return_code = self.process.wait(timeout=timeout)
        elapsed = time.perf_counter_ns() - started
        return return_code, elapsed

    def kill(self) -> None:
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait()

    def diagnostic_bytes(self) -> int:
        self._diagnostics.flush()
        return self._diagnostics.seek(0, 2)

    def _read_exact(self, length: int) -> bytes:
        value = bytearray()
        while len(value) < length:
            chunk = self.output.read(length - len(value))
            if not chunk:
                raise RuntimeError("fixture process closed binary output")
            value.extend(chunk)
        return bytes(value)

    def __enter__(self) -> Self:
        return self

    def __exit__(self, *_exc: object) -> None:
        self.kill()
        self._diagnostics.close()
