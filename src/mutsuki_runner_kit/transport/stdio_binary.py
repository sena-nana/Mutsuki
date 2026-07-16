from __future__ import annotations

import asyncio
import contextlib
import sys
from typing import BinaryIO, TextIO

from mutsuki_runner_kit.runners.backend import PythonRunnerBackend
from mutsuki_runner_kit.transport.dispatcher import (
    ResourceRequestHandler,
    RunnerRequestDispatcher,
)
from mutsuki_runner_kit.transport.stdio_jsonl import _runtime_error
from mutsuki_runner_kit.wire.binary import (
    BinaryRequestFrame,
    encode_binary_response,
    read_binary_request,
)
from mutsuki_runner_kit.wire.generated import MANAGEMENT_OPCODES
from mutsuki_runner_kit.wire.protocol import (
    BINARY_CODEC_ID,
    DEFAULT_WIRE_LIMITS,
    WireLimits,
    WireProtocolFailure,
)
from mutsuki_runner_kit.wire.requests import InitializeRequest


class StdioBinaryBridge:
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
        limits.validate()
        self._dispatcher = RunnerRequestDispatcher(
            runner_backend, resource_handler, BINARY_CODEC_ID, limits
        )
        self._limits = limits
        self._diagnostics = diagnostics if diagnostics is not None else sys.stderr
        self._active_ids: set[int] = set()
        self._active_non_management = 0
        self._shutdown_timeout = shutdown_timeout

    async def serve(self, input_stream: BinaryIO, output_stream: BinaryIO) -> None:
        with contextlib.redirect_stdout(self._diagnostics):
            await self._serve_protocol(input_stream, output_stream)

    async def _serve_protocol(self, input_stream: BinaryIO, output_stream: BinaryIO) -> None:
        writer_lock = asyncio.Lock()
        tasks: set[asyncio.Task[None]] = set()
        while True:
            try:
                frame = await asyncio.to_thread(read_binary_request, input_stream, self._limits)
            except Exception as exc:
                self._diagnostics.write(
                    f"{type(exc).__qualname__}: malformed binary frame; closing input\n"
                )
                self._diagnostics.flush()
                break
            if frame is None:
                break
            capacity_error = self._reserve(frame)
            if capacity_error is not None:
                encoded = encode_binary_response(
                    frame.request_id,
                    frame.opcode,
                    error=_runtime_error(capacity_error),
                    limits=self._limits,
                )
                async with writer_lock:
                    output_stream.write(encoded)
                    output_stream.flush()
                continue
            if isinstance(frame.request, InitializeRequest):
                await self._dispatch_and_write(frame, output_stream, writer_lock)
                self._limits = self._dispatcher.limits
                continue
            task = asyncio.create_task(self._dispatch_and_write(frame, output_stream, writer_lock))
            tasks.add(task)
            task.add_done_callback(tasks.discard)
        if tasks:
            pending_snapshot = list(tasks)
            _, pending = await asyncio.wait(pending_snapshot, timeout=self._shutdown_timeout)
            if pending:
                self._diagnostics.write(
                    f"wire.shutdown_timeout: cancelling {len(pending)} pending requests\n"
                )
                self._diagnostics.flush()
                for task in pending:
                    task.cancel()
            await asyncio.gather(*pending_snapshot, return_exceptions=True)

    def _reserve(self, frame: BinaryRequestFrame) -> WireProtocolFailure | None:
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
            self._limits.max_in_flight_requests - self._limits.management_reserved_requests
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
        frame: BinaryRequestFrame,
        output_stream: BinaryIO,
        writer_lock: asyncio.Lock,
    ) -> None:
        try:
            try:
                result = await self._dispatcher.dispatch(frame.request)
                encoded = encode_binary_response(
                    frame.request_id,
                    frame.opcode,
                    result=result,
                    limits=self._limits,
                )
            except Exception as exc:
                encoded = encode_binary_response(
                    frame.request_id,
                    frame.opcode,
                    error=_runtime_error(exc),
                    limits=self._limits,
                )
            async with writer_lock:
                output_stream.write(encoded)
                output_stream.flush()
        finally:
            self._active_ids.discard(frame.request_id)
            if frame.opcode not in MANAGEMENT_OPCODES:
                self._active_non_management -= 1

    @property
    def limits(self) -> WireLimits:
        return self._limits


def run_stdio_binary_bridge(
    runner_backend: PythonRunnerBackend,
    input_stream: BinaryIO,
    output_stream: BinaryIO,
    resource_handler: ResourceRequestHandler | None = None,
) -> None:
    bridge = StdioBinaryBridge(runner_backend, resource_handler)
    with contextlib.redirect_stdout(sys.stderr):
        asyncio.run(bridge.serve(input_stream, output_stream))
