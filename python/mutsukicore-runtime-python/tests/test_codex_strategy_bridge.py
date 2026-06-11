from __future__ import annotations

import importlib.util
import io
import json
import sys
from pathlib import Path
from types import ModuleType
from typing import Any

from mutsukicore_runtime_python.backend import BackendInvokeError
from mutsukicore_runtime_python.contracts import (
    ERR_RUNTIME_BACKEND_FAILED,
    Envelope,
    OperationDescriptor,
    RuntimeError,
    SourceRef,
    StrategyResultStatus,
    to_json_dict,
)
from mutsukicore_runtime_python.stdio import StdioJsonlBackendServer


def _load_bridge() -> ModuleType:
    repo_root = Path(__file__).resolve().parents[3]
    bridge_path = (
        repo_root
        / ".agents"
        / "plugins"
        / "plugins"
        / "mutsukicore-codex-core"
        / "scripts"
        / "mutsukicore_codex_strategy_backend.py"
    )
    spec = importlib.util.spec_from_file_location("mutsukicore_codex_strategy_backend", bridge_path)
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


class FailingCodexRunner:
    def __init__(self, exc: Exception) -> None:
        self.exc = exc

    async def run_decision(self, prompt: str) -> str:
        _ = prompt
        raise self.exc


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


async def test_codex_strategy_backend_extracts_json_from_surrounding_text() -> None:
    bridge = _load_bridge()
    strategy = bridge.CodexStrategyBackend(StubCodexRunner('thinking\n{"status":"wait_input"}\n'))

    result = await strategy.on_input("agent-a", _envelope())

    assert result.status == StrategyResultStatus.WAIT_INPUT
    assert result.error is None


async def test_codex_strategy_backend_empty_output_returns_structured_failure() -> None:
    bridge = _load_bridge()
    strategy = bridge.CodexStrategyBackend(StubCodexRunner(""))

    result = await strategy.on_input("agent-a", _envelope())

    assert result.status == StrategyResultStatus.FAILED
    assert result.error is not None
    assert result.error.route == "codex.output.decode"
    assert result.error.evidence["reason"] == "empty_output"


async def test_codex_strategy_backend_missing_or_invalid_status_fails_loud() -> None:
    bridge = _load_bridge()
    missing_status = bridge.CodexStrategyBackend(StubCodexRunner("{}"))
    invalid_status = bridge.CodexStrategyBackend(StubCodexRunner('{"status":"completed"}'))

    missing_result = await missing_status.on_input("agent-a", _envelope())
    invalid_result = await invalid_status.on_input("agent-a", _envelope())

    assert missing_result.status == StrategyResultStatus.FAILED
    assert missing_result.error is not None
    assert missing_result.error.route == "codex.output.status"
    assert invalid_result.status == StrategyResultStatus.FAILED
    assert invalid_result.error is not None
    assert invalid_result.error.route == "codex.output.status"


async def test_codex_strategy_backend_runner_exception_is_structured_failure() -> None:
    bridge = _load_bridge()
    strategy = bridge.CodexStrategyBackend(FailingCodexRunner(ValueError("boom")))

    result = await strategy.on_input("agent-a", _envelope())

    assert result.status == StrategyResultStatus.FAILED
    assert result.error is not None
    assert result.error.code == ERR_RUNTIME_BACKEND_FAILED
    assert result.error.route == "codex.runner"
    assert result.error.evidence["exception_type"] == "ValueError"


async def test_codex_strategy_backend_backend_invoke_error_is_preserved() -> None:
    bridge = _load_bridge()
    error = RuntimeError(
        code=ERR_RUNTIME_BACKEND_FAILED,
        source="codex-test-runner",
        route="codex.exec",
        evidence={"exit_code": 7},
    )
    strategy = bridge.CodexStrategyBackend(FailingCodexRunner(BackendInvokeError(error)))

    result = await strategy.on_input("agent-a", _envelope())

    assert result.status == StrategyResultStatus.FAILED
    assert result.error == error


async def test_codex_strategy_backend_next_step_prompt_and_session_cleanup() -> None:
    bridge = _load_bridge()
    runner = StubCodexRunner('{"status":"wait_input"}')
    strategy = bridge.CodexStrategyBackend(runner)

    await strategy.on_awake("agent-a")
    await strategy.on_input("agent-a", _envelope())
    await strategy.next_step("agent-a")

    assert len(runner.prompts) == 2
    assert '"phase":"next_step"' in runner.prompts[1]
    assert '"history":[{"event":"input"' in runner.prompts[1]
    assert "agent-a" in strategy.sessions

    await strategy.on_stop("agent-a")

    assert "agent-a" not in strategy.sessions


