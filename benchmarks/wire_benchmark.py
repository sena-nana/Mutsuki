from __future__ import annotations

import asyncio
import io
import json
import statistics
import subprocess
import sys
import time
import tracemalloc
from dataclasses import replace

import msgpack

from mutsuki_runner_kit.contracts.codec import to_json_dict
from mutsuki_runner_kit.contracts.resource import (
    ResourceAccess,
    ResourceId,
    ResourceLifetime,
    ResourceRef,
    ResourceSealState,
    ResourceSemantic,
)
from mutsuki_runner_kit.contracts.runner import RunnerContext, RunnerDescriptor, RunnerResult
from mutsuki_runner_kit.contracts.task import Task
from mutsuki_runner_kit.runners.backend import PythonRunnerBackend
from mutsuki_runner_kit.testing.batches import multi_entry_batch, runner_context
from mutsuki_runner_kit.testing.runners import EchoRunner, echo_descriptor
from mutsuki_runner_kit.transport.stdio_jsonl import StdioJsonlBridge
from mutsuki_runner_kit.wire.binary import decode_binary_request, encode_binary_request
from mutsuki_runner_kit.wire.generated import CORE_WIRE_REVISION, Opcode
from mutsuki_runner_kit.wire.jsonl import decode_jsonl_request, encode_jsonl_request
from mutsuki_runner_kit.wire.protocol import DEBUG_JSONL_CODEC_ID, SCHEMA_REVISION, ProtocolHello


class BlockingRunner(EchoRunner):
    def __init__(self, descriptor: RunnerDescriptor) -> None:
        super().__init__(descriptor)
        self.started = asyncio.Event()
        self.released = asyncio.Event()
        self.cancel_received_ns = 0

    async def run_one(self, ctx: RunnerContext, task: Task) -> RunnerResult:
        self.started.set()
        await self.released.wait()
        return await super().run_one(ctx, task)

    async def cancel(self, invocation_id: str) -> None:
        self.cancel_received_ns = time.perf_counter_ns()
        await super().cancel(invocation_id)
        self.released.set()


def benchmark_codec(
    codec: str, batch_size: int, payload_bytes: int, iterations: int
) -> dict[str, int | float | str]:
    tasks = tuple(
        replace(
            Task.new(f"task-{index}", "fixture.echo", {"data": "x" * payload_bytes}),
            lease_id=f"lease-{index}",
        )
        for index in range(batch_size)
    )
    lease_ids = tuple(f"lease-{index}" for index in range(batch_size))
    batch = multi_entry_batch(tasks, lease_ids=lease_ids)
    ctx = runner_context(lease_ids=lease_ids, batch_id=batch.batch_id)
    payload = {
        "runner_id": "echo.runner",
        "ctx": to_json_dict(ctx),
        "batch": to_json_dict(batch),
    }
    encoder = encode_jsonl_request if codec == "typed_jsonl" else encode_binary_request
    decoder = decode_jsonl_request if codec == "typed_jsonl" else decode_binary_request

    tracemalloc.start()
    started = time.perf_counter_ns()
    encoded = b""
    for request_id in range(1, iterations + 1):
        encoded = encoder(request_id, Opcode.RUNNER_RUN_BATCH, payload)
    encode_ns = time.perf_counter_ns() - started
    _, encode_peak = tracemalloc.get_traced_memory()
    tracemalloc.reset_peak()
    started = time.perf_counter_ns()
    for _ in range(iterations):
        decoder(encoded)
    decode_ns = time.perf_counter_ns() - started
    _, decode_peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    total_mb = len(encoded) * iterations / (1024 * 1024)
    return {
        "codec": codec,
        "batch_size": batch_size,
        "payload_bytes_per_entry": payload_bytes,
        "iterations": iterations,
        "frame_bytes": len(encoded),
        "encode_us_p50": encode_ns / iterations / 1_000,
        "decode_us_p50": decode_ns / iterations / 1_000,
        "encode_mib_s": total_mb / (encode_ns / 1_000_000_000),
        "decode_mib_s": total_mb / (decode_ns / 1_000_000_000),
        "encode_peak_bytes": encode_peak,
        "decode_peak_bytes": decode_peak,
    }


