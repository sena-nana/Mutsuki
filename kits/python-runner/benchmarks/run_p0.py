from __future__ import annotations

import argparse
import io
import os
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--baseline-ref", default="d4ad12fb0a0a584391421568f32c037750a93ea6"
    )
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    benchmark = Path(__file__).with_name("wire_benchmark.py")

    archive = subprocess.run(
        ["git", "archive", args.baseline_ref], check=True, capture_output=True
    ).stdout
    with tempfile.TemporaryDirectory(prefix="mutsuki-python-p0-") as temp:
        baseline_root = Path(temp) / "baseline"
        baseline_root.mkdir()
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r:") as bundle:
            bundle.extractall(baseline_root, filter="data")
        baseline_report = Path(temp) / "baseline.json"
        environment = os.environ.copy()
        environment["PYTHONPATH"] = str(baseline_root / "src")
        subprocess.run(
            [sys.executable, str(benchmark), "--output", str(baseline_report)],
            check=True,
            env=environment,
        )
        subprocess.run(
            [
                sys.executable,
                str(benchmark),
                "--baseline",
                str(baseline_report),
                "--output",
                str(args.output),
            ],
            check=True,
        )


if __name__ == "__main__":
    main()
