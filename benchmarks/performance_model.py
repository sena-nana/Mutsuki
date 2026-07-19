#!/usr/bin/env python3
from __future__ import annotations

import argparse
import asyncio
import concurrent.futures
import datetime as dt
import gc
import hashlib
import json
import os
import platform
import statistics
import subprocess
import sys
import time
import tracemalloc
from collections import defaultdict
from dataclasses import replace
from pathlib import Path
from typing import Any, cast

from pipe_client import FixtureProcess

from mutsuki_runner_kit.contracts.codec import to_json_dict
from mutsuki_runner_kit.contracts.task import Task
from mutsuki_runner_kit.testing.batches import multi_entry_batch, runner_context
from mutsuki_runner_kit.testing.benchmark_runners import calibrated_checksum
from mutsuki_runner_kit.wire.binary import decode_binary_request, encode_binary_request
from mutsuki_runner_kit.wire.generated import Opcode
from mutsuki_runner_kit.wire.jsonl import decode_jsonl_request, encode_jsonl_request
from mutsuki_runner_kit.wire.protocol import DEFAULT_WIRE_LIMITS, WireProtocolFailure

ROOT = Path(__file__).resolve().parents[1]


def canonical_hash(value: Any) -> str:
    return hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def distribution(values: list[float], unit: str) -> dict[str, Any]:
    if not values or any(value < 0 or value == float("inf") for value in values):
        raise ValueError("samples must be finite and non-negative")
    samples = sorted(values)
    median = statistics.median(samples)
    deviations = sorted(abs(value - median) for value in samples)

    def percentile(quantile: float) -> float:
        index = max(0, min(len(samples) - 1, int(len(samples) * quantile + 0.999999) - 1))
        return samples[index]

    return {
        "median": median,
        "p95": percentile(0.95),
        "p99": percentile(0.99),
        "mad": statistics.median(deviations),
        "min": samples[0],
        "max": samples[-1],
        "unit": unit,
        "sample_count": len(samples),
        "samples": samples,
    }


def measured_case(
    case_id: str,
    layer: str,
    dimensions: dict[str, Any],
    samples: list[float],
    *,
    units: float = 1.0,
    counters: dict[str, int] | None = None,
    metrics: dict[str, Any] | None = None,
) -> dict[str, Any]:
    counters = counters or {}
    values: dict[str, Any] = {
        "latency_ns": distribution(samples, "ns"),
        "throughput_per_second": distribution(
            [units * 1_000_000_000 / max(1, value) for value in samples], "units/s"
        ),
    }
    if metrics:
        values.update(metrics)
    return {
        "case_id": case_id,
        "measurement_mode": "system" if layer == "real-stdio-pipe" else "time",
        "dimensions": {"layer": layer, **dimensions},
        "metrics": values,
        "correctness": {
            "passed": all(value == 0 for value in counters.values()),
            "counters": counters,
        },
    }


def diagnostic_case(
    case_id: str, dimensions: dict[str, Any], counters: dict[str, int], metrics: dict[str, float]
) -> dict[str, Any]:
    return {
        "case_id": case_id,
        "measurement_mode": "diagnostic",
        "dimensions": dimensions,
        "metrics": metrics,
        "correctness": {
            "passed": all(value == 0 for value in counters.values()),
            "counters": counters,
        },
    }


def wire_payload(batch_size: int, payload_bytes: int) -> dict[str, object]:
    tasks = tuple(
        replace(
            Task.new(f"task-{index}", "runner.echo", {"data": "x" * payload_bytes}),
            lease_id=f"lease-{index}",
        )
        for index in range(batch_size)
    )
    lease_ids = tuple(f"lease-{index}" for index in range(batch_size))
    batch = multi_entry_batch(
        tasks,
        lease_ids=lease_ids,
        batch_id=f"codec-{batch_size}-{payload_bytes}",
        runner_id="mutsuki.test.abi-fixture.echo",
    )
    ctx = runner_context(
        lease_ids=lease_ids,
        invocation_id=f"codec-{batch_size}-{payload_bytes}",
        batch_id=batch.batch_id,
    )
    return {
        "runner_id": "mutsuki.test.abi-fixture.echo",
        "ctx": to_json_dict(ctx),
        "batch": to_json_dict(batch),
    }


