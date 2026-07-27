from __future__ import annotations

import asyncio
import os
import sys
from collections.abc import Mapping

from mutsuki_runner_kit.contracts.batch import CompletionBatch, WorkBatch
from mutsuki_runner_kit.contracts.codec import JsonValue
from mutsuki_runner_kit.contracts.errors import RuntimeError
from mutsuki_runner_kit.contracts.runner import (
    ExecutionClass,
    RunnerContext,
    RunnerDescriptor,
    RunnerPurity,
    RunnerResult,
)
from mutsuki_runner_kit.contracts.task import Task
from mutsuki_runner_kit.runners.protocol import RunnerInvokeError
from mutsuki_runner_kit.runners.scalar import ScalarBatchAdapter

PLUGIN_ID = "mutsuki.test.abi-fixture"
DEFAULT_CPU_SEED = 1_297_435_713


def calibrated_checksum(seed: int, iterations: int = 4_096) -> str:
    value = seed & 0xFFFF_FFFF_FFFF_FFFF
    for _ in range(iterations):
        value = (
            value * 6_364_136_223_846_793_005 + 1_442_695_040_888_963_407
        ) & 0xFFFF_FFFF_FFFF_FFFF
        value ^= value >> 33
    return f"{value:016x}"


class _ScalarFixture:
    protocol_id: str
    execution_class = ExecutionClass.CPU

    def __init__(self) -> None:
        self.cancelled: list[str] = []
        self.disposed = False
        self._adapter = ScalarBatchAdapter(self)

    @property
    def descriptor(self) -> RunnerDescriptor:
        return RunnerDescriptor(
            runner_id=f"{PLUGIN_ID}.{self.protocol_id.removeprefix('runner.')}",
            plugin_id=PLUGIN_ID,
            plugin_generation=1,
            accepted_protocol_ids=(self.protocol_id,),
            purity=RunnerPurity.PURE,
            execution_class=self.execution_class,
            contract_surfaces=(f"runner:{self.protocol_id}",),
        )

    async def run_batch(self, ctx: RunnerContext, batch: WorkBatch) -> CompletionBatch:
        return await self._adapter.run_batch(ctx, batch)

    async def run_one(self, ctx: RunnerContext, task: Task) -> RunnerResult:
        raise NotImplementedError

    async def cancel(self, invocation_id: str) -> None:
        self.cancelled.append(invocation_id)

    async def dispose(self) -> None:
        self.disposed = True


class NoopRunner(_ScalarFixture):
    protocol_id = "runner.noop"

    async def run_one(self, ctx: RunnerContext, task: Task) -> RunnerResult:
        return RunnerResult(task_id=task.task_id, output={"status": "ok"})


class BenchmarkEchoRunner(_ScalarFixture):
    protocol_id = "runner.echo"

    async def run_one(self, ctx: RunnerContext, task: Task) -> RunnerResult:
        return RunnerResult(task_id=task.task_id, output={"echo": task.payload})


class CalibratedCpuRunner(_ScalarFixture):
    protocol_id = "runner.calibrated-cpu"

    async def run_one(self, ctx: RunnerContext, task: Task) -> RunnerResult:
        payload = task.payload if isinstance(task.payload, Mapping) else {}
        seed = payload.get("seed", DEFAULT_CPU_SEED)
        iterations = payload.get("iterations", 4_096)
        if not isinstance(seed, int) or isinstance(seed, bool):
            raise TypeError("calibrated CPU seed must be an integer")
        if not isinstance(iterations, int) or isinstance(iterations, bool) or iterations < 0:
            raise TypeError("calibrated CPU iterations must be a non-negative integer")
        return RunnerResult(
            task_id=task.task_id,
            output={"checksum": calibrated_checksum(seed, iterations)},
        )


