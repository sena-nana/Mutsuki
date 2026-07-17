#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

import _bootstrap  # noqa: F401
from mutsuki_performance import validate_report


ROOT = Path(__file__).resolve().parents[2]
EXPECTED_DEPLOYMENTS = {
    "builtin",
    "abi",
    "rust-process-jsonl",
    "python-jsonl",
    "python-binary",
}


def load(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def report_arguments(values: list[str]) -> dict[str, Path]:
    reports: dict[str, Path] = {}
    for value in values:
        suite_id, separator, path = value.partition("=")
        if not separator or not suite_id or not path:
            raise SystemExit("--report expects SUITE_ID=PATH")
        if suite_id in reports:
            raise SystemExit(f"duplicate report argument for {suite_id}")
        reports[suite_id] = Path(path)
    return reports


def expected_fixture_hashes(manifest: dict[str, Any]) -> dict[str, str]:
    if manifest.get("schema_version") != "mutsuki.performance.runner-fixtures/v1":
        raise SystemExit("unsupported Runner fixture manifest")
    result: dict[str, str] = {}
    for fixture in manifest.get("fixtures", []):
        protocol_id = fixture.get("protocol_id")
        output_hash = fixture.get("output_sha256")
        if not isinstance(protocol_id, str) or not isinstance(output_hash, str):
            raise SystemExit(
                "Runner fixture entries require protocol_id and output_sha256"
            )
        if protocol_id in result:
            raise SystemExit(f"duplicate Runner fixture {protocol_id}")
        result[protocol_id] = output_hash
    if len(result) != 6:
        raise SystemExit("Runner fixture manifest must contain exactly six fixtures")
    return result


def validate_fixture_matrix(
    reports: dict[str, dict[str, Any]], expected: dict[str, str]
) -> None:
    matrix: dict[str, dict[str, str]] = {}
    for suite_id in ("service-host", "python-runner-kit"):
        hashes = reports[suite_id].get("metadata", {}).get("fixture_output_hashes", {})
        if not isinstance(hashes, dict):
            raise SystemExit(f"{suite_id} is missing fixture_output_hashes")
        for key, value in hashes.items():
            deployment, separator, protocol_id = key.partition(":")
            if not separator:
                raise SystemExit(f"invalid fixture hash key in {suite_id}: {key}")
            matrix.setdefault(deployment, {})[protocol_id] = value
    if set(matrix) != EXPECTED_DEPLOYMENTS:
        raise SystemExit(
            "Runner fixture deployments mismatch: "
            f"expected {sorted(EXPECTED_DEPLOYMENTS)}, got {sorted(matrix)}"
        )
    for deployment, actual in sorted(matrix.items()):
        if actual != expected:
            raise SystemExit(f"Runner fixture hashes mismatch for {deployment}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--report", action="append", default=[], metavar="SUITE_ID=PATH"
    )
    parser.add_argument("--fixture-manifest", required=True, type=Path)
    parser.add_argument("--require-clean", action="store_true")
    args = parser.parse_args()

    catalog = load(ROOT / "performance/catalog/suites-v1.json")
    suites = {suite["suite_id"]: suite for suite in catalog["suites"]}
    paths = report_arguments(args.report)
    if set(paths) != set(suites):
        missing = sorted(set(suites) - set(paths))
        extra = sorted(set(paths) - set(suites))
        raise SystemExit(f"report set mismatch: missing={missing}, extra={extra}")

    reports: dict[str, dict[str, Any]] = {}
    total_cases = 0
    for suite_id, suite in sorted(suites.items()):
        report = load(paths[suite_id])
        validate_report(report)
        if report["suite_version"] != suite["suite_version"]:
            raise SystemExit(f"{suite_id} suite_version mismatch")
        owner = report["repository_revisions"].get(suite["repository"])
        if owner is None:
            raise SystemExit(f"{suite_id} does not record owner {suite['repository']}")
        if "MutsukiBenchmarks" in report["repository_revisions"]:
            raise SystemExit(
                f"{suite_id} still depends on the retired central repository"
            )
        if args.require_clean and any(
            revision["dirty"] for revision in report["repository_revisions"].values()
        ):
            raise SystemExit(
                f"{suite_id} is provisional because a recorded repository is dirty"
            )
        if not report["correctness"]["passed"]:
            raise SystemExit(f"{suite_id} correctness failed")
        if len(report["cases"]) < suite["minimum_cases"]:
            raise SystemExit(
                f"{suite_id} has {len(report['cases'])} cases; "
                f"expected at least {suite['minimum_cases']}"
            )
        reports[suite_id] = report
        total_cases += len(report["cases"])

    validate_fixture_matrix(
        reports, expected_fixture_hashes(load(args.fixture_manifest))
    )
    print(
        f"valid Epic #35 owner reports: {len(reports)} reports, {total_cases} cases, "
        f"clean_required={str(args.require_clean).lower()}"
    )


if __name__ == "__main__":
    main()
