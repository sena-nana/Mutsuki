from __future__ import annotations

import asyncio
import gc
import io
import struct
import statistics
import time
import tracemalloc
from dataclasses import replace

from mutsuki_runner_kit.contracts.codec import to_json_dict
from mutsuki_runner_kit.contracts.runner import RunnerContext, RunnerDescriptor, RunnerResult
from mutsuki_runner_kit.contracts.task import Task
from mutsuki_runner_kit.runners.backend import PythonRunnerBackend
from mutsuki_runner_kit.testing.batches import multi_entry_batch, runner_context
from mutsuki_runner_kit.testing.runners import EchoRunner, echo_descriptor
from mutsuki_runner_kit.transport.stdio_binary import StdioBinaryBridge
from mutsuki_runner_kit.wire.binary import encode_binary_request
from mutsuki_runner_kit.wire.generated import Opcode
from mutsuki_runner_kit.wire.protocol import BINARY_CODEC_ID, ProtocolHello


class GroupBarrierRunner(EchoRunner):
    def __init__(self, descriptor: RunnerDescriptor, group_size: int) -> None:
        super().__init__(descriptor)
        self._group_size = group_size
        self._arrived = 0
        self._changed = asyncio.Condition()

    async def run_one(self, ctx: RunnerContext, task: Task) -> RunnerResult:
        async with self._changed:
            generation = self._arrived // self._group_size
            self._arrived += 1
            target = (generation + 1) * self._group_size
            self._changed.notify_all()
            await self._changed.wait_for(lambda: self._arrived >= target)
        return await super().run_one(ctx, task)


class CancelObservedRunner(EchoRunner):
    def __init__(self, descriptor: RunnerDescriptor) -> None:
        super().__init__(descriptor)
        self.released = asyncio.Event()
        self.cancel_received_ns = 0

    async def run_one(self, ctx: RunnerContext, task: Task) -> RunnerResult:
        await self.released.wait()
        return await super().run_one(ctx, task)

    async def cancel(self, invocation_id: str) -> None:
        self.cancel_received_ns = time.perf_counter_ns()
        await super().cancel(invocation_id)
        self.released.set()


async def concurrency_case(concurrency: int, groups: int) -> dict[str, int | float]:
    backend = PythonRunnerBackend()
    backend.register_runner(GroupBarrierRunner(echo_descriptor(), concurrency))
    bridge = StdioBinaryBridge(backend)
    await initialize(bridge)
    frames = request_group(concurrency)
    gc.collect()
    gc_was_enabled = gc.isenabled()
    gc.disable()
    tracemalloc.start()
    elapsed_ns = 0
    try:
        for _ in range(groups):
            output = io.BytesIO()
            started = time.perf_counter_ns()
            await bridge.serve(io.BytesIO(frames), output)
            elapsed_ns += time.perf_counter_ns() - started
            if count_frames(output.getvalue()) != concurrency:
                raise RuntimeError("concurrency benchmark lost a response")
        _, peak_bytes = tracemalloc.get_traced_memory()
    finally:
        tracemalloc.stop()
        if gc_was_enabled:
            gc.enable()
    requests = concurrency * groups
    return {
        "concurrency": concurrency,
        "groups": groups,
        "requests": requests,
        "elapsed_ns": elapsed_ns,
        "throughput_per_second": requests / (elapsed_ns / 1_000_000_000),
        "peak_bytes": peak_bytes,
        "peak_bytes_per_request": peak_bytes / concurrency,
    }


async def cancel_case(iterations: int) -> dict[str, int | float]:
    samples: list[int] = []
    for index in range(iterations):
        backend = PythonRunnerBackend()
        runner = CancelObservedRunner(echo_descriptor())
        backend.register_runner(runner)
        bridge = StdioBinaryBridge(backend)
        await initialize(bridge)
        run, invocation_id = run_request(2, index)
        cancel = encode_binary_request(
            3,
            Opcode.RUNNER_CANCEL,
            {"runner_id": "echo.runner", "invocation_id": invocation_id},
        )
        started = time.perf_counter_ns()
        await bridge.serve(io.BytesIO(run + cancel), io.BytesIO())
        samples.append(runner.cancel_received_ns - started)
    samples.sort()
    return {
        "iterations": iterations,
        "p50_ns": int(statistics.median(samples)),
        "p95_ns": samples[max(0, (iterations * 95 + 99) // 100 - 1)],
        "max_ns": samples[-1],
    }


async def initialize(bridge: StdioBinaryBridge) -> None:
    hello = ProtocolHello.for_codec(BINARY_CODEC_ID)
    output = io.BytesIO()
    await bridge.serve(
        io.BytesIO(
            encode_binary_request(1, Opcode.PLUGIN_INITIALIZE, {"hello": hello.to_dict()})
        ),
        output,
    )
    if count_frames(output.getvalue()) != 1:
        raise RuntimeError("benchmark bridge initialization failed")


def request_group(concurrency: int) -> bytes:
    return b"".join(run_request(index + 2, index)[0] for index in range(concurrency))


def run_request(request_id: int, index: int) -> tuple[bytes, str]:
    lease_id = f"lease-{index}"
    task = replace(Task.new(f"task-{index}", "raw.input"), lease_id=lease_id)
    batch = replace(multi_entry_batch((task,), lease_ids=(lease_id,)), batch_id=f"batch-{index}")
    ctx = replace(
        runner_context(lease_ids=(lease_id,), batch_id=batch.batch_id),
        invocation_id=f"invocation-{index}",
    )
    return (
        encode_binary_request(
            request_id,
            Opcode.RUNNER_RUN_BATCH,
            {"runner_id": "echo.runner", "ctx": to_json_dict(ctx), "batch": to_json_dict(batch)},
        ),
        ctx.invocation_id,
    )


def count_frames(encoded: bytes) -> int:
    count = 0
    offset = 0
    while offset < len(encoded):
        body_len = struct.unpack_from(">I", encoded, offset)[0]
        offset += 4 + body_len
        count += 1
    return count
