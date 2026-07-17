#!/usr/bin/env python3
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import platform
import statistics
import subprocess
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]


def digest(value: Any) -> str:
    return hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def distribution(samples: list[float], unit: str) -> dict[str, Any]:
    values = sorted(samples)
    median = statistics.median(values)
    deviations = sorted(abs(value - median) for value in values)

    def percentile(quantile: float) -> float:
        index = max(0, min(len(values) - 1, int(len(values) * quantile + 0.999999) - 1))
        return values[index]

    return {
        "median": median,
        "p95": percentile(0.95),
        "p99": percentile(0.99),
        "mad": statistics.median(deviations),
        "min": values[0],
        "max": values[-1],
        "unit": unit,
        "sample_count": len(values),
        "samples": values,
    }


def revisions(values: list[str]) -> dict[str, Any]:
    paths = {"MutsukiStdPlugins": ROOT}
    for value in values:
        name, separator, path = value.partition("=")
        if not separator:
            raise SystemExit("--repository must use NAME=PATH")
        paths[name] = Path(path).resolve()
    result = {}
    for name, path in sorted(paths.items()):
        revision = subprocess.check_output(
            ["git", "-C", str(path), "rev-parse", "HEAD"], text=True
        ).strip()
        dirty = bool(
            subprocess.check_output(
                ["git", "-C", str(path), "status", "--porcelain"], text=True
            )
        )
        remote = subprocess.run(
            ["git", "-C", str(path), "config", "--get", "remote.origin.url"],
            capture_output=True,
            text=True,
            check=False,
        ).stdout.strip() or "local-only"
        result[name] = {"revision": revision, "dirty": dirty, "remote": remote}
    return result


def environment(mode: str, process_runs: int) -> dict[str, Any]:
    if sys.platform == "darwin":
        cpu = subprocess.check_output(
            ["sysctl", "-n", "machdep.cpu.brand_string"], text=True
        ).strip()
        ram = int(subprocess.check_output(["sysctl", "-n", "hw.memsize"], text=True))
    elif os.name == "nt":
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
        cpu = platform.processor() or platform.machine() or "unknown"
        ram = int(status.total_physical)
    else:
        cpu = platform.processor() or platform.machine() or "unknown"
        ram = os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES")
    rustc = subprocess.check_output(["rustc", "--version"], text=True).strip()
    return {
        "cpu_model": cpu,
        "cpu_topology": f"logical={os.cpu_count() or 1}",
        "ram_bytes": ram,
        "os": platform.platform(),
        "kernel": platform.release(),
        "architecture": platform.machine(),
        "target_triple": f"{platform.machine()}-{sys.platform}",
        "toolchains": {"rustc": rustc, "python": platform.python_version()},
        "release_profile": {"name": "release", "lto": False, "codegen_units": 16},
        "power_mode": os.environ.get("MUTSUKI_BENCH_POWER_MODE", "not-recorded"),
        "virtualization": os.environ.get("MUTSUKI_BENCH_VIRTUALIZATION", "not-recorded"),
        "runner_configuration": {
            "mode": mode,
            "process_runs": process_runs,
            "fixed_seed": 1_297_435_713,
            "network": "loopback-only",
        },
        "network": {"scope": "loopback deterministic server", "public_requests": False},
    }


def analyze(cases: list[dict[str, Any]], counters: dict[str, int]) -> dict[str, Any]:
    noisy = []
    for case in cases:
        latency = case["metrics"]["latency_ns"]
        if latency["median"] and latency["mad"] / latency["median"] > 0.10:
            noisy.append({"case_id": case["case_id"], "dimensions": case["dimensions"]})
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
            "Plugin cases measure handler/provider cost and do not replace Core or ServiceHost.",
            "HTTP uses a deterministic loopback server and makes no public-network claim.",
        ],
    }


