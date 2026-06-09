from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from types import ModuleType
from typing import Any

from mutsuki_runtime_python.contracts import (
    ERR_RUNTIME_BACKEND_FAILED,
    Envelope,
    SourceRef,
    StrategyResultStatus,
    to_json_dict,
)
from mutsuki_runtime_python.stdio import StdioJsonlBackendServer


def _load_bridge() -> ModuleType:
    repo_root = Path(__file__).resolve().parents[3]
    bridge_path = (
        repo_root
        / ".agents"
        / "plugins"
        / "plugins"
        / "mutsuki-codex-core"
        / "scripts"
        / "mutsuki_codex_strategy_backend.py"
    )
    spec = importlib.util.spec_from_file_location("mutsuki_codex_strategy_backend", bridge_path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class StubCodexRunner:
    def __init__(self, output: str) -> None:
        self.output = output
        self.prompts: list[str] = []

    async def run_decision(self, prompt: str) -> str:
        self.prompts.append(prompt)
        return self.output


def _envelope() -> Envelope:
    return Envelope(
        id="env-1",
        timestamp=1.0,
        source=SourceRef(source_id="codex:local", kind="codex.strategy"),
        payload_schema_id="codex.input",
        payload={"prompt": "decide"},
    )


async def test_codex_strategy_backend_on_input_returns_strategy_result() -> None:
    bridge = _load_bridge()
    runner = StubCodexRunner(
        '{"status":"continue","decision":{"operation":"test.echo","payload":{"value":"ok"}}}'
    )
    strategy = bridge.CodexStrategyBackend(runner)

    await strategy.on_awake("agent-a")
    result = await strategy.on_input("agent-a", _envelope())

    assert result.status == StrategyResultStatus.CONTINUE
    assert result.decision == {"operation": "test.echo", "payload": {"value": "ok"}}
    assert runner.prompts
    assert '"agent_id":"agent-a"' in runner.prompts[0]
    assert '"phase":"on_input"' in runner.prompts[0]


async def test_codex_strategy_backend_invalid_json_returns_structured_failure() -> None:
    bridge = _load_bridge()
    strategy = bridge.CodexStrategyBackend(StubCodexRunner("not json"))

    result = await strategy.on_input("agent-a", _envelope())

    assert result.status == StrategyResultStatus.FAILED
    assert result.error is not None
    assert result.error.code == ERR_RUNTIME_BACKEND_FAILED
    assert result.error.source == "mutsuki-codex-core"
    assert result.error.route == "codex.output.decode"


async def test_codex_strategy_backend_failed_status_without_error_gets_structured_error() -> None:
    bridge = _load_bridge()
    strategy = bridge.CodexStrategyBackend(StubCodexRunner('{"status":"failed"}'))

    result = await strategy.on_input("agent-a", _envelope())

    assert result.status == StrategyResultStatus.FAILED
    assert result.error is not None
    assert result.error.code == ERR_RUNTIME_BACKEND_FAILED
    assert result.error.route == "codex.output.failed"
    assert result.error.evidence["reason"] == "failed_status_missing_error"


async def test_codex_strategy_backend_lifecycle_runs_through_stdio_server() -> None:
    bridge = _load_bridge()
    host = bridge.build_backend_host(["agent-a"], StubCodexRunner('{"status":"wait_input"}'))
    server = StdioJsonlBackendServer(host)

    awake = await server.handle_request(
        {"id": "req-1", "method": "on_awake", "params": {"agent_id": "agent-a"}}
    )
    input_result = await server.handle_request(
        {
            "id": "req-2",
            "method": "on_input",
            "params": {"agent_id": "agent-a", "envelope": to_json_dict(_envelope())},
        }
    )
    next_step = await server.handle_request(
        {"id": "req-3", "method": "next_step", "params": {"agent_id": "agent-a"}}
    )
    stop = await server.handle_request(
        {"id": "req-4", "method": "on_stop", "params": {"agent_id": "agent-a"}}
    )

    assert awake == {"id": "req-1", "ok": True, "result": None}
    assert input_result["ok"] is True
    assert _as_dict(input_result["result"])["status"] == StrategyResultStatus.WAIT_INPUT.value
    assert next_step["ok"] is True
    assert _as_dict(next_step["result"])["status"] == StrategyResultStatus.WAIT_INPUT.value
    assert stop == {"id": "req-4", "ok": True, "result": None}


async def test_codex_bridge_registers_codex_source_not_operation() -> None:
    bridge = _load_bridge()
    host = bridge.build_backend_host(["agent-a"], StubCodexRunner('{"status":"wait_input"}'))

    sources = host.list_sources("agent-a")
    operations = host.list_operations("agent-a")

    assert len(sources) == 1
    assert sources[0].descriptor.source_id == "codex:local"
    assert sources[0].plugin_id == "mutsuki-codex-core"
    assert operations == ()


def _as_dict(value: Any) -> dict[str, Any]:
    assert isinstance(value, dict)
    return value
