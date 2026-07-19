#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import math
import statistics
import subprocess
from copy import deepcopy
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]


def distribution(samples: list[float], unit: str) -> dict[str, Any]:
    ordered = sorted(samples)
    median = statistics.median_low(ordered)
    deviations = sorted(abs(sample - median) for sample in ordered)

    def percentile(quantile: float) -> float:
        index = max(0, min(len(ordered) - 1, math.ceil(len(ordered) * quantile) - 1))
        return ordered[index]

    return {
        "median": median,
        "p95": percentile(0.95),
        "p99": percentile(0.99),
        "mad": statistics.median_low(deviations),
        "min": ordered[0],
        "max": ordered[-1],
        "unit": unit,
        "sample_count": len(ordered),
        "samples": ordered,
    }


def run_fragment(
    *,
    mode: str,
    lane: str,
    warmup: int,
    samples: int,
    output: Path,
) -> dict[str, Any]:
    command = ["cargo", "run", "--release"]
    if lane == "allocation":
        command.extend(["--features", "allocation-tracking"])
    command.extend(
        [
            "--package",
            "mutsuki-runtime-benchmarks",
            "--",
            mode,
            "--lane",
            lane,
            "--warmup",
            str(warmup),
            "--samples",
            str(samples),
            "--gate",
            "smoke",
            "--output",
            str(output),
        ]
    )
    subprocess.run(command, cwd=ROOT, check=True)
    return json.loads(output.read_text(encoding="utf-8"))


def case_key(case: dict[str, Any]) -> str:
    dimensions = json.dumps(
        {
            name: value
            for name, value in case["dimensions"].items()
            if name not in {"iterations", "units"}
        },
        sort_keys=True,
        separators=(",", ":"),
    )
    return f"{case['case_id']}|{case['measurement_mode']}|{dimensions}"


def merge_time_reports(reports: list[dict[str, Any]]) -> dict[str, Any]:
    reference = deepcopy(reports[0])
    for report in reports[1:]:
        if report["environment_id"] != reference["environment_id"]:
            raise RuntimeError("environment fingerprint changed between process rounds")
        if report["revision_lock_hash"] != reference["revision_lock_hash"]:
            raise RuntimeError(
                "repository revision snapshot changed between process rounds"
            )
    cases_by_report = [
        {case_key(case): case for case in report["cases"]} for report in reports
    ]
    merged_cases: list[dict[str, Any]] = []
    for template in reference["cases"]:
        key = case_key(template)
        merged = deepcopy(template)
        for metric in ("latency_ns", "throughput_per_second", "cpu_time_ns"):
            values: list[float] = []
            unit = None
            for cases in cases_by_report:
                current = cases[key]["metrics"].get(metric)
                if current:
                    values.extend(current["samples"])
                    unit = current["unit"]
            if values:
                merged["metrics"][metric] = distribution(values, unit or "unit")
        if template["measurement_mode"] == "system":
            merged["metrics"]["peak_rss_bytes"] = max(
                cases[key]["metrics"].get("peak_rss_bytes", 0)
                for cases in cases_by_report
            )
            merged["metrics"]["context_switches"] = statistics.median_low(
                [
                    cases[key]["metrics"].get("context_switches", 0)
                    for cases in cases_by_report
                ]
            )
        merged_cases.append(merged)
    reference["cases"] = merged_cases
    reference["sampling"]["process_runs"] = len(reports)
    reference["report_id"] = reference["report_id"].replace("time", "reference")
    reference["measurement_boundary"] = (
        "Core runtime kernel and Host-facing component reference: system-allocator time lane, "
        "separate tracking-allocation process, and whole-process system metrics; no ABI/process/network"
    )
    return reference


