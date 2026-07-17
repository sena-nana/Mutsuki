from __future__ import annotations

import argparse
import json
from pathlib import Path

import _bootstrap  # noqa: F401
from mutsuki_performance import validate_report


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("report", type=Path)
    args = parser.parse_args()
    report = json.loads(args.report.read_text(encoding="utf-8"))
    validate_report(report)
    print(f"valid report: {report['report_id']} ({len(report['cases'])} cases)")


if __name__ == "__main__":
    main()
