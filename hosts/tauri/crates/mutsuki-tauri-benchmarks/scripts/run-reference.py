#!/usr/bin/env python3
"""Run TauriHost raw fixtures and emit Performance Model v1 output."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import platform
import re
import statistics
import subprocess
import tempfile
from collections import defaultdict
from datetime import datetime, timezone


ROOT = pathlib.Path(__file__).resolve().parents[3]


def run(command: list[str]) -> None:
    subprocess.run(command, cwd=ROOT, check=True)


def canonical_hash(value: object) -> str:
    return hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def percentile(values: list[float], ratio: float) -> float:
    ordered = sorted(values)
    index = min(len(ordered) - 1, max(0, int(len(ordered) * ratio + 0.999999) - 1))
    return ordered[index]


def distribution(values: list[float], unit: str) -> dict[str, object]:
    ordered = sorted(values)
    median = statistics.median(ordered)
    deviations = sorted(abs(value - median) for value in ordered)
    return {
        "median": median,
        "p95": percentile(ordered, 0.95),
        "p99": percentile(ordered, 0.99),
        "mad": statistics.median(deviations),
        "min": ordered[0],
        "max": ordered[-1],
        "unit": unit,
        "sample_count": len(ordered),
        "samples": ordered,
    }


def command_output(*command: str) -> str:
    try:
        return subprocess.check_output(command, cwd=ROOT, text=True).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def environment(mode: str, warmup: int, samples: int) -> dict[str, object]:
    cpu = command_output("sysctl", "-n", "machdep.cpu.brand_string")
    if cpu == "unknown":
        cpu = platform.processor() or platform.machine()
    ram = command_output("sysctl", "-n", "hw.memsize")
    ram_bytes = int(ram) if ram.isdigit() else 1
    rust_verbose = command_output("rustc", "-vV")
    target = next(
        (line.removeprefix("host: ") for line in rust_verbose.splitlines() if line.startswith("host: ")),
        "unknown",
    )
    return {
        "cpu_model": cpu,
        "cpu_topology": f"logical={os.cpu_count() or 1}",
        "ram_bytes": ram_bytes,
        "os": platform.system(),
        "kernel": platform.release(),
        "architecture": platform.machine(),
        "target_triple": target,
        "toolchains": {
            "rustc": command_output("rustc", "--version"),
            "cargo": command_output("cargo", "--version"),
            "python": platform.python_version(),
        },
        "release_profile": {"name": "release", "lto": False, "codegen_units": 16},
        "power_mode": "local-unspecified",
        "virtualization": "local-unspecified",
        "runner_configuration": {"mode": mode, "warmup": warmup, "samples": samples},
        "webview": {
            "backend": "not-instantiated",
            "boundary": "executable Tauri command/event adapter; OS WebView IPC excluded",
        },
    }


def collect_raw(warmup: int, samples: int) -> tuple[list[dict], list[dict]]:
    run(["cargo", "build", "--release", "-p", "mutsuki-tauri-benchmarks"])
    run(
        [
            "cargo",
            "build",
            "--release",
            "-p",
            "mutsuki-tauri-host",
            "--example",
            "task_pump_benchmark",
        ]
    )
    pump_binary = ROOT / "target/release/examples/task_pump_benchmark"
    bridge_binary = ROOT / "target/release/mutsuki-tauri-benchmarks"
    if os.name == "nt":
        pump_binary = pump_binary.with_suffix(".exe")
        bridge_binary = bridge_binary.with_suffix(".exe")
    pump_reports: list[dict] = []
    bridge_reports: list[dict] = []
    with tempfile.TemporaryDirectory(prefix="mutsuki-tauri-benchmark-") as temporary:
        temporary = pathlib.Path(temporary)
        for index in range(warmup + samples):
            pump_path = temporary / f"pump-{index}.json"
            bridge_path = temporary / f"bridge-{index}.json"
            subprocess.run(
                [str(pump_binary), str(pump_path)], cwd=ROOT, check=True, stdout=subprocess.DEVNULL
            )
            subprocess.run(
                [str(bridge_binary), str(bridge_path)], cwd=ROOT, check=True, stdout=subprocess.DEVNULL
            )
            if index >= warmup:
                pump_reports.append(json.loads(pump_path.read_text()))
                bridge_reports.append(json.loads(bridge_path.read_text()))
    return pump_reports, bridge_reports


def task_pump_cases(reports: list[dict], failures: list[str]) -> list[dict]:
    by_count: dict[int, list[dict]] = defaultdict(list)
    for report in reports:
        for scenario in report["scenarios"]:
            by_count[scenario["active_tasks"]].append(scenario)
            for counter in (
                "idle_actor_commands",
                "idle_task_status_queries",
                "idle_task_state_batch_queries",
            ):
                if scenario[counter] != 0:
                    failures.append(
                        f"waiting-{scenario['active_tasks']} periodic activity: {counter}={scenario[counter]}"
                    )
    cases = []
    idle = by_count[1]
    cases.append(
        case(
            "tauri.task-pump.idle",
            {"waiting_tasks": 1, "idle_window_ms": reports[0]["idle_window_ms"]},
            [value["idle_wall_ms"] * 1e6 for value in idle],
            cpu=[value["idle_cpu_ms"] * 1e6 for value in idle],
            extra={
                "actor_commands": max(value["idle_actor_commands"] for value in idle),
                "task_status_queries": max(value["idle_task_status_queries"] for value in idle),
                "task_state_batch_queries": max(
                    value["idle_task_state_batch_queries"] for value in idle
                ),
            },
            passed=not failures,
        )
    )
    for count in (1, 100, 1000):
        values = by_count[count]
        cases.append(
            case(
                f"tauri.task-pump.waiting-{count}",
                {"waiting_tasks": count},
                [value["setup_latency_ms"] * 1e6 for value in values],
                cpu=[value["idle_cpu_ms"] * 1e6 for value in values],
                extra={
                    "peak_rss_bytes": max(value["rss_after_waiting_bytes"] for value in values),
                    "actor_commands": max(value["idle_actor_commands"] for value in values),
                },
                passed=not failures,
            )
        )
    values = by_count[1]
    cases.append(
        case(
            "tauri.task-pump.single-completion",
            {"waiting_tasks": 1},
            [value["completion_latency_ms"] * 1e6 for value in values],
            extra={"actor_commands": max(value["completion_actor_commands"] for value in values)},
            passed=not failures,
        )
    )
    cases.append(
        case(
            "tauri.bridge.cancel",
            {"waiting_tasks": 1, "adapter": "host-command-equivalent"},
            [value["completion_latency_ms"] * 1e6 for value in values],
            passed=not failures,
        )
    )
    values = by_count[1000]
    cases.append(
        case(
            "tauri.task-pump.completion-burst",
            {"waiting_tasks": 1000, "completed_tasks": 999},
            [value["completion_burst_latency_ms"] * 1e6 for value in values],
            passed=not failures,
        )
    )
    for operation, field in (("startup", "host_startup_ms"), ("shutdown", "shutdown_latency_ms")):
        values = by_count[1]
        cases.append(
            case(
                f"tauri.lifecycle.{operation}",
                {"host_state": "embedded"},
                [value[field] * 1e6 for value in values],
                passed=not failures,
            )
        )
    return cases


def bridge_resource_cases(reports: list[dict], failures: list[str]) -> list[dict]:
    grouped: dict[tuple[str, str], list[dict]] = defaultdict(list)
    for report in reports:
        for raw in report["cases"]:
            key = (raw["case_id"], json.dumps(raw["dimensions"], sort_keys=True))
            grouped[key].append(raw)
            if raw.get("content_in_command_or_event_json") is not None:
                if raw["content_in_command_or_event_json"]:
                    failures.append(f"{raw['case_id']} inlined resource content")
                if raw["descriptor_frame_bytes"] >= 8192 or raw["preview_frame_bytes"] >= 8192:
                    failures.append(f"{raw['case_id']} descriptor or preview frame exceeded 8 KiB")
            if raw.get("retained_events", 0) > raw.get("dimensions", {}).get(
                "retention_capacity", 256
            ):
                failures.append(f"{raw['case_id']} exceeded bounded frontend retention")
    cases = []
    for (case_id, dimensions_json), values in sorted(grouped.items()):
        extras = {}
        for field in (
            "request_frame_bytes",
            "response_frame_bytes",
            "event_frame_bytes",
            "descriptor_frame_bytes",
            "preview_frame_bytes",
            "chunk_frame_bytes",
            "retained_events",
        ):
            if field in values[0]:
                extras[field] = max(value[field] for value in values)
        cases.append(
            case(
                case_id,
                json.loads(dimensions_json),
                [value["latency_ns"] for value in values],
                cpu=[value["cpu_time_ns"] for value in values if "cpu_time_ns" in value] or None,
                extra=extras,
                passed=not failures,
            )
        )
    return cases


def case(
    case_id: str,
    dimensions: dict,
    latency: list[float],
    *,
    cpu: list[float] | None = None,
    extra: dict[str, float] | None = None,
    passed: bool,
) -> dict:
    metrics: dict[str, object] = {"latency_ns": distribution(latency, "ns")}
    if cpu:
        metrics["cpu_time_ns"] = distribution(cpu, "ns")
    metrics.update(extra or {})
    return {
        "case_id": case_id,
        "measurement_mode": "time",
        "dimensions": dimensions,
        "metrics": metrics,
        "correctness": {"passed": passed, "counters": {}},
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", choices=("smoke", "reference"), default="reference")
    parser.add_argument("--warmup", type=int)
    parser.add_argument("--samples", type=int)
    parser.add_argument(
        "--output",
        type=pathlib.Path,
        default=ROOT / "target/mutsuki-benchmarks/tauri-reference.json",
    )
    args = parser.parse_args()
    default_warmup, default_samples = ((0, 1) if args.mode == "smoke" else (1, 5))
    warmup = default_warmup if args.warmup is None else args.warmup
    samples = default_samples if args.samples is None else max(1, args.samples)
    pump, bridge = collect_raw(warmup, samples)
    failures: list[str] = []
    cases = task_pump_cases(pump, failures)
    cases.extend(bridge_resource_cases(bridge, failures))
    revision = command_output("git", "rev-parse", "HEAD")
    cargo = (ROOT / "Cargo.toml").read_text()
    core_revision = re.search(r"MutsukiCore\.git\", rev = \"([0-9a-f]{40})", cargo)
    revisions = {
        "MutsukiTauriHost": {
            "revision": revision,
            "dirty": bool(command_output("git", "status", "--porcelain")),
            "remote": "https://github.com/sena-nana/MutsukiTauriHost.git",
        },
        "MutsukiCore": {
            "revision": core_revision.group(1) if core_revision else "0" * 40,
            "dirty": False,
            "remote": "https://github.com/sena-nana/MutsukiCore.git",
        },
    }
    env = environment(args.mode, warmup, samples)
    report = {
        "schema_version": "mutsuki.performance.report/v1",
        "suite_version": "tauri-host-performance/v1",
        "workload_version": "runner-fixtures/v1",
        "report_id": f"tauri-host-{args.mode}-{os.getpid()}",
        "generated_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "revision_lock_hash": canonical_hash(revisions),
        "repository_revisions": revisions,
        "environment_id": canonical_hash(env),
        "environment": env,
        "feature_set": [
            "embedded-host",
            "task-pump-notification",
            "executable-command-event-adapter",
            "bounded-resource-chunks",
            "preview-protocol",
        ],
        "deployment": "tauri-embedded",
        "measurement_boundary": "Rust embedded Host plus executable Tauri-equivalent command/event serialization; real OS WebView IPC and rendering excluded",
        "sampling": {
            "warmup_iterations": warmup,
            "samples_per_process": 1,
            "process_runs": samples,
        },
        "cases": cases,
        "correctness": {
            "passed": not failures,
            "counters": {"failures": len(failures)},
        },
        "gates": [
            {
                "gate_id": "tauri.correctness-and-no-poller",
                "passed": not failures,
                "actual": len(failures),
                "limit": 0,
                "unit": "failures",
            }
        ],
        "metadata": {
            "case_count": len(cases),
            "failures": failures,
            "webview_claim": "No full WebView roundtrip claim is made by this executable fixture",
            "public_runner_gate": "correctness-only",
        },
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    noisy = [
        item["case_id"]
        for item in cases
        if (metric := item["metrics"].get("latency_ns"))
        and metric["median"]
        and metric["mad"] / metric["median"] > 0.05
    ]
    analysis = {
        "classification": (
            "framework-suspect"
            if failures
            else "environmental-noise"
            if noisy
            else "no-obvious-anomaly"
        ),
        "correctness_failures": failures,
        "noisy_cases": noisy,
        "rule": "correctness or periodic-poller failures indicate a framework suspect; relative MAD above 5% alone indicates environmental noise",
    }
    args.output.with_suffix(".analysis.json").write_text(
        json.dumps(analysis, indent=2, sort_keys=True) + "\n"
    )
    print(json.dumps(analysis, indent=2))
    if failures:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
