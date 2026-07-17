from __future__ import annotations

import argparse
import json
from pathlib import Path

import _bootstrap  # noqa: F401
from mutsuki_performance import (
    compare_reports,
    validate_baseline_approval,
    validate_report,
)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("baseline", type=Path)
    parser.add_argument("current", type=Path)
    parser.add_argument("--approval", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    baseline_bytes = args.baseline.read_bytes()
    baseline = json.loads(baseline_bytes)
    current = json.loads(args.current.read_text(encoding="utf-8"))
    validate_report(baseline)
    validate_baseline_approval(
        json.loads(args.approval.read_text(encoding="utf-8")), baseline_bytes, baseline
    )
    comparison = compare_reports(baseline, current)
    rendered = (
        json.dumps(comparison, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    )
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(rendered, encoding="utf-8")
    print(rendered, end="")
    if not comparison["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
