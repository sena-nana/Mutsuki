from __future__ import annotations

import json
import subprocess
import sys
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any


def add_baseline_gates(report: dict[str, Any], baseline: Mapping[str, Any]) -> dict[str, Any]:
    baseline_cases = {
        _case_key(case): case for case in _case_sequence(baseline.get("codec_results"))
    }
    gates: list[dict[str, object]] = []
    for case in _case_sequence(report.get("codec_results")):
        prior = baseline_cases[_case_key(case)]
        for metric in (
            "encode_us_p50",
            "decode_us_p50",
            "encode_peak_bytes",
            "decode_peak_bytes",
        ):
            actual = _number(case[metric], metric)
            baseline_value = _number(prior[metric], metric)
            limit = baseline_value * 1.5
            gates.append(
                {
                    "name": f"p0.python.{_case_label(case)}.{metric}",
                    "kind": "non_regression",
                    "actual": actual,
                    "baseline": baseline_value,
                    "limit": limit,
                    "passed": actual <= limit,
                }
            )

    cancel = _mapping(report.get("cancel_latency"), "cancel_latency")
    baseline_cancel = _mapping(baseline.get("cancel_latency"), "cancel_latency")
    actual_cancel = _number(cancel["p95_ms"], "cancel p95_ms")
    baseline_cancel_value = _number(baseline_cancel["p95_ms"], "cancel p95_ms")
    cancel_limit = max(2.0, baseline_cancel_value * 3.0)
    gates.append(
        {
            "name": "p0.python.cancel.p95_ms",
            "kind": "management_latency",
            "actual": actual_cancel,
            "baseline": baseline_cancel_value,
            "limit": cancel_limit,
            "passed": actual_cancel <= cancel_limit,
        }
    )
    report["baseline"] = {
        "schema_revision": baseline.get("schema_revision"),
        "core_revision": baseline.get("core_revision"),
    }
    report["gates"] = gates
    report["passed"] = all(bool(gate["passed"]) for gate in gates)
    return report


def write_report(report: Mapping[str, Any], output: Path | None) -> None:
    encoded = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if output is None:
        sys.stdout.write(encoded)
        return
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(encoded, encoding="utf-8")


def load_report(path: Path) -> Mapping[str, Any]:
    return _mapping(json.loads(path.read_text(encoding="utf-8")), str(path))


def repository_state() -> dict[str, object]:
    revision = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    status = subprocess.run(
        ["git", "status", "--porcelain"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return {"revision": revision, "dirty": bool(status)}


def _case_sequence(value: object) -> list[Mapping[str, Any]]:
    if not isinstance(value, Sequence) or isinstance(value, str | bytes | bytearray):
        raise TypeError("codec_results expects sequence")
    return [_mapping(case, "codec result") for case in value]


def _case_key(case: Mapping[str, Any]) -> tuple[object, object, object]:
    return (case["codec"], case["batch_size"], case["payload_bytes_per_entry"])


def _case_label(case: Mapping[str, Any]) -> str:
    codec, batch_size, payload_bytes = _case_key(case)
    return f"{codec}.batch-{batch_size}.payload-{payload_bytes}"


def _mapping(value: object, name: str) -> Mapping[str, Any]:
    if not isinstance(value, Mapping):
        raise TypeError(f"{name} expects mapping")
    return value


def _number(value: object, name: str) -> float:
    if not isinstance(value, int | float) or isinstance(value, bool):
        raise TypeError(f"{name} expects number")
    return float(value)
