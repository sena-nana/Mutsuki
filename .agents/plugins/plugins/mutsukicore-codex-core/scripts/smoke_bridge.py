from __future__ import annotations

import asyncio
import sys
from pathlib import Path
from typing import Any

for parent in Path(__file__).resolve().parents:
    package_src = parent / "python" / "mutsukicore-runtime-python" / "src"
    if package_src.is_dir():
        sys.path.insert(0, str(package_src))
        break

sys.path.insert(0, str(Path(__file__).resolve().parent))

from mutsukicore_runtime_python.contracts import StrategyResultStatus
from mutsukicore_runtime_python.stdio import StdioJsonlBackendServer

from mutsukicore_codex_strategy_backend import PLUGIN_ID, build_backend_host


class StubCodexRunner:
    async def run_decision(self, prompt: str) -> str:
        _ = prompt
        return '{"status":"wait_input"}'


async def _smoke() -> None:
    host = build_backend_host(["agent-a"], StubCodexRunner())
    server = StdioJsonlBackendServer(host)
    source_response = await server.handle_request(
        {
            "id": "req-1",
            "method": "list_sources",
            "params": {"enabled_plugin_ids": [PLUGIN_ID]},
        }
    )
    input_response = await server.handle_request(
        {
            "id": "req-2",
            "method": "on_input",
            "params": {
                "agent_id": "agent-a",
                "envelope": {
                    "id": "env-1",
                    "timestamp": 1.0,
                    "source": {
                        "source_id": "codex:local",
                        "kind": "codex.strategy",
                        "metadata": {},
                    },
                    "payload_schema_id": "codex.input",
                    "capabilities_required": [],
                    "payload": {"prompt": "hello"},
                },
            },
        }
    )
    assert source_response["ok"] is True
    result = _as_dict(input_response["result"])
    assert result["status"] == StrategyResultStatus.WAIT_INPUT.value


def _as_dict(value: Any) -> dict[str, Any]:
    assert isinstance(value, dict)
    return value


def main() -> int:
    asyncio.run(_smoke())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