def run_benchmark_process(binary: Path, environment_value: dict[str, str]) -> dict[str, float]:
    if os.name != "nt":
        process = subprocess.Popen([str(binary)], cwd=ROOT, env=environment_value)
        _, wait_status, usage = os.wait4(process.pid, 0)
        process.returncode = os.waitstatus_to_exitcode(wait_status)
        if process.returncode:
            raise subprocess.CalledProcessError(process.returncode, process.args)
        return {
            "cpu_ns": (usage.ru_utime + usage.ru_stime) * 1_000_000_000,
            "peak_rss_bytes": usage.ru_maxrss * (1 if sys.platform == "darwin" else 1024),
        }

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

    process = subprocess.Popen([str(binary)], cwd=ROOT, env=environment_value)
    process.wait()
    handle = wintypes.HANDLE(int(process._handle))
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
    if not ctypes.windll.psapi.GetProcessMemoryInfo(
        handle, ctypes.byref(memory), memory.cb
    ):
        raise ctypes.WinError()
    if process.returncode:
        raise subprocess.CalledProcessError(process.returncode, process.args)

    def filetime_100ns(value: wintypes.FILETIME) -> int:
        return (value.dwHighDateTime << 32) | value.dwLowDateTime

    return {
        "cpu_ns": float((filetime_100ns(kernel) + filetime_100ns(user)) * 100),
        "peak_rss_bytes": float(memory.peak_working_set_size),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", choices=("smoke", "reference"), default="smoke")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repository", action="append", default=[], metavar="NAME=PATH")
    parser.add_argument("--process-runs", type=int)
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()
    process_runs = args.process_runs or (1 if args.mode == "smoke" else 3)
    output = args.output.resolve()
    raw_dir = output.with_suffix("").with_name(output.stem + "-raw")
    raw_dir.mkdir(parents=True, exist_ok=True)
    if not args.skip_build:
        subprocess.run(
            ["cargo", "build", "--release", "-p", "mutsuki-std-benchmarks"],
            cwd=ROOT,
            check=True,
        )
    executable = "mutsuki-std-benchmarks.exe" if os.name == "nt" else "mutsuki-std-benchmarks"
    binary = ROOT / "target/release" / executable
    reports = []
    process_metrics = []
    for process_run in range(process_runs):
        raw = raw_dir / f"std-{process_run}.json"
        process_metrics.append(
            run_benchmark_process(
                binary,
                {
                **os.environ,
                "MUTSUKI_BENCH_MODE": args.mode,
                "MUTSUKI_BENCH_OUTPUT": str(raw),
                },
            )
        )
        reports.append(json.loads(raw.read_text()))
    grouped: dict[tuple[str, str], list[dict[str, Any]]] = defaultdict(list)
    counters: dict[str, int] = defaultdict(int)
    for report in reports:
        for name, value in report["correctness"].items():
            counters[name] += int(value)
        for case in report["cases"]:
            key = (case["case_id"], json.dumps(case["dimensions"], sort_keys=True))
            grouped[key].append(case)
    cases = []
    for (case_id, _), items in sorted(grouped.items()):
        first = items[0]
        hashes = {item["output_hash"] for item in items}
        if len(hashes) != 1:
            counters["cross_process_hash_mismatches"] += 1
        samples = [float(value) for item in items for value in item["samples_ns"]]
        units = float(first["units"])
        cases.append(
            {
                "case_id": case_id,
                "measurement_mode": "time",
                "dimensions": {
                    **first["dimensions"],
                    "boundary": "plugin-handler-or-provider-only",
                },
                "metrics": {
                    "latency_ns": distribution(samples, "ns"),
                    "throughput_per_second": distribution(
                        [units * 1_000_000_000 / max(1, value) for value in samples],
                        "units/s",
                    ),
                    "disk_bytes": float(first["bytes"])
                    if case_id.startswith(("std.io.fs", "std.db.sqlite"))
                    else 0.0,
                    "network_bytes": float(first["bytes"])
                    if case_id == "std.io.http-client"
                    else 0.0,
                    "allocations": statistics.median(
                        float(item["allocations"]) / len(item["samples_ns"])
                        for item in items
                    ),
                    "allocated_bytes": statistics.median(
                        float(item["allocated_bytes"]) / len(item["samples_ns"])
                        for item in items
                    ),
                },
                "correctness": {
                    "passed": len(hashes) == 1 and not any(counters.values()),
                    "output_hash": next(iter(hashes)),
                    "counters": dict(counters),
                },
                "stage_breakdown": {"plugin_handler_fraction": 1.0, "framework_fraction": 0.0},
            }
        )
    cases.append(
        {
            "case_id": "std.system.process",
            "measurement_mode": "system",
            "dimensions": {"boundary": "whole benchmark process"},
            "metrics": {
                "latency_ns": distribution(
                    [float(metric["cpu_ns"]) for metric in process_metrics], "ns"
                ),
                "cpu_time_ns": distribution(
                    [float(metric["cpu_ns"]) for metric in process_metrics], "ns"
                ),
                "peak_rss_bytes": max(metric["peak_rss_bytes"] for metric in process_metrics),
            },
            "correctness": {"passed": not any(counters.values()), "counters": dict(counters)},
        }
    )
    repository_revisions = revisions(args.repository)
    environment_value = environment(args.mode, process_runs)
    generated_at = dt.datetime.now(dt.UTC).isoformat().replace("+00:00", "Z")
    report = {
        "schema_version": "mutsuki.performance.report/v1",
        "suite_version": "mutsuki-std-plugins-issue5-v1",
        "workload_version": "mutsuki.performance.std-workloads/v1",
        "report_id": f"std-plugins-{args.mode}-{generated_at}",
        "generated_at": generated_at,
        "revision_lock_hash": digest(repository_revisions),
        "repository_revisions": repository_revisions,
        "environment_id": digest(environment_value),
        "environment": environment_value,
        "feature_set": ["workflow", "resource", "sqlite", "fs", "http", "observe"],
        "deployment": "real standard plugin handlers with deterministic local boundaries",
        "measurement_boundary": (
            "plugin handler/provider only; framework cost excluded and public network forbidden"
        ),
        "sampling": {
            "warmup_iterations": 0,
            "samples_per_process": 3 if args.mode == "smoke" else 30,
            "process_runs": process_runs,
        },
        "cases": cases,
        "correctness": {
            "passed": not any(counters.values()),
            "counters": dict(sorted(counters.items())),
        },
        "metadata": {
            "fixture_manifest": "benchmarks/workloads-v1.json",
            "contract_validation": "MutsukiCore performance model v1",
        },
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n")
    analysis_path = output.with_name(output.stem + "-analysis.json")
    analysis_path.write_text(json.dumps(analyze(cases, dict(counters)), indent=2) + "\n")
    sys.stdout.write(f"{output}\n{analysis_path}\n")
    if any(counters.values()):
        raise SystemExit("StdPlugins performance correctness gate failed")


if __name__ == "__main__":
    main()
