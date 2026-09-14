from __future__ import annotations

import json
import sys
import unittest
from copy import deepcopy
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tooling"))

from mutsuki_performance import (  # noqa: E402
    ContractError,
    canonical_sha256,
    compare_reports,
    validate_baseline_approval,
    validate_report,
    validate_repository_snapshot,
    validate_workload,
)


def distribution(value: float, mad: float = 1.0) -> dict[str, float | str | int]:
    return {
        "median": value,
        "p95": value,
        "p99": value,
        "mad": mad,
        "min": value,
        "max": value,
        "unit": "ns",
        "sample_count": 5,
    }


def report() -> dict:
    environment = {
        "cpu_model": "fixture cpu",
        "cpu_topology": "logical=8",
        "ram_bytes": 16_000_000_000,
        "os": "fixture os",
        "kernel": "fixture kernel",
        "architecture": "aarch64",
        "target_triple": "aarch64-apple-darwin",
        "toolchains": {"rust": "fixture"},
        "release_profile": {"name": "release", "lto": "thin", "codegen_units": 1},
        "power_mode": "ac",
        "virtualization": "none",
        "runner_configuration": {},
    }
    value = {
        "schema_version": "mutsuki.performance.report/v1",
        "suite_version": "fixture/v1",
        "workload_version": "fixture/v1",
        "report_id": "fixture-report",
        "generated_at": "2026-07-17T00:00:00Z",
        "revision_lock_hash": "0" * 64,
        "repository_revisions": {"Mutsuki": {"revision": "2" * 40, "dirty": False}},
        "environment_id": canonical_sha256(environment),
        "environment": environment,
        "feature_set": [],
        "deployment": "builtin",
        "measurement_boundary": "fixture boundary",
        "sampling": {
            "warmup_iterations": 1,
            "samples_per_process": 5,
            "process_runs": 3,
        },
        "cases": [
            {
                "case_id": "core.fixture",
                "measurement_mode": "time",
                "dimensions": {},
                "metrics": {
                    "latency_ns": distribution(100.0, 2.0),
                    "throughput_per_second": distribution(1000.0),
                    "allocated_bytes": 64.0,
                    "peak_rss_bytes": 1024.0,
                },
                "correctness": {"passed": True, "counters": {"duplicate_execution": 0}},
            }
        ],
        "correctness": {"passed": True, "counters": {}},
    }
    value["revision_lock_hash"] = canonical_sha256(value["repository_revisions"])
    return value


