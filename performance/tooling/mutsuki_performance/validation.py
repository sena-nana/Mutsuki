from __future__ import annotations

import datetime as dt
import hashlib
import re
from typing import Any

from .fingerprint import canonical_sha256


HEX_64 = re.compile(r"^[a-f0-9]{64}$")
HEX_40 = re.compile(r"^[a-f0-9]{40}$")
CASE_ID = re.compile(r"^[a-z0-9][a-z0-9._-]+$")


class ContractError(ValueError):
    pass


def _require(mapping: dict[str, Any], fields: tuple[str, ...], where: str) -> None:
    missing = [field for field in fields if field not in mapping]
    if missing:
        raise ContractError(f"{where} missing required fields: {', '.join(missing)}")


def _distribution(value: Any, where: str) -> None:
    if not isinstance(value, dict):
        raise ContractError(f"{where} must be an object")
    _require(value, ("median", "p95", "p99", "mad", "min", "max", "unit"), where)
    numbers = [value[name] for name in ("median", "p95", "p99", "mad", "min", "max")]
    if not all(isinstance(number, (int, float)) and number >= 0 for number in numbers):
        raise ContractError(f"{where} contains a negative or non-numeric statistic")
    if (
        not value["min"]
        <= value["median"]
        <= value["p95"]
        <= value["p99"]
        <= value["max"]
    ):
        raise ContractError(f"{where} percentile ordering is invalid")
    if "sample_count" in value:
        if not isinstance(value["sample_count"], int) or value["sample_count"] < 1:
            raise ContractError(f"{where}.sample_count must be a positive integer")
        if "samples" in value and len(value["samples"]) != value["sample_count"]:
            raise ContractError(f"{where}.samples does not match sample_count")
    if "samples" in value and not all(
        isinstance(sample, (int, float)) and sample >= 0 for sample in value["samples"]
    ):
        raise ContractError(
            f"{where}.samples contains a negative or non-numeric sample"
        )


def validate_report(report: Any) -> None:
    if not isinstance(report, dict):
        raise ContractError("report must be an object")
    _require(
        report,
        (
            "schema_version",
            "suite_version",
            "workload_version",
            "report_id",
            "generated_at",
            "revision_lock_hash",
            "repository_revisions",
            "environment_id",
            "environment",
            "feature_set",
            "deployment",
            "measurement_boundary",
            "sampling",
            "cases",
            "correctness",
        ),
        "report",
    )
    if report["schema_version"] != "mutsuki.performance.report/v1":
        raise ContractError("unsupported report schema_version")
    if not HEX_64.fullmatch(report["revision_lock_hash"]):
        raise ContractError("revision_lock_hash must be a lowercase SHA-256")
    if not HEX_64.fullmatch(report["environment_id"]):
        raise ContractError("environment_id must be a lowercase SHA-256")
    if canonical_sha256(report["environment"]) != report["environment_id"]:
        raise ContractError(
            "environment_id does not match the canonical environment fingerprint"
        )
    if not report["repository_revisions"]:
        raise ContractError("repository_revisions cannot be empty")
    for name, revision in report["repository_revisions"].items():
        _require(revision, ("revision", "dirty"), f"repository_revisions.{name}")
        if not HEX_40.fullmatch(revision["revision"]):
            raise ContractError(
                f"repository_revisions.{name}.revision is not a full commit SHA"
            )
        if not isinstance(revision["dirty"], bool):
            raise ContractError(f"repository_revisions.{name}.dirty must be boolean")
    if canonical_sha256(report["repository_revisions"]) != report["revision_lock_hash"]:
        raise ContractError(
            "revision_lock_hash does not match canonical repository_revisions"
        )
    sampling = report["sampling"]
    _require(
        sampling,
        ("warmup_iterations", "samples_per_process", "process_runs"),
        "sampling",
    )
    if sampling["samples_per_process"] < 1 or sampling["process_runs"] < 1:
        raise ContractError("sampling counts must be positive")
    cases = report["cases"]
    if not isinstance(cases, list) or not cases:
        raise ContractError("cases must be a non-empty array")
    seen: set[tuple[str, str, str]] = set()
    for index, case in enumerate(cases):
        where = f"cases[{index}]"
        _require(
            case,
            ("case_id", "measurement_mode", "dimensions", "metrics", "correctness"),
            where,
        )
        if not CASE_ID.fullmatch(case["case_id"]):
            raise ContractError(f"{where}.case_id is not stable kebab/dot notation")
        if case["measurement_mode"] not in {
            "time",
            "allocation",
            "diagnostic",
            "system",
        }:
            raise ContractError(f"{where}.measurement_mode is invalid")
        key = (
            case["case_id"],
            case["measurement_mode"],
            repr(sorted(case["dimensions"].items())),
        )
        if key in seen:
            raise ContractError(f"duplicate case/lane/dimensions: {case['case_id']}")
        seen.add(key)
        for metric, value in case["metrics"].items():
            if isinstance(value, dict):
                _distribution(value, f"{where}.metrics.{metric}")
            elif not isinstance(value, (int, float)):
                raise ContractError(
                    f"{where}.metrics.{metric} must be numeric or a distribution"
                )
        _validate_correctness(case["correctness"], f"{where}.correctness")
    _validate_correctness(report["correctness"], "correctness")


