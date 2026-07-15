from __future__ import annotations

import asyncio
import contextlib
import json
import sys
from collections.abc import Mapping
from typing import TextIO

from mutsuki_runner_kit.contracts.codec import JsonValue
from mutsuki_runner_kit.contracts.errors import ERR_RUNTIME_HOST_FAILED, RuntimeError
from mutsuki_runner_kit.runners.backend import PythonRunnerBackend
from mutsuki_runner_kit.runners.protocol import RunnerInvokeError
from mutsuki_runner_kit.transport.dispatcher import (
    ResourceRequestHandler,
    RunnerRequestDispatcher,
)
from mutsuki_runner_kit.wire.generated import MANAGEMENT_OPCODES, Opcode
from mutsuki_runner_kit.wire.jsonl import (
    JsonlRequestFrame,
    decode_jsonl_request,
    encode_jsonl_response,
    response_dict,
    safe_request_identity,
)
from mutsuki_runner_kit.wire.protocol import (
    DEBUG_JSONL_CODEC_ID,
    DEFAULT_WIRE_LIMITS,
    WireLimits,
    WireProtocolFailure,
)


class StdioJsonlBridge:
    def __init__(
        self,
        runner_backend: PythonRunnerBackend,
        resource_handler: ResourceRequestHandler | None = None,
        *,
        limits: WireLimits = DEFAULT_WIRE_LIMITS,
        diagnostics: TextIO | None = None,
        shutdown_timeout: float = 5.0,
    ) -> None:
        if shutdown_timeout <= 0:
            raise ValueError("shutdown_timeout must be positive")
        self._dispatcher = RunnerRequestDispatcher(
            runner_backend, resource_handler, DEBUG_JSONL_CODEC_ID
        )
        self._limits = limits
        self._diagnostics = diagnostics if diagnostics is not None else sys.stderr
        self._active_ids: set[int] = set()
        self._active_non_management = 0
        self._shutdown_timeout = shutdown_timeout

    async def handle_request(self, request: object) -> dict[str, JsonValue]:
        try:
            if not isinstance(request, Mapping):
                raise TypeError("request expects mapping")
            frame = decode_jsonl_request(request, self._limits)
            result = await self._dispatcher.dispatch(frame.request)
            return response_dict(
                encode_jsonl_response(
                    frame.request_id, frame.opcode, result=result, limits=self._limits
                )
            )
        except Exception as exc:
            identity = safe_request_identity(request)
            if identity is None:
                return _uncorrelated_error(exc)
            request_id, opcode = identity
            return response_dict(
                encode_jsonl_response(
                    request_id,
                    opcode,
                    error=_runtime_error(exc),
                    limits=self._limits,
                )
            )

    async def serve(self, input_stream: TextIO, output_stream: TextIO) -> None:
        with contextlib.redirect_stdout(self._diagnostics):
            await self._serve_protocol(input_stream, output_stream)

    async def _serve_protocol(self, input_stream: TextIO, output_stream: TextIO) -> None:
        writer_lock = asyncio.Lock()
        tasks: set[asyncio.Task[None]] = set()
        while True:
            line = await asyncio.to_thread(
                input_stream.readline, self._limits.max_jsonl_line_bytes + 1
            )
            if line == "":
                break
            if not line.strip():
                continue
            if len(line.encode()) > self._limits.max_jsonl_line_bytes:
                self._diagnostics.write("wire.frame_oversized: closing JSONL input\n")
                self._diagnostics.flush()
                break
            try:
                frame = decode_jsonl_request(line, self._limits)
            except Exception as exc:
                identity = _safe_json_identity(line)
                if identity is None:
                    self._diagnostics.write(
                        f"{type(exc).__qualname__}: malformed uncorrelated JSONL frame\n"
                    )
                    self._diagnostics.flush()
                    continue
                await self._write_error(identity, exc, output_stream, writer_lock)
                continue
            capacity_error = self._reserve(frame)
            if capacity_error is not None:
                await self._write_error(
                    (frame.request_id, frame.opcode),
                    capacity_error,
                    output_stream,
                    writer_lock,
                )
                continue
            task = asyncio.create_task(
                self._dispatch_and_write(frame, output_stream, writer_lock)
            )
            tasks.add(task)
            task.add_done_callback(tasks.discard)
        if tasks:
            pending_snapshot = list(tasks)
            _, pending = await asyncio.wait(
                pending_snapshot, timeout=self._shutdown_timeout
            )
            if pending:
                self._diagnostics.write(
                    f"wire.shutdown_timeout: cancelling {len(pending)} pending requests\n"
                )
                self._diagnostics.flush()
                for task in pending:
                    task.cancel()
            await asyncio.gather(*pending_snapshot, return_exceptions=True)

    def _reserve(self, frame: JsonlRequestFrame) -> WireProtocolFailure | None:
        if frame.request_id in self._active_ids:
            return WireProtocolFailure(
                "wire.request_id_duplicate", f"duplicate request id {frame.request_id}"
            )
        management = frame.opcode in MANAGEMENT_OPCODES
        if len(self._active_ids) >= self._limits.max_in_flight_requests:
            return WireProtocolFailure(
                "wire.pending_exhausted", "maximum in-flight requests reached"
            )
        work_capacity = (
            self._limits.max_in_flight_requests
            - self._limits.management_reserved_requests
        )
        if not management and self._active_non_management >= work_capacity:
            return WireProtocolFailure(
                "wire.pending_exhausted", "non-management in-flight capacity reached"
            )
        self._active_ids.add(frame.request_id)
        if not management:
            self._active_non_management += 1
        return None

    async def _dispatch_and_write(
        self,
        frame: JsonlRequestFrame,
        output_stream: TextIO,
        writer_lock: asyncio.Lock,
    ) -> None:
        try:
            try:
                result = await self._dispatcher.dispatch(frame.request)
                encoded = encode_jsonl_response(
                    frame.request_id,
                    frame.opcode,
                    result=result,
                    limits=self._limits,
                )
            except Exception as exc:
                encoded = encode_jsonl_response(
                    frame.request_id,
                    frame.opcode,
                    error=_runtime_error(exc),
                    limits=self._limits,
                )
            async with writer_lock:
                output_stream.write(encoded.decode())
                output_stream.flush()
        finally:
            self._release(frame)

    async def _write_error(
        self,
        identity: tuple[int, Opcode],
        exc: Exception,
        output_stream: TextIO,
        writer_lock: asyncio.Lock,
    ) -> None:
        request_id, opcode = identity
        encoded = encode_jsonl_response(
            request_id, opcode, error=_runtime_error(exc), limits=self._limits
        )
        async with writer_lock:
            output_stream.write(encoded.decode())
            output_stream.flush()

    def _release(self, frame: JsonlRequestFrame) -> None:
        self._active_ids.discard(frame.request_id)
        if frame.opcode not in MANAGEMENT_OPCODES:
            self._active_non_management -= 1