def codec_cases(mode: str, correctness: dict[str, int]) -> list[dict[str, Any]]:
    matrix = [(batch, payload) for batch in (1, 32, 256) for payload in (256, 4096, 65536)]
    matrix.append((1, 1024 * 1024))
    iterations = 3 if mode == "smoke" else 20
    cases: list[dict[str, Any]] = []
    for codec in ("python-jsonl", "python-binary"):
        encoder = encode_jsonl_request if codec == "python-jsonl" else encode_binary_request
        decoder = decode_jsonl_request if codec == "python-jsonl" else decode_binary_request
        for batch_size, payload_bytes in matrix:
            dimensions = {
                "codec": codec,
                "batch": batch_size,
                "payload_bytes": payload_bytes,
                "gc_enabled_during_measurement": False,
            }
            payload = wire_payload(batch_size, payload_bytes)
            try:
                encoded = encoder(1, Opcode.RUNNER_RUN_BATCH, payload)
                decoder(encoded)
            except WireProtocolFailure:
                cases.append(
                    diagnostic_case(
                        "python.codec.policy-rejection",
                        {**dimensions, "layer": "codec-only"},
                        {"unexpected_accepts": 0},
                        {"configured_max_frame_bytes": float(DEFAULT_WIRE_LIMITS.max_frame_bytes)},
                    )
                )
                continue
            gc.collect()
            gc_was_enabled = gc.isenabled()
            gc.disable()
            tracemalloc.start()
            try:
                encode_samples = []
                for request_id in range(1, iterations + 1):
                    started = time.perf_counter_ns()
                    encoded = encoder(request_id, Opcode.RUNNER_RUN_BATCH, payload)
                    encode_samples.append(float(time.perf_counter_ns() - started))
                _, encode_peak = tracemalloc.get_traced_memory()
                tracemalloc.reset_peak()
                decode_samples = []
                frame_samples = []
                for _ in range(iterations):
                    started = time.perf_counter_ns()
                    decoder(encoded)
                    decode_samples.append(float(time.perf_counter_ns() - started))
                    started = time.perf_counter_ns()
                    decoder(encoder(1, Opcode.RUNNER_RUN_BATCH, payload))
                    frame_samples.append(float(time.perf_counter_ns() - started))
                _, decode_peak = tracemalloc.get_traced_memory()
            finally:
                tracemalloc.stop()
                if gc_was_enabled:
                    gc.enable()
            for operation, samples, peak in (
                ("encode", encode_samples, encode_peak),
                ("decode", decode_samples, decode_peak),
                ("frame", frame_samples, max(encode_peak, decode_peak)),
            ):
                cases.append(
                    measured_case(
                        f"python.codec.{operation}",
                        "codec-only",
                        dimensions,
                        samples,
                        units=float(len(encoded)),
                        counters={"codec_failures": 0},
                        metrics={
                            "frame_bytes": float(len(encoded)),
                            "tracemalloc_peak_bytes": float(peak),
                        },
                    )
                )
    for codec in ("python-jsonl", "python-binary"):
        decoder = decode_jsonl_request if codec == "python-jsonl" else decode_binary_request
        malformed = b"{broken}\n" if codec == "python-jsonl" else b"\x00\x00\x00\x01x"
        try:
            decoder(malformed)
        except Exception:
            pass
        else:
            correctness["malformed_frames_accepted"] += 1
        oversized = b"x" * (DEFAULT_WIRE_LIMITS.max_frame_bytes + 1)
        try:
            decoder(oversized)
        except Exception:
            pass
        else:
            correctness["oversized_frames_accepted"] += 1
    cases.append(
        diagnostic_case(
            "python.codec.rejection",
            {"layer": "codec-only", "codecs": "python-jsonl,python-binary"},
            {
                "malformed_frames_accepted": correctness["malformed_frames_accepted"],
                "oversized_frames_accepted": correctness["oversized_frames_accepted"],
            },
            {"max_frame_bytes": float(DEFAULT_WIRE_LIMITS.max_frame_bytes)},
        )
    )
    return cases


