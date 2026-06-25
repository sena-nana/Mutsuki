from __future__ import annotations

import asyncio
import sys
from pathlib import Path
from typing import Any

for parent in Path(__file__).resolve().parents:
    package_src = parent / "python" / "mutsuki-runtime-python" / "src"
    if package_src.is_dir():
        sys.path.insert(0, str(package_src))
        break

sys.path.insert(0, str(Path(__file__).resolve().parent))

from mutsuki_runtime_python.contracts.codec import to_json_dict
from mutsuki_runtime_python.contracts.runner import RunnerContext
from mutsuki_runtime_python.contracts.task import Task
from mutsuki_runtime_python.transport.stdio_jsonl import StdioJsonlRunnerServer

from mutsuki_codex_runner import RUNNER_ID, TASK_KIND, build_runner_host


class StubCodexRunner:
    async def run_decision(self, _prompt: str) -> str:
        return '{"status":"wait_input"}'


async def _smoke() -> None:
    host = build_runner_host(StubCodexRunner())
    server = StdioJsonlRunnerServer(host)
    response = await server.handle_request(
        {
            "id": "req-1",
            "method": "runner.step",
            "params": {
                "runner_id": RUNNER_ID,
                "ctx": to_json_dict(RunnerContext(registry_generation=1, current_step=1)),
                "tasks": [
                    to_json_dict(
                        Task.new(
                            "task-1",
                            TASK_KIND,
                            {"prompt": "hello", "agent_id": "agent-a", "phase": "on_input"},
                        )
                    )
                ],
            },
        }
    )
    assert response["ok"] is True
    results = _as_list(response["result"])
    result = _as_dict(results[0])
    event = _as_dict(_as_list(result["events"])[0])
    assert event["kind"] == "codex.strategy.result"
    assert _as_dict(event["payload"])["status"] == "wait_input"


def _as_dict(value: Any) -> dict[str, Any]:
    assert isinstance(value, dict)
    return value


def _as_list(value: Any) -> list[Any]:
    assert isinstance(value, list)
    return value


def main() -> int:
    asyncio.run(_smoke())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
