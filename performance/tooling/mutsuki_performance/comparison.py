from __future__ import annotations

from typing import Any

from .validation import ContractError, validate_report


ZERO_TOLERANCE_COUNTERS = {
    "duplicate_committed_results",
    "stale_results_accepted",
    "unsafe_retries",
    "unsafe_remote_placements",
    "duplicate_execution",
    "duplicate_executions",
    "duplicate_commits",
    "duplicate_commits_accepted",
    "stale_outputs_accepted",
    "unsafe_automatic_retries",
    "hash_mismatches",
    "public_network_requests",
    "wrong_routes",
    "unexpected_errors",
    "failures",
    "failed_gates",
}


def _case_key(case: dict[str, Any]) -> tuple[str, str, str]:
    dimensions = {
        name: value
        for name, value in case["dimensions"].items()
        if name not in {"iterations", "units"}
    }
    return (
        case["case_id"],
        case["measurement_mode"],
        repr(sorted(dimensions.items())),
    )


def compare_reports(
    baseline: dict[str, Any], current: dict[str, Any]
) -> dict[str, Any]:
    validate_report(baseline)
    validate_report(current)
    if baseline["environment_id"] != current["environment_id"]:
        raise ContractError(
            "baseline and current report use different environment_id values"
        )
    if baseline["measurement_boundary"] != current["measurement_boundary"]:
        raise ContractError(
            "baseline and current report use different measurement boundaries"
        )
    previous = {_case_key(case): case for case in baseline["cases"]}
    findings: list[dict[str, Any]] = [
        {
            "case_id": "report",
            "metric": "correctness.passed",
            "kind": "zero-tolerance",
            "actual": int(not current["correctness"]["passed"]),
            "limit": 0,
            "passed": current["correctness"]["passed"],
        }
    ]
    _append_zero_tolerance_counters(
        "report", current["correctness"]["counters"], findings
    )
    for case in current["cases"]:
        old = previous.get(_case_key(case))
        if old is None:
            findings.append(
                {"case_id": case["case_id"], "kind": "unmatched", "passed": True}
            )
            continue
        _compare_distribution(case, old, "latency_ns", findings)
        _compare_distribution(
            case, old, "throughput_per_second", findings, lower_is_better=False
        )
        _compare_scalar(case, old, "allocated_bytes", findings, minimum_delta=64.0)
        _compare_scalar(case, old, "peak_rss_bytes", findings)
        findings.append(
            {
                "case_id": case["case_id"],
                "metric": "correctness.passed",
                "kind": "zero-tolerance",
                "actual": int(not case["correctness"]["passed"]),
                "limit": 0,
                "passed": case["correctness"]["passed"],
            }
        )
        counters = case["correctness"]["counters"]
        _append_zero_tolerance_counters(case["case_id"], counters, findings)
        slope = counters.get("retained_growth_slope_bytes_per_sample")
        if slope is not None:
            findings.append(
                {
                    "case_id": case["case_id"],
                    "metric": "retained_growth_slope_bytes_per_sample",
                    "kind": "bounded-memory",
                    "actual": slope,
                    "limit": 0,
                    "passed": slope <= 0,
                }
            )
    return {"passed": all(item["passed"] for item in findings), "findings": findings}


def _append_zero_tolerance_counters(
    case_id: str, counters: dict[str, int], findings: list[dict[str, Any]]
) -> None:
    for counter in sorted(ZERO_TOLERANCE_COUNTERS & counters.keys()):
        value = counters[counter]
        findings.append(
            {
                "case_id": case_id,
                "metric": counter,
                "kind": "zero-tolerance",
                "actual": value,
                "limit": 0,
                "passed": value == 0,
            }
        )


def _compare_distribution(
    case: dict[str, Any],
    old: dict[str, Any],
    metric: str,
    findings: list[dict[str, Any]],
    *,
    lower_is_better: bool = True,
) -> None:
    if metric not in case["metrics"] or metric not in old["metrics"]:
        return
    current = case["metrics"][metric]
    baseline = old["metrics"][metric]
    if lower_is_better:
        median_delta = current["median"] - baseline["median"]
        median_limit = max(baseline["median"] * 0.10, baseline["mad"] * 3.0)
        median_passed = median_delta <= median_limit
        p99_passed = current["p99"] <= baseline["p99"] * 1.20
    else:
        median_delta = baseline["median"] - current["median"]
        median_limit = baseline["median"] * 0.10
        median_passed = median_delta <= median_limit
        p99_passed = True
    findings.extend(
        [
            {
                "case_id": case["case_id"],
                "metric": f"{metric}.median",
                "kind": "relative-regression",
                "actual": median_delta,
                "limit": median_limit,
                "passed": median_passed,
            },
            {
                "case_id": case["case_id"],
                "metric": f"{metric}.p99",
                "kind": "relative-regression",
                "actual": current["p99"],
                "limit": baseline["p99"] * 1.20,
                "passed": p99_passed,
            },
        ]
    )


def _compare_scalar(
    case: dict[str, Any],
    old: dict[str, Any],
    metric: str,
    findings: list[dict[str, Any]],
    *,
    minimum_delta: float = 0.0,
) -> None:
    if metric not in case["metrics"] or metric not in old["metrics"]:
        return
    current = float(case["metrics"][metric])
    baseline = float(old["metrics"][metric])
    limit = max(baseline * 0.10, minimum_delta)
    delta = current - baseline
    findings.append(
        {
            "case_id": case["case_id"],
            "metric": metric,
            "kind": "relative-regression",
            "actual": delta,
            "limit": limit,
            "passed": delta <= limit,
        }
    )