def analyze(report: dict[str, Any]) -> dict[str, Any]:
    harness_errors: list[str] = []
    noisy_cases: list[dict[str, Any]] = []
    framework_suspects: list[str] = []
    time_cases = [
        case for case in report["cases"] if case["measurement_mode"] == "time"
    ]
    expected_samples = (
        report["sampling"]["samples_per_process"] * report["sampling"]["process_runs"]
    )
    for case in time_cases:
        latency = case["metrics"].get("latency_ns")
        throughput = case["metrics"].get("throughput_per_second")
        if not latency or latency["sample_count"] != expected_samples:
            harness_errors.append(f"{case['case_id']}: incomplete latency sample set")
            continue
        if not all(math.isfinite(value) and value >= 0 for value in latency["samples"]):
            harness_errors.append(f"{case['case_id']}: invalid latency sample")
        if latency["median"] > 0 and latency["mad"] / latency["median"] > 0.05:
            noisy_cases.append(
                {
                    "case_id": case["case_id"],
                    "legacy_case_id": case["dimensions"].get("legacy_case_id"),
                    "relative_mad": latency["mad"] / latency["median"],
                }
            )
        if latency and throughput:
            units = float(case["dimensions"].get("units", 1))
            iterations = float(case["dimensions"].get("iterations", 1))
            if units <= 0 or iterations <= 0:
                harness_errors.append(f"{case['case_id']}: invalid units/iterations")
    zero_tolerance = {
        "duplicate_committed_results",
        "stale_results_accepted",
        "unsafe_retries",
        "unsafe_remote_placements",
        "duplicate_execution",
    }
    for case in report["cases"]:
        for name, value in case["correctness"]["counters"].items():
            if name in zero_tolerance and value != 0:
                framework_suspects.append(f"{case['case_id']}: {name}={value}")
            if name == "retained_growth_slope_bytes_per_sample" and value > 0:
                framework_suspects.append(
                    f"{case['case_id']}: retained growth slope={value}"
                )
    noisy_ratio = len(noisy_cases) / max(1, len(time_cases))
    if harness_errors:
        classification = "test-implementation-error"
        judgment = "The report is structurally inconsistent; fix the benchmark implementation before attributing data to the framework."
    elif framework_suspects:
        classification = "framework-suspect"
        judgment = "Correctness or bounded-memory invariants failed; investigate the owning runtime path before accepting a baseline."
    elif noisy_ratio > 0.20:
        classification = "environmental-noise"
        judgment = "Noise is broad across cases, which is more consistent with host/environment instability than one framework path."
    elif noisy_cases:
        classification = "case-specific-noise"
        judgment = "Only a limited set of cases is noisy; inspect fixture isolation and sample duration before treating it as a framework regression."
    else:
        classification = "no-obvious-anomaly"
        judgment = "Sampling, correctness counters and cross-process dispersion show no immediate benchmark-implementation or framework anomaly."
    return {
        "classification": classification,
        "judgment": judgment,
        "expected_time_samples_per_case": expected_samples,
        "time_case_count": len(time_cases),
        "noisy_case_count": len(noisy_cases),
        "noisy_case_ratio": noisy_ratio,
        "noisy_cases": noisy_cases,
        "harness_errors": harness_errors,
        "framework_suspects": framework_suspects,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", choices=("smoke", "full"), default="full")
    parser.add_argument("--process-runs", type=int, default=3)
    parser.add_argument("--warmup", type=int, default=1)
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument(
        "--output",
        type=Path,
        default=ROOT / "target/mutsuki-benchmarks/core-reference.json",
    )
    parser.add_argument("--analysis-output", type=Path)
    parser.add_argument(
        "--reuse-fragments",
        action="store_true",
        help="rebuild the report from the complete existing fragment set without rerunning workloads",
    )
    args = parser.parse_args()
    if args.process_runs < 2 or args.samples < 2 or args.warmup < 0:
        raise SystemExit(
            "reference runs require process-runs >= 2, samples >= 2 and warmup >= 0"
        )
    fragments = args.output.parent / f"{args.output.stem}-fragments"
    fragments.mkdir(parents=True, exist_ok=True)
    time_paths = [
        fragments / f"time-{index + 1}.json" for index in range(args.process_runs)
    ]
    allocation_path = fragments / "allocation.json"
    if args.reuse_fragments:
        missing = [
            str(path) for path in [*time_paths, allocation_path] if not path.is_file()
        ]
        if missing:
            raise SystemExit("fragment set is incomplete:\n" + "\n".join(missing))
        time_reports = [
            json.loads(path.read_text(encoding="utf-8")) for path in time_paths
        ]
        allocation = json.loads(allocation_path.read_text(encoding="utf-8"))
    else:
        time_reports = [
            run_fragment(
                mode=args.mode,
                lane="time",
                warmup=args.warmup,
                samples=args.samples,
                output=path,
            )
            for path in time_paths
        ]
        allocation = run_fragment(
            mode=args.mode,
            lane="allocation",
            warmup=args.warmup,
            samples=args.samples,
            output=allocation_path,
        )
    report = merge_time_reports(time_reports)
    report["cases"].extend(allocation["cases"])
    report["feature_set"] = sorted(
        set(report["feature_set"] + ["separate-process-allocation-lane"])
    )
    report["metadata"]["allocation_samples_per_process"] = str(args.samples)
    report["correctness"]["passed"] = (
        report["correctness"]["passed"] and allocation["correctness"]["passed"]
    )
    report["gates"].extend(allocation["gates"])
    analysis = analyze(report)
    report["metadata"]["anomaly_classification"] = analysis["classification"]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    analysis_output = args.analysis_output or args.output.with_name(
        f"{args.output.stem}-analysis.json"
    )
    analysis_output.write_text(
        json.dumps(analysis, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(f"reference report: {args.output}")
    print(f"anomaly analysis: {analysis_output} ({analysis['classification']})")


if __name__ == "__main__":
    main()