def run_stdio_bridge(
    runner_backend: PythonRunnerBackend,
    input_stream: TextIO,
    output_stream: TextIO,
) -> None:
    bridge = StdioJsonlBridge(runner_backend)
    with contextlib.redirect_stdout(sys.stderr):
        asyncio.run(bridge.serve(input_stream, output_stream))


def run_stdio_provider_bridge(
    runner_backend: PythonRunnerBackend,
    resource_handler: ResourceRequestHandler,
    input_stream: TextIO,
    output_stream: TextIO,
) -> None:
    bridge = StdioJsonlBridge(runner_backend, resource_handler)
    with contextlib.redirect_stdout(sys.stderr):
        asyncio.run(bridge.serve(input_stream, output_stream))


def _runtime_error(exc: Exception) -> RuntimeError:
    if isinstance(exc, RunnerInvokeError):
        return exc.error
    if isinstance(exc, WireProtocolFailure):
        return RuntimeError(
            code=ERR_RUNTIME_HOST_FAILED,
            source="python_runtime_wire",
            route=exc.code,
            evidence={"reason": exc.code, "detail": exc.detail[:512]},
        )
    return RuntimeError(
        code=ERR_RUNTIME_HOST_FAILED,
        source="python_runtime_wire",
        route="wire.dispatch_failed",
        evidence={"exception_type": type(exc).__qualname__},
    )


def _uncorrelated_error(exc: Exception) -> dict[str, JsonValue]:
    error = _runtime_error(exc)
    from mutsuki_runner_kit.contracts.codec import to_json_dict

    return {
        "request_id": 0,
        "protocol": {"major": 1, "minor": 0},
        "opcode": 0,
        "payload_len": 0,
        "ok": False,
        "result": None,
        "error": to_json_dict(error),
    }


def _safe_json_identity(line: str) -> tuple[int, Opcode] | None:
    try:
        loaded = json.loads(line)
    except json.JSONDecodeError:
        return None
    return safe_request_identity(loaded)


__all__ = [
    "ResourceRequestHandler",
    "StdioJsonlBridge",
    "run_stdio_bridge",
    "run_stdio_provider_bridge",
]