async def test_codex_strategy_backend_invalid_json_returns_structured_failure() -> None:
    bridge = _load_bridge()
    strategy = bridge.CodexStrategyBackend(StubCodexRunner("not json"))

    result = await strategy.on_input("agent-a", _envelope())

    assert result.status == StrategyResultStatus.FAILED
    assert result.error is not None
    assert result.error.code == ERR_RUNTIME_BACKEND_FAILED
    assert result.error.source == "mutsukicore-codex-core"
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
    assert sources[0].plugin_id == "mutsukicore-codex-core"
    assert operations == ()


async def test_codex_bridge_registers_multiple_agents_and_prompt_operations() -> None:
    bridge = _load_bridge()
    operation = bridge.PythonBackendHost().register_operation(
        OperationDescriptor(op_id="test.echo", name="echo", plugin_id="test"),
        lambda payload: payload,
    )
    runner = StubCodexRunner('{"status":"wait_input"}')
    host = bridge.build_backend_host(["agent-a", "agent-b"], runner, (operation,))

    await host.on_awake("agent-a")
    await host.on_awake("agent-b")
    await host.next_step("agent-b")

    assert host.awake_count("agent-a") == 1
    assert host.awake_count("agent-b") == 1
    assert host.list_sources("agent-a")[0].descriptor.source_id == "codex:local"
    assert host.list_operations("agent-a") == ()
    assert '"op_id":"test.echo"' in runner.prompts[0]


def _as_dict(value: Any) -> dict[str, Any]:
    assert isinstance(value, dict)
    return value


async def test_codex_bridge_load_operation_snapshot_list_from_json() -> None:
    """_load_operation_snapshot_list parses valid JSON input."""
    bridge = _load_bridge()
    raw = [
        {
            "descriptor": {
                "op_id": "test.echo",
                "name": "echo",
                "description": "",
                "plugin_id": "test",
                "func_qualname": "",
                "parameters_schema": {},
                "return_schema": {},
                "perms_rule_id": None,
                "requires_capabilities": [],
                "is_tool": True,
            },
            "status": "active",
            "key": {
                "plugin_id": "test",
                "plugin_generation": 0,
                "op_id": "test.echo",
                "handler_id": "test:test.echo:0",
            },
        }
    ]
    snapshots = bridge._load_operation_snapshot_list(raw)
    assert len(snapshots) == 1
    assert snapshots[0].descriptor.op_id == "test.echo"
    assert snapshots[0].descriptor.plugin_id == "test"
    assert snapshots[0].key.plugin_generation == 0


async def test_codex_bridge_load_operation_snapshot_list_invalid_type() -> None:
    """_load_operation_snapshot_list raises TypeError for non-list input."""
    bridge = _load_bridge()
    try:
        bridge._load_operation_snapshot_list({"not": "a list"})  # type: ignore[arg-type]
        assert False, "expected TypeError"
    except TypeError:
        pass




async def test_codex_bridge_cli_stdin_loads_operations_and_passes_to_prompt() -> None:
    """--operation-snapshots-stdin reads first stdin line as ops JSON."""
    bridge = _load_bridge()
    first_line = json.dumps([
        {
            "descriptor": {
                "op_id": "stdin.echo",
                "name": "stdin-echo",
                "description": "From stdin",
                "plugin_id": "stdin-test",
                "func_qualname": "",
                "parameters_schema": {},
                "return_schema": {},
                "perms_rule_id": None,
                "requires_capabilities": [],
                "is_tool": True,
            },
            "status": "active",
            "key": {
                "plugin_id": "stdin-test",
                "plugin_generation": 0,
                "op_id": "stdin.echo",
                "handler_id": "stdin-test:stdin.echo:0",
            },
        }
    ])

    # Simulate stdin: first line = ops JSON, then JSONL request
    saved_stdin = sys.stdin
    try:
        sys.stdin = io.StringIO(first_line + "\n")
        runner = StubCodexRunner('{"status":"wait_input"}')
        host = bridge.build_backend_host(
            ["agent-a"],
            runner,
            bridge._load_operation_snapshot_list(json.loads(first_line)),
        )
        await host.on_awake("agent-a")
        await host.on_input("agent-a", _envelope())
        assert '"op_id":"stdin.echo"' in runner.prompts[0]
        assert '"plugin_id":"stdin-test"' in runner.prompts[0]
        assert '"description":"From stdin"' in runner.prompts[0]
    finally:
        sys.stdin = saved_stdin