def _validate_correctness(value: Any, where: str) -> None:
    if not isinstance(value, dict):
        raise ContractError(f"{where} must be an object")
    _require(value, ("passed", "counters"), where)
    if not isinstance(value["passed"], bool):
        raise ContractError(f"{where}.passed must be boolean")
    if "output_hash" in value and not HEX_64.fullmatch(value["output_hash"]):
        raise ContractError(f"{where}.output_hash must be a lowercase SHA-256")
    if not all(isinstance(counter, int) for counter in value["counters"].values()):
        raise ContractError(f"{where}.counters must contain integers")


def validate_workload(workload: Any) -> None:
    if not isinstance(workload, dict):
        raise ContractError("workload must be an object")
    _require(
        workload, ("schema_version", "workload_version", "seed", "fixtures"), "workload"
    )
    if workload["schema_version"] != "mutsuki.performance.workload/v1":
        raise ContractError("unsupported workload schema_version")
    seen: set[str] = set()
    for index, fixture in enumerate(workload["fixtures"]):
        where = f"fixtures[{index}]"
        _require(
            fixture,
            ("fixture_id", "behavior", "input", "expected_output_hash", "dimensions"),
            where,
        )
        if fixture["fixture_id"] in seen:
            raise ContractError(f"duplicate fixture_id: {fixture['fixture_id']}")
        seen.add(fixture["fixture_id"])
        if not HEX_64.fullmatch(fixture["expected_output_hash"]):
            raise ContractError(
                f"{where}.expected_output_hash must be a lowercase SHA-256"
            )


def validate_repository_snapshot(snapshot: Any) -> None:
    if not isinstance(snapshot, dict):
        raise ContractError("repository snapshot must be an object")
    _require(
        snapshot,
        (
            "schema_version",
            "snapshot_version",
            "owner_repository",
            "snapshot_hash",
            "repositories",
        ),
        "repository snapshot",
    )
    if snapshot["schema_version"] != "mutsuki.performance.repository-snapshot/v1":
        raise ContractError("unsupported repository snapshot schema_version")
    if (
        not isinstance(snapshot["snapshot_version"], str)
        or not snapshot["snapshot_version"]
    ):
        raise ContractError("repository snapshot snapshot_version must be non-empty")
    repositories = snapshot["repositories"]
    if not isinstance(repositories, dict) or not repositories:
        raise ContractError("repository snapshot repositories cannot be empty")
    if snapshot["owner_repository"] not in repositories:
        raise ContractError("repository snapshot does not contain its owner")
    for name, revision in repositories.items():
        if not isinstance(revision, dict):
            raise ContractError(f"repository snapshot {name} must be an object")
        _require(revision, ("revision", "dirty"), f"repository snapshot {name}")
        if not HEX_40.fullmatch(revision["revision"]):
            raise ContractError(
                f"repository snapshot {name}.revision is not a full SHA"
            )
        if not isinstance(revision["dirty"], bool):
            raise ContractError(f"repository snapshot {name}.dirty must be boolean")
    if canonical_sha256(repositories) != snapshot["snapshot_hash"]:
        raise ContractError("repository snapshot hash does not match repositories")


def validate_baseline_approval(
    approval: Any, report_bytes: bytes, report: dict[str, Any]
) -> None:
    if not isinstance(approval, dict):
        raise ContractError("baseline approval must be an object")
    _require(
        approval,
        (
            "schema_version",
            "report_sha256",
            "revision_lock_hash",
            "environment_id",
            "approved_by",
            "approved_at",
            "reason",
        ),
        "baseline approval",
    )
    if approval["schema_version"] != "mutsuki.performance.baseline-approval/v1":
        raise ContractError("unsupported baseline approval schema_version")
    if hashlib.sha256(report_bytes).hexdigest() != approval["report_sha256"]:
        raise ContractError(
            "baseline approval report_sha256 does not match report bytes"
        )
    if approval["revision_lock_hash"] != report["revision_lock_hash"]:
        raise ContractError(
            "baseline approval revision_lock_hash does not match report"
        )
    if approval["environment_id"] != report["environment_id"]:
        raise ContractError("baseline approval environment_id does not match report")
    if any(revision["dirty"] for revision in report["repository_revisions"].values()):
        raise ContractError("a dirty repository report cannot be an approved baseline")
    if not approval["approved_by"] or not approval["reason"]:
        raise ContractError("baseline approval approver and reason must be non-empty")
    try:
        parsed = dt.datetime.fromisoformat(
            approval["approved_at"].replace("Z", "+00:00")
        )
    except (AttributeError, ValueError) as error:
        raise ContractError("baseline approval approved_at is not ISO-8601") from error
    if parsed.tzinfo is None:
        raise ContractError("baseline approval approved_at must include a timezone")
