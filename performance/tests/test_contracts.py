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
        "repository_revisions": {"MutsukiCore": {"revision": "2" * 40, "dirty": False}},
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

    def test_zero_tolerance_correctness_counter_fails(self) -> None:
        baseline = report()
        current = deepcopy(baseline)
        current["cases"][0]["correctness"]["counters"]["duplicate_execution"] = 1
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
        dirty["repository_revisions"]["MutsukiCore"]["dirty"] = True
        dirty["revision_lock_hash"] = canonical_sha256(dirty["repository_revisions"])
        dirty_bytes = (json.dumps(dirty, sort_keys=True) + "\n").encode()
        dirty_approval = dict(approval)
        dirty_approval["report_sha256"] = hashlib.sha256(dirty_bytes).hexdigest()
        dirty_approval["revision_lock_hash"] = dirty["revision_lock_hash"]
        with self.assertRaisesRegex(ContractError, "dirty"):
            validate_baseline_approval(dirty_approval, dirty_bytes, dirty)

    def test_owner_repository_snapshot_is_hashed_and_contains_owner(self) -> None:
        repositories = {"MutsukiCore": {"revision": "2" * 40, "dirty": False}}
        snapshot = {
            "schema_version": "mutsuki.performance.repository-snapshot/v1",
            "snapshot_version": "fixture/v1",
            "owner_repository": "MutsukiCore",
            "snapshot_hash": canonical_sha256(repositories),
            "repositories": repositories,
        }
        validate_repository_snapshot(snapshot)
        snapshot["owner_repository"] = "MutsukiServiceHost"
        with self.assertRaisesRegex(ContractError, "owner"):
            validate_repository_snapshot(snapshot)


if __name__ == "__main__":
    unittest.main()
