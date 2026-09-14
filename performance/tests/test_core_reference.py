from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from copy import deepcopy
from pathlib import Path

from test_contracts import distribution, report


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "core_reference", ROOT / "scripts/run-core-reference.py"
)
reference = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(reference)


def fragment(lane: str = "time") -> dict:
    value = report()
    value["suite_version"] = "mutsuki-core/v2"
    value["measurement_boundary"] = f"fixture {lane} boundary"
    value["sampling"] = {"warmup_iterations": 1, "samples_per_process": 2, "process_runs": 1}
    value["feature_set"] = ["allocation-tracking"] if lane == "allocation" else []
    case = value["cases"][0]
    case["case_id"] = "core.observability.disabled-trace"
    case["measurement_mode"] = lane
    case["dimensions"] = {"capacity": "0", "iterations": "2", "units": "2"}
    case["correctness"]["counters"] = {
        "span_constructions": 0, "allocated_trace_capacity": 0, "retained_traces": 0,
    }
    case["metrics"] = {}
    if lane == "time":
        for metric, measured in (("latency_ns", 100.0), ("throughput_per_second", 1000.0)):
            case["metrics"][metric] = {
                **distribution(measured, 0), "sample_count": 2,
                "samples": [measured, measured],
            }
    else:
        case["metrics"]["allocated_bytes"] = 0.0
    value["gates"] = [{
        "gate_id": "fixture", "passed": True, "actual": 1, "limit": 1, "unit": "case"
    }]
    value["metadata"] = {}
    return value


class ReferenceTests(unittest.TestCase):
    def test_merges_all_samples_and_keeps_each_process_gate(self) -> None:
        first, second = fragment(), fragment()
        second["cases"][0]["dimensions"]["units"] = "3"
        merged = reference.merge_time_reports([first, second])
        self.assertEqual(merged["cases"][0]["metrics"]["latency_ns"]["samples"], [100.0] * 4)
        self.assertEqual(merged["sampling"]["process_runs"], 2)
        self.assertEqual(len(merged["gates"]), 2)

    def test_later_process_failure_is_not_lost(self) -> None:
        for failure in ("report", "case", "gate", "semantics", "analysis"):
            with self.subTest(failure=failure):
                bad = fragment()
                if failure == "report":
                    bad["correctness"]["passed"] = False
                elif failure == "case":
                    bad["cases"][0]["correctness"]["passed"] = False
                elif failure == "gate":
                    bad["gates"][0]["passed"] = False
                elif failure == "semantics":
                    bad["cases"][0]["correctness"]["counters"]["span_constructions"] = 1
                else:
                    bad["cases"][0]["correctness"]["counters"]["duplicate_execution"] = 1
                with self.assertRaises((ValueError, RuntimeError)):
                    reference.merge_time_reports([fragment(), bad])

    def test_case_matrix_samples_and_metadata_must_match(self) -> None:
        for mismatch in (
            "extra-case", "missing-metric", "missing-sample", "warmup", "workload", "boundary"
        ):
            with self.subTest(mismatch=mismatch):
                bad = fragment()
                if mismatch == "extra-case":
                    extra = deepcopy(bad["cases"][0])
                    extra["case_id"] = "core.extra"
                    bad["cases"].append(extra)
                elif mismatch == "missing-metric":
                    del bad["cases"][0]["metrics"]["throughput_per_second"]
                elif mismatch == "missing-sample":
                    metric = bad["cases"][0]["metrics"]["throughput_per_second"]
                    metric["sample_count"] = 1
                    metric["samples"] = [1000.0]
                elif mismatch == "warmup":
                    bad["sampling"]["warmup_iterations"] = 0
                elif mismatch == "workload":
                    bad["workload_version"] = "different/v1"
                else:
                    bad["measurement_boundary"] = "different boundary"
                with self.assertRaises((ValueError, RuntimeError)):
                    reference.merge_time_reports([fragment(), bad])

    def test_allocation_and_requested_sampling_are_validated(self) -> None:
        for mismatch in ("failure", "environment", "revision", "samples", "lane", "missing-case"):
            with self.subTest(mismatch=mismatch):
                allocation = fragment("allocation")
                if mismatch == "failure":
                    allocation["gates"][0]["passed"] = False
                elif mismatch == "environment":
                    allocation["environment_id"] = "0" * 64
                elif mismatch == "revision":
                    allocation["revision_lock_hash"] = "0" * 64
                elif mismatch == "samples":
                    allocation["sampling"]["samples_per_process"] = 3
                elif mismatch == "lane":
                    allocation["cases"][0]["measurement_mode"] = "time"
                else:
                    allocation["cases"][0]["case_id"] = "core.other"
                with self.assertRaises((ValueError, RuntimeError)):
                    reference.build_reference(
                        [fragment(), fragment()], allocation, warmup=1, samples=2
                    )

    def test_reuse_cli_returns_failure_for_bad_fragments(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "report.json"
            fragments = Path(directory) / "report-fragments"
            fragments.mkdir()
            for name, value in (
                ("time-1", fragment()), ("time-2", fragment()),
                ("allocation", fragment("allocation")),
            ):
                (fragments / f"{name}.json").write_text(json.dumps(value))
            command = [
                sys.executable, str(ROOT / "scripts/run-core-reference.py"),
                "--mode", "smoke", "--process-runs", "2", "--samples", "2",
                "--warmup", "1", "--reuse-fragments", "--output", str(output),
            ]
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue(json.loads(output.read_text())["correctness"]["passed"])
            output.unlink()
            bad = fragment()
            bad["correctness"]["passed"] = False
            (fragments / "time-2.json").write_text(json.dumps(bad))
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
