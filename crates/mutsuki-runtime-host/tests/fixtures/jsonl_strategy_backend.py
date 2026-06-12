from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

for parent in Path(__file__).resolve().parents:
    package_src = parent / "python" / "mutsuki-runtime-python" / "src"
    if package_src.is_dir():
        sys.path.insert(0, str(package_src))
        break

from mutsuki_runtime_python.backend import StrategyBackend
from mutsuki_runtime_python.contracts import (
    Envelope,
    RuntimeError,
    SourceDescriptor,
    SourceSnapshot,
    StrategyResult,
    StrategyResultStatus,
)
from mutsuki_runtime_python.host import PythonBackendHost
from mutsuki_runtime_python.stdio import run_stdio_server

PLUGIN_ID = "test-strategy-plugin"
SOURCE_ID = "source:strategy-test"
SOURCE_KIND = "test.strategy"


class StubStrategy(StrategyBackend):
    def __init__(self, output: dict[str, object]) -> None:
        self._output = output

    async def on_awake(self, agent_id: str) -> None:
        _ = agent_id

    async def on_input(self, agent_id: str, envelope: Envelope) -> StrategyResult:
        _ = (agent_id, envelope)
        return self._strategy_result()

    async def next_step(self, agent_id: str) -> StrategyResult:
        _ = agent_id
        return self._strategy_result()

    async def on_stop(self, agent_id: str) -> None:
        _ = agent_id

    def _strategy_result(self) -> StrategyResult:
        status = StrategyResultStatus(str(self._output["status"]))
        error_data = self._output.get("error")
        return StrategyResult(
            status=status,
            decision=self._output.get("decision"),
            emitted=(),
            error=RuntimeError.from_json_dict(error_data) if error_data is not None else None,
        )


def build_host(agent_id: str, output: dict[str, object]) -> PythonBackendHost:
    host = PythonBackendHost()
    host.register_agent(agent_id, StubStrategy(output))
    host.register_source(
        SourceSnapshot(
            descriptor=SourceDescriptor(
                source_id=SOURCE_ID,
                kind=SOURCE_KIND,
                capabilities=("strategy",),
                description="JSONL strategy fixture source",
            ),
            plugin_id=PLUGIN_ID,
            plugin_generation=0,
        )
    )
    return host


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--agent-id", required=True)
    parser.add_argument("--stub-output", required=True)
    args = parser.parse_args()

    output = json.loads(args.stub_output)
    if not isinstance(output, dict):
        raise TypeError("--stub-output must be a JSON object")
    run_stdio_server(build_host(args.agent_id, output), sys.stdin, sys.stdout)


if __name__ == "__main__":
    main()