class WaitRunner(_ScalarFixture):
    protocol_id = "runner.wait"
    execution_class = ExecutionClass.BLOCKING

    def __init__(self) -> None:
        super().__init__()
        self._released = asyncio.Event()

    async def run_one(self, ctx: RunnerContext, task: Task) -> RunnerResult:
        payload = task.payload if isinstance(task.payload, Mapping) else {}
        delay_us = payload.get("delay_us", 0)
        if not isinstance(delay_us, int) or isinstance(delay_us, bool) or delay_us < 0:
            raise TypeError("wait delay_us must be a non-negative integer")
        if payload.get("blocked") is True:
            self._released.clear()
            await self._released.wait()
        elif delay_us:
            await asyncio.sleep(delay_us / 1_000_000)
        return RunnerResult(task_id=task.task_id, output={"resumed": True})

    async def cancel(self, invocation_id: str) -> None:
        await super().cancel(invocation_id)
        self._released.set()


class ResourceRunner(_ScalarFixture):
    protocol_id = "runner.resource"
    execution_class = ExecutionClass.IO

    async def run_one(self, ctx: RunnerContext, task: Task) -> RunnerResult:
        payload = task.payload if isinstance(task.payload, Mapping) else {}
        return RunnerResult(
            task_id=task.task_id,
            output={
                "resource_id": payload.get("resource_ref"),
                "version": payload.get("version"),
            },
        )


class FaultRunner(_ScalarFixture):
    protocol_id = "runner.fault"
    execution_class = ExecutionClass.BLOCKING

    def __init__(self) -> None:
        super().__init__()
        self._released = asyncio.Event()

    async def run_one(self, ctx: RunnerContext, task: Task) -> RunnerResult:
        payload = task.payload if isinstance(task.payload, Mapping) else {}
        mode = payload.get("fault", "error")
        delay_ms = payload.get("delay_ms", 0)
        if isinstance(delay_ms, int) and not isinstance(delay_ms, bool) and delay_ms > 0:
            await asyncio.sleep(delay_ms / 1_000)
        if mode == "block":
            await self._released.wait()
            return RunnerResult(task_id=task.task_id, output={"cancelled": True})
        if mode == "process_exit":
            os._exit(70)
        if mode == "stdout_stderr_pressure":
            pressure_bytes = payload.get("bytes", 64 * 1024)
            if not isinstance(pressure_bytes, int) or pressure_bytes < 0:
                raise TypeError("pressure byte count must be non-negative")
            sys.stdout.write("x" * pressure_bytes)
            sys.stdout.flush()
            sys.stderr.write("x" * pressure_bytes)
            sys.stderr.flush()
            return RunnerResult(task_id=task.task_id, output={"pressure_bytes": pressure_bytes})
        if mode == "stale_completion":
            return RunnerResult(
                task_id=task.task_id,
                output={"attempt": payload.get("attempt"), "stale": True},
            )
        raise RunnerInvokeError(
            RuntimeError(
                code="fixture.failure",
                source=PLUGIN_ID,
                route="fixture.requested_failure",
            )
        )

    async def cancel(self, invocation_id: str) -> None:
        await super().cancel(invocation_id)
        self._released.set()


def standard_fixture_runners() -> tuple[_ScalarFixture, ...]:
    return (
        NoopRunner(),
        BenchmarkEchoRunner(),
        CalibratedCpuRunner(),
        WaitRunner(),
        ResourceRunner(),
        FaultRunner(),
    )


def fixture_output(protocol_id: str, payload: JsonValue) -> JsonValue:
    if protocol_id == "runner.noop":
        return {"status": "ok"}
    if protocol_id == "runner.echo":
        return {"echo": payload}
    if protocol_id == "runner.calibrated-cpu":
        data = payload if isinstance(payload, Mapping) else {}
        seed = data.get("seed", DEFAULT_CPU_SEED)
        iterations = data.get("iterations", 4_096)
        if not isinstance(seed, int) or not isinstance(iterations, int):
            raise TypeError("invalid calibrated CPU fixture")
        return {"checksum": calibrated_checksum(seed, iterations)}
    if protocol_id == "runner.wait":
        return {"resumed": True}
    if protocol_id == "runner.resource":
        data = payload if isinstance(payload, Mapping) else {}
        return {"resource_id": data.get("resource_ref"), "version": data.get("version")}
    if protocol_id == "runner.fault":
        return {"error": {"code": "fixture.failure", "retryable": False}}
    raise ValueError(f"unknown benchmark fixture protocol: {protocol_id}")
