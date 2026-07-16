from __future__ import annotations

import argparse
import asyncio
import platform
import sys
from pathlib import Path

from p1_cases import cancel_case, concurrency_case
from wire_report import repository_state, write_report

from mutsuki_runner_kit.wire.generated import CORE_WIRE_REVISION
from mutsuki_runner_kit.wire.protocol import SCHEMA_REVISION


async def benchmark(mode: str) -> dict[str, object]:
    groups = {"smoke": {1: 8, 16: 3, 56: 2}, "full": {1: 100, 16: 30, 56: 12}}[mode]
    cancel = await cancel_case(10 if mode == "smoke" else 100)
    concurrency = [await concurrency_case(value, groups[value]) for value in (1, 16, 56)]
    by_concurrency = {int(case["concurrency"]): case for case in concurrency}
    single = by_concurrency[1]
    sixteen = by_concurrency[16]
    fifty_six = by_concurrency[56]
    gates = [
        gate("p1.python.cancel.p95", cancel["p95_ns"], 5_000_000, "ns"),
        gate("p1.python.cancel.max", cancel["max_ns"], 20_000_000, "ns"),
        gate(
            "p1.python.concurrent-16.throughput-scaling",
            sixteen["throughput_per_second"],
            float(single["throughput_per_second"]) * 1.2,
            "requests/s_min",
            minimum=True,
        ),
        gate(
            "p1.python.concurrent-56.throughput-non-collapse",
            fifty_six["throughput_per_second"],
            float(sixteen["throughput_per_second"]) * 0.75,
            "requests/s_min",
            minimum=True,
        ),
        gate(
            "p1.python.concurrent-56.peak-bytes-per-request",
            fifty_six["peak_bytes_per_request"],
            float(single["peak_bytes_per_request"]) * 2,
            "bytes/request",
        ),
    ]
    return {
        "issue": 32,
        "phase": "p1",
        "mode": mode,
        "schema_revision": SCHEMA_REVISION,
        "core_revision": CORE_WIRE_REVISION,
        "python": sys.version,
        "platform": platform.platform(),
        "repository": repository_state(),
        "cancel_latency": cancel,
        "concurrency": concurrency,
        "gates": gates,
        "passed": all(bool(item["passed"]) for item in gates),
    }


def gate(
    name: str,
    actual: float,
    limit: float,
    unit: str,
    *,
    minimum: bool = False,
) -> dict[str, object]:
    return {
        "name": name,
        "actual": actual,
        "limit": limit,
        "unit": unit,
        "passed": actual >= limit if minimum else actual <= limit,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=("smoke", "full"))
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    report = asyncio.run(benchmark(args.mode))
    write_report(report, args.output)
    if report["passed"] is not True:
        raise SystemExit("Python Runtime Wire P1 performance gates failed")


if __name__ == "__main__":
    main()
