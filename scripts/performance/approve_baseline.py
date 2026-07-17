from __future__ import annotations

import argparse
import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path

import _bootstrap  # noqa: F401
from mutsuki_performance import validate_report


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--report", required=True, type=Path)
    parser.add_argument("--approver", required=True)
    parser.add_argument("--reason", required=True)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    report_bytes = args.report.read_bytes()
    report = json.loads(report_bytes)
    validate_report(report)
    if any(revision["dirty"] for revision in report["repository_revisions"].values()):
        raise SystemExit(
            "refusing to approve a report produced from a dirty repository snapshot"
        )
    approval = {
        "schema_version": "mutsuki.performance.baseline-approval/v1",
        "report_sha256": hashlib.sha256(report_bytes).hexdigest(),
        "revision_lock_hash": report["revision_lock_hash"],
        "environment_id": report["environment_id"],
        "approved_by": args.approver,
        "approved_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "reason": args.reason,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(approval, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(f"wrote explicit approval: {args.output}")


if __name__ == "__main__":
    main()
