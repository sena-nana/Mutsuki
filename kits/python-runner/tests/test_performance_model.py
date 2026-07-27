from __future__ import annotations

import runpy
import sys
from collections.abc import Callable
from pathlib import Path
from typing import cast

ROOT = Path(__file__).parents[1]


def load_cpu_time_parser() -> Callable[[str], int]:
    sys.path.insert(0, str(ROOT / "benchmarks"))
    try:
        module = runpy.run_path(str(ROOT / "benchmarks" / "performance_model.py"))
    finally:
        sys.path.pop(0)
    return cast(Callable[[str], int], module["parse_ps_cpu_time"])


def test_parse_ps_cpu_time_accepts_portable_ps_formats() -> None:
    parse_ps_cpu_time = load_cpu_time_parser()

    assert parse_ps_cpu_time("00:01") == 1_000_000_000
    assert parse_ps_cpu_time("01:02:03") == 3_723_000_000_000
    assert parse_ps_cpu_time("2-03:04:05") == 183_845_000_000_000