async def benchmark_cancel(iterations: int) -> dict[str, float | int]:
    latencies_ms: list[float] = []
    for index in range(iterations):
        backend = PythonRunnerBackend()
        runner = BlockingRunner(echo_descriptor())
        backend.register_runner(runner)
        bridge = StdioJsonlBridge(backend)
        await bridge.handle_request(
            json.loads(
                encode_jsonl_request(
                    1,
                    Opcode.PLUGIN_INITIALIZE,
                    {"hello": ProtocolHello.for_codec(DEBUG_JSONL_CODEC_ID).to_dict()},
                )
            )
        )
        task = replace(
            Task.new(f"cancel-task-{index}", "raw.input"), lease_id="lease-cancel"
        )
        batch = multi_entry_batch((task,), lease_ids=("lease-cancel",))
        ctx = runner_context(
            lease_ids=("lease-cancel",), batch_id=batch.batch_id
        )
        run = encode_jsonl_request(
            2,
            Opcode.RUNNER_RUN_BATCH,
            {
                "runner_id": "echo.runner",
                "ctx": to_json_dict(ctx),
                "batch": to_json_dict(batch),
            },
        )
        cancel = encode_jsonl_request(
            3,
            Opcode.RUNNER_CANCEL,
            {"runner_id": "echo.runner", "invocation_id": ctx.invocation_id},
        )
        started = time.perf_counter_ns()
        await bridge.serve(io.StringIO((run + cancel).decode()), io.StringIO())
        latencies_ms.append((runner.cancel_received_ns - started) / 1_000_000)
    return {
        "iterations": iterations,
        "p50_ms": statistics.median(latencies_ms),
        "p95_ms": sorted(latencies_ms)[max(0, int(iterations * 0.95) - 1)],
        "max_ms": max(latencies_ms),
    }


async def benchmark_runner_reuse(iterations: int) -> dict[str, float | int]:
    backend = PythonRunnerBackend()
    backend.register_runner(EchoRunner(echo_descriptor()))
    samples_ms: list[float] = []
    for index in range(iterations):
        lease_id = f"reuse-lease-{index}"
        task = replace(
            Task.new(f"reuse-task-{index}", "raw.input"), lease_id=lease_id
        )
        batch = multi_entry_batch((task,), lease_ids=(lease_id,))
        ctx = runner_context(lease_ids=(lease_id,), batch_id=batch.batch_id)
        started = time.perf_counter_ns()
        await backend.run_batch_runner("echo.runner", ctx, batch)
        samples_ms.append((time.perf_counter_ns() - started) / 1_000_000)
    return {
        "iterations": iterations,
        "p50_ms": statistics.median(samples_ms),
        "p95_ms": sorted(samples_ms)[max(0, int(iterations * 0.95) - 1)],
        "max_ms": max(samples_ms),
    }


def benchmark_startup(iterations: int) -> dict[str, float | int]:
    samples_ms: list[float] = []
    for _ in range(iterations):
        started = time.perf_counter_ns()
        subprocess.run(
            [sys.executable, "-c", "pass"],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        samples_ms.append((time.perf_counter_ns() - started) / 1_000_000)
    return {
        "iterations": iterations,
        "p50_ms": statistics.median(samples_ms),
        "p95_ms": sorted(samples_ms)[max(0, int(iterations * 0.95) - 1)],
        "max_ms": max(samples_ms),
    }


def resource_descriptor_measurement() -> dict[str, int | float]:
    resource = ResourceRef(
        ref_id="resource-1mib",
        resource_id=ResourceId("fixture.bytes", "slot-1", 1, 1),
        semantic=ResourceSemantic.FROZEN_VALUE,
        provider_id="fixture.provider",
        resource_kind="fixture.bytes",
        schema="fixture.bytes.v1",
        version=1,
        generation=1,
        access=ResourceAccess.blob("fixture.store", "blob-1mib"),
        size_hint=1024 * 1024,
        content_hash="sha256:fixture",
        lifetime=ResourceLifetime.PERSISTENT,
        lease=None,
        seal_state=ResourceSealState.SEALED,
    )
    descriptor = to_json_dict(resource)
    json_bytes = len(json.dumps(descriptor, separators=(",", ":")).encode())
    msgpack_bytes = len(msgpack.packb(descriptor, use_bin_type=True))
    return {
        "referenced_resource_bytes": 1024 * 1024,
        "json_descriptor_bytes": json_bytes,
        "msgpack_descriptor_bytes": msgpack_bytes,
        "json_copy_ratio": json_bytes / (1024 * 1024),
        "msgpack_copy_ratio": msgpack_bytes / (1024 * 1024),
    }


def main() -> None:
    cases: list[tuple[int, int, int]] = [
        (1, 1024, 100),
        (32, 1024, 30),
        (256, 1024, 8),
        (1, 64 * 1024, 30),
        (1, 1024 * 1024, 8),
    ]
    codec_results = [
        benchmark_codec(codec, batch, payload, iterations)
        for codec in ("typed_jsonl", "typed_msgpack")
        for batch, payload, iterations in cases
    ]
    report = {
        "schema_revision": SCHEMA_REVISION,
        "core_revision": CORE_WIRE_REVISION,
        "python": sys.version,
        "platform": sys.platform,
        "codec_results": codec_results,
        "cancel_latency": asyncio.run(benchmark_cancel(30)),
        "process_startup": benchmark_startup(20),
        "runner_reuse": asyncio.run(benchmark_runner_reuse(100)),
        "resource_descriptor": resource_descriptor_measurement(),
    }
    sys.stdout.write(json.dumps(report, indent=2, sort_keys=True))
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