def process_usage(pid: int) -> tuple[int, int]:
    if os.name == "nt":
        return windows_process_usage(pid)
    result = subprocess.run(
        ["ps", "-o", "time=,rss=", "-p", str(pid)], capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        return 0, 0
    fields = result.stdout.split()
    if len(fields) < 2:
        return 0, 0
    minutes, seconds = fields[0].rsplit(":", 1)
    cpu_ns = int((int(minutes) * 60 + float(seconds)) * 1_000_000_000)
    return cpu_ns, int(fields[1]) * 1024


def windows_process_usage(pid: int) -> tuple[int, int]:
    import ctypes
    from ctypes import wintypes

    class ProcessMemoryCounters(ctypes.Structure):
        _fields_ = [
            ("cb", wintypes.DWORD),
            ("page_fault_count", wintypes.DWORD),
            ("peak_working_set_size", ctypes.c_size_t),
            ("working_set_size", ctypes.c_size_t),
            ("quota_peak_paged_pool_usage", ctypes.c_size_t),
            ("quota_paged_pool_usage", ctypes.c_size_t),
            ("quota_peak_non_paged_pool_usage", ctypes.c_size_t),
            ("quota_non_paged_pool_usage", ctypes.c_size_t),
            ("pagefile_usage", ctypes.c_size_t),
            ("peak_pagefile_usage", ctypes.c_size_t),
        ]

    process_query_information = 0x0400
    process_vm_read = 0x0010
    handle = ctypes.windll.kernel32.OpenProcess(
        process_query_information | process_vm_read, False, pid
    )
    if not handle:
        raise ctypes.WinError()
    try:
        creation = wintypes.FILETIME()
        exit_time = wintypes.FILETIME()
        kernel = wintypes.FILETIME()
        user = wintypes.FILETIME()
        if not ctypes.windll.kernel32.GetProcessTimes(
            handle,
            ctypes.byref(creation),
            ctypes.byref(exit_time),
            ctypes.byref(kernel),
            ctypes.byref(user),
        ):
            raise ctypes.WinError()
        memory = ProcessMemoryCounters()
        memory.cb = ctypes.sizeof(memory)
        if not ctypes.windll.psapi.GetProcessMemoryInfo(handle, ctypes.byref(memory), memory.cb):
            raise ctypes.WinError()
    finally:
        ctypes.windll.kernel32.CloseHandle(handle)

    def ticks(value: wintypes.FILETIME) -> int:
        return (value.dwHighDateTime << 32) | value.dwLowDateTime

    return (ticks(kernel) + ticks(user)) * 100, int(memory.peak_working_set_size)


def validate_response(response: tuple[int, bool, object], correctness: dict[str, int]) -> None:
    if not response[1]:
        correctness["pipe_response_errors"] += 1


def calibrate_cpu_iterations(target_ns: int) -> int:
    iterations = 64
    for _ in range(8):
        started = time.perf_counter_ns()
        calibrated_checksum(1_297_435_713, iterations)
        elapsed = max(1, time.perf_counter_ns() - started)
        scaled = max(1, int(iterations * target_ns / elapsed))
        if abs(elapsed - target_ns) / target_ns < 0.10:
            break
        iterations = scaled
    return iterations


def pipe_fixture_hashes(codec: str, correctness: dict[str, int]) -> dict[str, str]:
    fixture_manifest = json.loads((ROOT / "benchmarks/runner-fixtures-v1.json").read_text())
    expected = {
        fixture["protocol_id"]: fixture["output_sha256"] for fixture in fixture_manifest["fixtures"]
    }
    inputs = {
        fixture["protocol_id"]: fixture["payload"] for fixture in fixture_manifest["fixtures"]
    }
    hashes: dict[str, str] = {}
    with FixtureProcess(ROOT, codec) as process:
        process.initialize()
        for sequence, (protocol_id, payload) in enumerate(inputs.items(), start=1):
            request_id = process.dispatch(protocol_id, payload, sequence)
            response_id, ok, completion = process.receive()
            if response_id != request_id or not ok or not isinstance(completion, dict):
                correctness["fixture_response_failures"] += 1
                continue
            results = completion.get("results")
            if not isinstance(results, list) or len(results) != 1:
                correctness["fixture_response_failures"] += 1
                continue
            entry = results[0]
            if not isinstance(entry, dict):
                correctness["fixture_response_failures"] += 1
                continue
            if protocol_id == "runner.fault":
                error = entry.get("error")
                code = error.get("code") if isinstance(error, dict) else None
                output: Any = {"error": {"code": code, "retryable": False}}
            else:
                result = entry.get("result")
                output = result.get("output") if isinstance(result, dict) else None
            output_hash = canonical_hash(output)
            hashes[f"{codec}:{protocol_id}"] = output_hash
            if output_hash != expected[protocol_id]:
                correctness["fixture_hash_mismatches"] += 1
        process.close()
    return hashes


def pipe_cases(
    mode: str, correctness: dict[str, int]
) -> tuple[list[dict[str, Any]], dict[str, str]]:
    cases: list[dict[str, Any]] = []
    fixture_hashes: dict[str, str] = {}
    process_runs = 2 if mode == "smoke" else 10
    warm_samples = 5 if mode == "smoke" else 50
    idle_seconds = 0.2 if mode == "smoke" else 5.0
    cancel_samples = 3 if mode == "smoke" else 30
    cpu_samples = 2 if mode == "smoke" else 10
    cpu_iterations = {
        target_ns: calibrate_cpu_iterations(target_ns)
        for target_ns in (50_000, 1_000_000, 10_000_000)
    }
    for codec in ("python-jsonl", "python-binary"):
        fixture_hashes.update(pipe_fixture_hashes(codec, correctness))
        startup = []
        negotiation = []
        shutdown = []
        rss_samples = []
        for process_run in range(process_runs):
            started = time.perf_counter_ns()
            with FixtureProcess(ROOT, codec) as process:
                negotiation.append(float(process.initialize()))
                startup.append(float(time.perf_counter_ns() - started))
                rss_samples.append(float(process_usage(process.process.pid)[1]))
                return_code, elapsed = process.close()
                shutdown.append(float(elapsed))
                if return_code != 0:
                    correctness["unclean_shutdowns"] += 1
        cases.extend(
            [
                measured_case(
                    "python.process.cold-start", "real-stdio-pipe", {"codec": codec}, startup
                ),
                measured_case(
                    "python.process.hello-ack", "real-stdio-pipe", {"codec": codec}, negotiation
                ),
                measured_case(
                    "python.process.shutdown", "real-stdio-pipe", {"codec": codec}, shutdown
                ),
                measured_case(
                    "python.process.restart-rss",
                    "real-stdio-pipe",
                    {"codec": codec},
                    [1.0 for _ in rss_samples],
                    metrics={
                        "peak_rss_bytes": max(rss_samples),
                        "retained_rss_bytes": rss_samples[-1] - rss_samples[0],
                    },
                ),
            ]
        )

        with FixtureProcess(ROOT, codec) as process:
            process.initialize()
            reuse = []
            for sequence in range(1, warm_samples + 1):
                started = time.perf_counter_ns()
                request_id = process.dispatch("runner.echo", {"message": "mutsuki"}, sequence)
                response = process.receive()
                reuse.append(float(time.perf_counter_ns() - started))
                if response[0] != request_id:
                    correctness["response_id_mismatches"] += 1
                validate_response(response, correctness)
            cases.append(
                measured_case(
                    "python.process.warm-dispatch",
                    "real-stdio-pipe",
                    {"codec": codec, "inflight": 1},
                    reuse,
                    counters={"response_errors": correctness["pipe_response_errors"]},
                )
            )
            sequence = warm_samples + 1
            for inflight in (1, 16, 56):
                started = time.perf_counter_ns()
                with concurrent.futures.ThreadPoolExecutor(max_workers=1) as receiver:
                    responses_future = receiver.submit(
                        lambda: [process.receive() for _ in range(inflight)]
                    )
                    expected = {
                        process.dispatch(
                            "runner.echo", {"message": "mutsuki"}, sequence + index
                        )
                        for index in range(inflight)
                    }
                    try:
                        responses = responses_future.result(timeout=30)
                    except TimeoutError as error:
                        process.kill()
                        raise RuntimeError(
                            f"timed out receiving {inflight} concurrent {codec} responses"
                        ) from error
                received = {response[0] for response in responses}
                for response in responses:
                    validate_response(response, correctness)
                elapsed = float(time.perf_counter_ns() - started)
                if received != expected:
                    correctness["response_id_mismatches"] += 1
                cases.append(
                    measured_case(
                        "python.process.concurrent-dispatch",
                        "real-stdio-pipe",
                        {"codec": codec, "inflight": inflight},
                        [elapsed],
                        units=float(inflight),
                        counters={"response_id_mismatches": correctness["response_id_mismatches"]},
                    )
                )
                sequence += inflight

            cancel_latencies = []
            for cancel_index in range(cancel_samples):
                blocked_sequence = sequence + cancel_index + 1
                work_id = process.dispatch("runner.wait", {"blocked": True}, blocked_sequence)
                time.sleep(0.001)
                started = time.perf_counter_ns()
                cancel_id = process.cancel("runner.wait", f"invocation-{blocked_sequence}")
                seen: set[int] = set()
                cancel_elapsed = 0.0
                for _ in range(2):
                    response = process.receive()
                    seen.add(response[0])
                    validate_response(response, correctness)
                    if response[0] == cancel_id:
                        cancel_elapsed = float(time.perf_counter_ns() - started)
                if seen != {work_id, cancel_id}:
                    correctness["cancel_response_mismatches"] += 1
                cancel_latencies.append(cancel_elapsed)
            cases.append(
                measured_case(
                    "python.process.cancel",
                    "real-stdio-pipe",
                    {"codec": codec, "management_reserved": True},
                    cancel_latencies,
                    counters={
                        "cancel_response_mismatches": correctness["cancel_response_mismatches"]
                    },
                )
            )
            sequence += cancel_samples

            for target_ns, iterations in cpu_iterations.items():
                samples = []
                for cpu_index in range(cpu_samples):
                    sequence += 1
                    started = time.perf_counter_ns()
                    request_id = process.dispatch(
                        "runner.calibrated-cpu",
                        {"seed": 1_297_435_713, "iterations": iterations},
                        sequence,
                    )
                    response = process.receive()
                    samples.append(float(time.perf_counter_ns() - started))
                    if response[0] != request_id or not response[1]:
                        correctness["cpu_fixture_failures"] += 1
                cases.append(
                    measured_case(
                        "python.process.calibrated-cpu",
                        "real-stdio-pipe",
                        {
                            "codec": codec,
                            "target_execution_ns": target_ns,
                            "iterations": iterations,
                        },
                        samples,
                        counters={"cpu_fixture_failures": correctness["cpu_fixture_failures"]},
                        metrics={"simulated_execution_ns": float(target_ns)},
                    )
                )

            sequence += 1
            started = time.perf_counter_ns()
            resource_id = process.dispatch(
                "runner.resource",
                {"resource_ref": "fixture-resource", "version": 1},
                sequence,
            )
            resource_response = process.receive()
            resource_elapsed = float(time.perf_counter_ns() - started)
            if resource_response[0] != resource_id or not resource_response[1]:
                correctness["resource_fixture_failures"] += 1
            cases.append(
                measured_case(
                    "python.process.resource-ref",
                    "real-stdio-pipe",
                    {"codec": codec, "resource_bytes": 1024 * 1024},
                    [resource_elapsed],
                    counters={
                        "resource_fixture_failures": correctness["resource_fixture_failures"]
                    },
                    metrics={"referenced_resource_bytes": float(1024 * 1024)},
                )
            )

            cpu_before, rss_before = process_usage(process.process.pid)
            time.sleep(idle_seconds)
            cpu_after, rss_after = process_usage(process.process.pid)
            cases.append(
                measured_case(
                    "python.process.idle",
                    "real-stdio-pipe",
                    {"codec": codec, "idle_seconds": idle_seconds},
                    [idle_seconds * 1_000_000_000],
                    metrics={
                        "cpu_time_ns": distribution([float(max(0, cpu_after - cpu_before))], "ns"),
                        "peak_rss_bytes": float(max(rss_before, rss_after)),
                        "retained_rss_bytes": float(rss_after - rss_before),
                    },
                )
            )
            pressure_id = process.dispatch(
                "runner.fault",
                {"fault": "stdout_stderr_pressure", "bytes": 64 * 1024},
                sequence + 2,
            )
            pressure_response = process.receive()
            if pressure_response[0] != pressure_id or not pressure_response[1]:
                correctness["pressure_failures"] += 1
            cases.append(
                diagnostic_case(
                    "python.process.stdio-pressure",
                    {"layer": "real-stdio-pipe", "codec": codec},
                    {"pressure_failures": correctness["pressure_failures"]},
                    {"diagnostic_bytes": float(process.diagnostic_bytes())},
                )
            )
            started = time.perf_counter_ns()
            dispose_id = process.send(
                Opcode.RUNNER_DISPOSE,
                {"runner_id": "mutsuki.test.abi-fixture.echo"},
            )
            dispose_response = process.receive()
            dispose_elapsed = float(time.perf_counter_ns() - started)
            if dispose_response[0] != dispose_id or not dispose_response[1]:
                correctness["dispose_failures"] += 1
            cases.append(
                measured_case(
                    "python.process.dispose",
                    "real-stdio-pipe",
                    {"codec": codec},
                    [dispose_elapsed],
                    counters={"dispose_failures": correctness["dispose_failures"]},
                )
            )
            process.close()

        with FixtureProcess(ROOT, codec) as process:
            process.initialize()
            process.dispatch("runner.fault", {"fault": "process_exit"}, 1)
            try:
                process.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                correctness["crash_fixture_failures"] += 1
                process.kill()
            if process.process.returncode == 0:
                correctness["crash_fixture_failures"] += 1
            cases.append(
                diagnostic_case(
                    "python.process.crash",
                    {"layer": "real-stdio-pipe", "codec": codec},
                    {"crash_fixture_failures": correctness["crash_fixture_failures"]},
                    {"exit_code": float(process.process.returncode or 0)},
                )
            )
    return cases, fixture_hashes


async def in_memory_cases(mode: str) -> list[dict[str, Any]]:
    from mutsuki_runner_kit.runners.backend import PythonRunnerBackend
    from mutsuki_runner_kit.testing.benchmark_runners import standard_fixture_runners

    backend = PythonRunnerBackend()
    for runner in standard_fixture_runners():
        backend.register_runner(runner)
    samples = []
    count = 10 if mode == "smoke" else 100
    for index in range(count):
        lease_id = f"memory-{index}"
        task = replace(
            Task.new(f"memory-{index}", "runner.echo", {"value": index}), lease_id=lease_id
        )
        batch = multi_entry_batch(
            (task,),
            lease_ids=(lease_id,),
            batch_id=f"memory-{index}",
            runner_id="mutsuki.test.abi-fixture.echo",
        )
        ctx = runner_context(
            lease_ids=(lease_id,), invocation_id=f"memory-{index}", batch_id=batch.batch_id
        )
        started = time.perf_counter_ns()
        completion = await backend.run_batch_runner("mutsuki.test.abi-fixture.echo", ctx, batch)
        samples.append(float(time.perf_counter_ns() - started))
        if completion.results[0].error is not None:
            raise RuntimeError("in-memory fixture failed")
    return [
        measured_case(
            "python.runner.in-memory-dispatch",
            "in-memory-bridge",
            {"codec": "typed-contract"},
            samples,
        )
    ]


def repository_revisions(values: list[str]) -> dict[str, Any]:
    repositories = {"MutsukiPythonRunnerKit": ROOT}
    for value in values:
        name, separator, raw_path = value.partition("=")
        if not separator:
            raise SystemExit("--repository must use NAME=PATH")
        repositories[name] = Path(raw_path).resolve()
    result = {}
    for name, path in sorted(repositories.items()):
        revision = subprocess.check_output(
            ["git", "-C", str(path), "rev-parse", "HEAD"], text=True
        ).strip()
        dirty = bool(
            subprocess.check_output(["git", "-C", str(path), "status", "--porcelain"], text=True)
        )
        remote = (
            subprocess.run(
                ["git", "-C", str(path), "config", "--get", "remote.origin.url"],
                capture_output=True,
                text=True,
                check=False,
            ).stdout.strip()
            or "local-only"
        )
        result[name] = {"revision": revision, "dirty": dirty, "remote": remote}
    return result


def environment(mode: str) -> dict[str, Any]:
    cpu = platform.processor() or platform.machine() or "unknown"
    if sys.platform == "darwin":
        cpu = subprocess.check_output(
            ["sysctl", "-n", "machdep.cpu.brand_string"], text=True
        ).strip()
        ram = int(subprocess.check_output(["sysctl", "-n", "hw.memsize"], text=True))
    elif os.name == "nt":
        ram = windows_memory_bytes()
    else:
        ram = os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES")
    import msgpack

    return {
        "cpu_model": cpu,
        "cpu_topology": f"logical={os.cpu_count() or 1}",
        "ram_bytes": ram,
        "os": platform.platform(),
        "kernel": platform.release(),
        "architecture": platform.machine(),
        "target_triple": f"{platform.machine()}-{sys.platform}",
        "toolchains": {
            "python": platform.python_version(),
            "msgpack": cast(str, msgpack.__version__),
        },
        "release_profile": {"name": "python-optimized", "lto": False, "codegen_units": 1},
        "power_mode": os.environ.get("MUTSUKI_BENCH_POWER_MODE", "not-recorded"),
        "virtualization": os.environ.get("MUTSUKI_BENCH_VIRTUALIZATION", "not-recorded"),
        "runner_configuration": {
            "mode": mode,
            "gc": "disabled only during codec sampling",
            "entrypoints": ["python-jsonl", "python-binary"],
        },
    }


def windows_memory_bytes() -> int:
    import ctypes
    from ctypes import wintypes

    class MemoryStatus(ctypes.Structure):
        _fields_ = [
            ("length", wintypes.DWORD),
            ("memory_load", wintypes.DWORD),
            ("total_physical", ctypes.c_ulonglong),
            ("available_physical", ctypes.c_ulonglong),
            ("total_page_file", ctypes.c_ulonglong),
            ("available_page_file", ctypes.c_ulonglong),
            ("total_virtual", ctypes.c_ulonglong),
            ("available_virtual", ctypes.c_ulonglong),
            ("available_extended_virtual", ctypes.c_ulonglong),
        ]

    status = MemoryStatus()
    status.length = ctypes.sizeof(status)
    if not ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status)):
        raise ctypes.WinError()
    return int(status.total_physical)


