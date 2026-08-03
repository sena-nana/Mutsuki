#!/usr/bin/env python3
from __future__ import annotations

import argparse
import sys

from mutsuki_runner_kit.runners.backend import PythonRunnerBackend
from mutsuki_runner_kit.testing.benchmark_runners import standard_fixture_runners
from mutsuki_runner_kit.transport.stdio_binary import run_stdio_binary_bridge


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--codec", choices=("python-binary",), required=True)
    args = parser.parse_args()
    backend = PythonRunnerBackend()
    for runner in standard_fixture_runners():
        backend.register_runner(runner)
    run_stdio_binary_bridge(backend, sys.stdin.buffer, sys.stdout.buffer)


if __name__ == "__main__":
    main()