class ContractTests(unittest.TestCase):
    def test_runner_workload_is_valid_and_complete(self) -> None:
        workload = {
            "schema_version": "mutsuki.performance.workload/v1",
            "workload_version": "fixture/v1",
            "seed": 1,
            "fixtures": [
                {
                    "fixture_id": "core.fixture",
                    "behavior": "Return one deterministic result.",
                    "input": {},
                    "expected_output_hash": "1" * 64,
                    "dimensions": {},
                }
            ],
        }
        validate_workload(workload)
        self.assertEqual(len(workload["fixtures"]), 1)

    def test_report_checks_environment_fingerprint_and_percentile_order(self) -> None:
        value = report()
        validate_report(value)
        value["environment"]["cpu_model"] = "changed"
        with self.assertRaisesRegex(ContractError, "environment_id"):
            validate_report(value)

    def test_comparison_rejects_statistically_significant_regression(self) -> None:
        baseline = report()
        current = deepcopy(baseline)
        current["report_id"] = "current"
        current["cases"][0]["metrics"]["latency_ns"] = distribution(130.0, 2.0)
        comparison = compare_reports(baseline, current)
        self.assertFalse(comparison["passed"])
        self.assertTrue(
            any(not finding["passed"] for finding in comparison["findings"])
        )

    def test_comparison_ignores_observed_iteration_and_unit_counts(self) -> None:
        baseline = report()
        baseline["cases"][0]["dimensions"] = {
            "legacy_case_id": "host/events-pagination/entries-1",
            "entries": "1",
            "iterations": "1",
            "units": "9",
        }
        current = deepcopy(baseline)
        current["report_id"] = "current"
        current["cases"][0]["dimensions"]["iterations"] = "2"
        current["cases"][0]["dimensions"]["units"] = "12"
        current["cases"][0]["metrics"]["latency_ns"] = distribution(130.0, 2.0)

        comparison = compare_reports(baseline, current)

        self.assertFalse(comparison["passed"])
        self.assertFalse(
            any(finding["kind"] == "unmatched" for finding in comparison["findings"])
        )

    def test_zero_tolerance_correctness_counter_fails(self) -> None:
        baseline = report()
        current = deepcopy(baseline)
        current["cases"][0]["correctness"]["counters"]["duplicate_execution"] = 1
        self.assertFalse(compare_reports(baseline, current)["passed"])

    def test_disabled_trace_requires_matching_baseline(self) -> None:
        baseline = report()
        current = deepcopy(baseline)
        current["cases"][0]["case_id"] = "core.observability.disabled-trace"
        self.assertFalse(compare_reports(baseline, current)["passed"])
        baseline = deepcopy(current)
        self.assertTrue(compare_reports(baseline, current)["passed"])
        current["cases"][0]["dimensions"]["capacity"] = "64"
        self.assertFalse(compare_reports(baseline, current)["passed"])

    def test_disabled_trace_detects_regression_in_matched_baseline(self) -> None:
        baseline = report()
        baseline["cases"][0]["case_id"] = "core.observability.disabled-trace"
        current = deepcopy(baseline)
        current["cases"][0]["metrics"]["latency_ns"] = distribution(130.0, 2.0)
        self.assertFalse(compare_reports(baseline, current)["passed"])

    def test_disabled_trace_requires_lane_metrics_on_both_sides(self) -> None:
        for lane, required in (
            ("time", ("latency_ns", "throughput_per_second")),
            ("allocation", ("allocated_bytes",)),
        ):
            complete = report()
            case = complete["cases"][0]
            case["case_id"] = "core.observability.disabled-trace"
            case["measurement_mode"] = lane
            case["metrics"] = {name: case["metrics"][name] for name in required}
            self.assertTrue(compare_reports(complete, complete)["passed"])
            missing_sets = [required] + [(name,) for name in required if len(required) > 1]
            for missing in missing_sets:
                for side in ("baseline", "current"):
                    with self.subTest(lane=lane, missing=missing, side=side):
                        baseline, current = deepcopy(complete), deepcopy(complete)
                        incomplete = baseline if side == "baseline" else current
                        for name in missing:
                            del incomplete["cases"][0]["metrics"][name]
                        validate_report(incomplete)
                        self.assertFalse(compare_reports(baseline, current)["passed"])

    def test_disabled_trace_empty_baseline_cannot_hide_tenfold_regression(self) -> None:
        baseline = report()
        baseline["cases"][0]["case_id"] = "core.observability.disabled-trace"
        current = deepcopy(baseline)
        baseline["cases"][0]["metrics"] = {}
        current["cases"][0]["metrics"]["latency_ns"] = distribution(1000.0)
        current["cases"][0]["metrics"]["throughput_per_second"] = distribution(100.0)
        self.assertFalse(compare_reports(baseline, current)["passed"])

    def test_current_cannot_drop_an_approved_observability_case(self) -> None:
        baseline = report()
        trace = deepcopy(baseline["cases"][0])
        trace["case_id"] = "core.observability.disabled-trace"
        baseline["cases"].append(trace)
        current = deepcopy(baseline)
        current["cases"].pop()
        self.assertFalse(compare_reports(baseline, current)["passed"])

    def test_metrics_reject_nonfinite_values_and_wrong_shapes(self) -> None:
        for invalid in (float("inf"), float("nan"), float("-inf"), True):
            for target in ("statistic", "sample", "scalar"):
                with self.subTest(invalid=invalid, target=target):
                    value = report()
                    metrics = value["cases"][0]["metrics"]
                    if target == "statistic":
                        for key in ("min", "median", "p95", "p99", "max", "mad"):
                            metrics["latency_ns"][key] = invalid
                    elif target == "sample":
                        metrics["latency_ns"]["samples"] = [invalid] * 5
                    else:
                        metrics["allocated_bytes"] = invalid
                    with self.assertRaises(ContractError):
                        validate_report(value)
        for name, invalid in (("latency_ns", 100.0), ("allocated_bytes", distribution(1.0))):
            with self.subTest(metric=name):
                value = report()
                value["cases"][0]["metrics"][name] = invalid
                with self.assertRaises(ContractError):
                    validate_report(value)

    def test_failed_reports_cannot_be_approved_baselines(self) -> None:
        import hashlib

        for failure in ("report", "case", "gate"):
            with self.subTest(failure=failure):
                value = report()
                if failure == "report":
                    value["correctness"]["passed"] = False
                elif failure == "case":
                    value["cases"][0]["correctness"]["passed"] = False
                else:
                    value["gates"] = [{"gate_id": "test", "passed": False, "actual": 2, "limit": 1, "unit": "ns"}]
                raw = json.dumps(value).encode()
                approval = {
                    "schema_version": "mutsuki.performance.baseline-approval/v1",
                    "report_sha256": hashlib.sha256(raw).hexdigest(),
                    "revision_lock_hash": value["revision_lock_hash"],
                    "environment_id": value["environment_id"],
                    "approved_by": "fixture-reviewer",
                    "approved_at": "2026-07-17T00:00:00Z",
                    "reason": "fixture approval",
                }
                with self.assertRaises(ContractError):
                    validate_baseline_approval(approval, raw, value)

    def test_core_comparison_requires_observability_in_each_measured_lane(self) -> None:
        for lane in ("time", "allocation"):
            baseline = report()
            baseline["suite_version"] = "mutsuki-core/v2"
            baseline["cases"][0]["measurement_mode"] = lane
            self.assertFalse(compare_reports(baseline, deepcopy(baseline))["passed"])

    def test_unmatched_case_and_gate_failures_are_not_skipped(self) -> None:
        for failure in ("case", "counter", "gate"):
            with self.subTest(failure=failure):
                baseline = report()
                current = deepcopy(baseline)
                case = current["cases"][0]
                case["case_id"] = "core.new-case"
                if failure == "case":
                    case["correctness"]["passed"] = False
                elif failure == "counter":
                    case["correctness"]["counters"]["duplicate_execution"] = 1
                else:
                    current["gates"] = [{"gate_id": "fixture", "passed": False,
                                         "actual": 2, "limit": 1, "unit": "ns"}]
                self.assertFalse(compare_reports(baseline, current)["passed"])

    def test_owner_correctness_failure_always_fails_comparison(self) -> None:
        baseline = report()
        current = deepcopy(baseline)
        current["correctness"]["passed"] = False
        self.assertFalse(compare_reports(baseline, current)["passed"])

    def test_baseline_approval_is_bound_to_exact_report_bytes(self) -> None:
        import hashlib

        value = report()
        report_bytes = (json.dumps(value, sort_keys=True) + "\n").encode()
        approval = {
            "schema_version": "mutsuki.performance.baseline-approval/v1",
            "report_sha256": hashlib.sha256(report_bytes).hexdigest(),
            "revision_lock_hash": value["revision_lock_hash"],
            "environment_id": value["environment_id"],
            "approved_by": "fixture-reviewer",
            "approved_at": "2026-07-17T00:00:00Z",
            "reason": "fixture approval",
        }
        validate_baseline_approval(approval, report_bytes, value)
        with self.assertRaisesRegex(ContractError, "report_sha256"):
            validate_baseline_approval(approval, report_bytes + b" ", value)
        dirty = report()
        dirty["repository_revisions"]["Mutsuki"]["dirty"] = True
        dirty["revision_lock_hash"] = canonical_sha256(dirty["repository_revisions"])
        dirty_bytes = (json.dumps(dirty, sort_keys=True) + "\n").encode()
        dirty_approval = dict(approval)
        dirty_approval["report_sha256"] = hashlib.sha256(dirty_bytes).hexdigest()
        dirty_approval["revision_lock_hash"] = dirty["revision_lock_hash"]
        with self.assertRaisesRegex(ContractError, "dirty"):
            validate_baseline_approval(dirty_approval, dirty_bytes, dirty)

    def test_owner_repository_snapshot_is_hashed_and_contains_owner(self) -> None:
        repositories = {"Mutsuki": {"revision": "2" * 40, "dirty": False}}
        snapshot = {
            "schema_version": "mutsuki.performance.repository-snapshot/v1",
            "snapshot_version": "fixture/v1",
            "owner_repository": "Mutsuki",
            "snapshot_hash": canonical_sha256(repositories),
            "repositories": repositories,
        }
        validate_repository_snapshot(snapshot)
        snapshot["owner_repository"] = "MutsukiServiceHost"
        with self.assertRaisesRegex(ContractError, "owner"):
            validate_repository_snapshot(snapshot)


if __name__ == "__main__":
    unittest.main()