def analyze(cases: list[dict[str, Any]], counters: dict[str, int]) -> dict[str, Any]:
    noisy = []
    for item in cases:
        latency = item["metrics"].get("latency_ns")
        if latency and latency["median"] and latency["mad"] / latency["median"] > 0.10:
            noisy.append({"case_id": item["case_id"], "dimensions": item["dimensions"]})
    if any(counters.values()):
        classification = "framework-suspect"
    elif len(noisy) / max(1, len(cases)) > 0.20:
        classification = "environmental-noise"
    elif noisy:
        classification = "case-specific-noise"
    else:
        classification = "no-obvious-anomaly"
    return {
        "schema_version": "mutsuki.performance.analysis/v1",
        "classification": classification,
        "correctness_counters": counters,
        "noisy_cases": noisy,
        "limitations": [
            "ServiceHost end-to-end is intentionally not measured by this owner runner.",
            (
                "Use benchmarks/fixture_process.py as the python-jsonl/python-binary "
                "deployment entrypoint."
            ),
        ],
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", choices=("smoke", "reference"), default="smoke")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repository", action="append", default=[], metavar="NAME=PATH")
    args = parser.parse_args()
    correctness: dict[str, int] = defaultdict(int)
    cases = codec_cases(args.mode, correctness)
    cases.extend(asyncio.run(in_memory_cases(args.mode)))
    process_cases, fixture_hashes = pipe_cases(args.mode, correctness)
    cases.extend(process_cases)
    revisions = repository_revisions(args.repository)
    environment_value = environment(args.mode)
    generated_at = dt.datetime.now(dt.UTC).isoformat().replace("+00:00", "Z")
    report = {
        "schema_version": "mutsuki.performance.report/v1",
        "suite_version": "mutsuki-python-runner-kit-issue4-v1",
        "workload_version": "mutsuki.performance.runner-fixtures/v1",
        "report_id": f"python-runner-{args.mode}-{generated_at}",
        "generated_at": generated_at,
        "revision_lock_hash": canonical_hash(revisions),
        "repository_revisions": revisions,
        "environment_id": canonical_hash(environment_value),
        "environment": environment_value,
        "feature_set": ["python-jsonl", "python-binary", "real-stdio-pipe"],
        "deployment": "Python fixture process over real stdin/stdout pipe",
        "measurement_boundary": (
            "codec-only, in-memory-bridge and real-stdio-pipe are separate; "
            "ServiceHost E2E is external"
        ),
        "sampling": {
            "warmup_iterations": 1,
            "samples_per_process": min(
                item["metrics"]["latency_ns"]["sample_count"]
                for item in cases
                if "latency_ns" in item["metrics"]
            ),
            "process_runs": 2 if args.mode == "smoke" else 10,
        },
        "cases": cases,
        "correctness": {
            "passed": not any(correctness.values()),
            "counters": dict(sorted(correctness.items())),
        },
        "metadata": {
            "service_host_entrypoints": "python-jsonl,python-binary",
            "fixture_manifest": "benchmarks/runner-fixtures-v1.json",
            "fixture_output_hashes": fixture_hashes,
            "service_host_e2e": "not measured in this repository",
        },
    }
    args.output = args.output.resolve()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    analysis_path = args.output.with_name(args.output.stem + "-analysis.json")
    analysis_path.write_text(json.dumps(analyze(cases, dict(correctness)), indent=2) + "\n")
    sys.stdout.write(f"{args.output}\n{analysis_path}\n")
    if any(correctness.values()):
        raise SystemExit("Python performance correctness gate failed")


if __name__ == "__main__":
    main()
