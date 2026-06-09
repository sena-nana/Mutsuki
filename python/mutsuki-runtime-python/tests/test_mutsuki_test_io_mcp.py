from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path
from types import ModuleType
from typing import Any


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[3]


def _load_test_io() -> ModuleType:
    script = (
        _repo_root()
        / ".agents"
        / "plugins"
        / "plugins"
        / "mutsuki-test-io"
        / "scripts"
        / "mutsuki_test_io_mcp.py"
    )
    spec = importlib.util.spec_from_file_location("mutsuki_test_io_mcp", script)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


async def _request(
    server: Any,
    request_id: int,
    method: str,
    params: dict[str, Any],
) -> dict[str, Any]:
    response = await server.handle_request(
        {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
    )
    assert isinstance(response, dict)
    assert response["id"] == request_id
    return response


async def _tool_call(
    server: Any,
    request_id: int,
    name: str,
    arguments: dict[str, Any],
) -> dict[str, Any]:
    return await _tool_result(server, request_id, name, arguments, is_error=False)


async def _tool_error(
    server: Any,
    request_id: int,
    name: str,
    arguments: dict[str, Any],
) -> dict[str, Any]:
    return await _tool_result(server, request_id, name, arguments, is_error=True)


async def _tool_result(
    server: Any,
    request_id: int,
    name: str,
    arguments: dict[str, Any],
    *,
    is_error: bool,
) -> dict[str, Any]:
    response = await _request(
        server,
        request_id,
        "tools/call",
        {"name": name, "arguments": arguments},
    )
    result = response["result"]
    assert isinstance(result, dict)
    assert result.get("isError") is is_error, result
    content = result["content"]
    assert isinstance(content, list)
    text = content[0]["text"]
    assert isinstance(text, str)
    payload = json.loads(text)
    assert isinstance(payload, dict)
    return payload


async def test_mutsuki_test_io_initialize_and_tool_list() -> None:
    module = _load_test_io()
    server = module.TestIoServer()

    initialize = await _request(server, 1, "initialize", {})
    tools = await _request(server, 2, "tools/list", {})

    assert initialize["result"]["serverInfo"]["name"] == "mutsuki-test-io"
    tool_names = {item["name"] for item in tools["result"]["tools"]}
    assert {
        "run_command",
        "start_process",
        "write_stdin",
        "read_output",
        "stop_process",
        "jsonl_request",
    } <= tool_names


async def test_mutsuki_test_io_run_command_and_truncation() -> None:
    module = _load_test_io()
    server = module.TestIoServer()

    echo = await _tool_call(
        server,
        1,
        "run_command",
        {
            "command": [sys.executable, "-c", "print('mutsuki-test-io-ok')"],
            "timeout_ms": 5000,
        },
    )
    truncated = await _tool_call(
        server,
        2,
        "run_command",
        {
            "command": [sys.executable, "-c", "import sys; sys.stdout.write('0123456789')"],
            "timeout_ms": 5000,
            "max_bytes": 4,
        },
    )

    assert echo["exit_code"] == 0
    assert "mutsuki-test-io-ok" in echo["stdout"]
    assert truncated["stdout"] == "0123"
    assert truncated["stdout_truncated"] is True
    assert truncated["stdout_total_bytes"] == 10


async def test_mutsuki_test_io_interactive_process_io_and_stop_cleanup() -> None:
    module = _load_test_io()
    server = module.TestIoServer()

    started = await _tool_call(
        server,
        1,
        "start_process",
        {
            "command": [
                sys.executable,
                "-u",
                "-c",
                "import sys\nfor line in sys.stdin:\n print('reply:'+line.strip(), flush=True)",
            ]
        },
    )
    session_id = started["session_id"]

    await _tool_call(server, 2, "write_stdin", {"session_id": session_id, "text": "hello\n"})
    output = await _tool_call(
        server,
        3,
        "read_output",
        {
            "session_id": session_id,
            "until_stdout_contains": "reply:hello",
            "timeout_ms": 5000,
        },
    )
    stopped = await _tool_call(server, 4, "stop_process", {"session_id": session_id})

    assert "reply:hello" in output["stdout"]
    assert stopped["session_id"] == session_id
    assert session_id not in server.sessions


async def test_mutsuki_test_io_rejects_cwd_outside_repo() -> None:
    module = _load_test_io()
    server = module.TestIoServer()
    outside = str(_repo_root().parent)

    error = await _tool_error(
        server,
        1,
        "run_command",
        {"command": [sys.executable, "-c", "print('nope')"], "cwd": outside},
    )

    assert error["error"] == "cwd_outside_repo"
    assert error["repo_root"] == str(_repo_root())


async def test_mutsuki_test_io_jsonl_request_matches_id_and_ignores_other_lines() -> None:
    module = _load_test_io()
    server = module.TestIoServer()

    started = await _tool_call(
        server,
        1,
        "start_process",
        {
            "command": [
                sys.executable,
                "-u",
                "-c",
                (
                    "import json,sys\n"
                    "for line in sys.stdin:\n"
                    " req=json.loads(line)\n"
                    " print(json.dumps({'id':'other','ok':True,'result':None}), flush=True)\n"
                    " print(json.dumps({'id':req['id'],'ok':True,'result':{'seen':req['method']}}),"
                    " flush=True)\n"
                ),
            ]
        },
    )
    session_id = started["session_id"]

    response = await _tool_call(
        server,
        2,
        "jsonl_request",
        {
            "session_id": session_id,
            "request": {"id": "req-1", "method": "ping", "params": {}},
            "timeout_ms": 5000,
        },
    )
    await _tool_call(server, 3, "stop_process", {"session_id": session_id})

    assert response["response"]["id"] == "req-1"
    assert response["response"]["result"] == {"seen": "ping"}
    assert session_id not in server.sessions


async def test_mutsuki_test_io_jsonl_timeout_reports_evidence_after_bad_json() -> None:
    module = _load_test_io()
    server = module.TestIoServer()

    started = await _tool_call(
        server,
        1,
        "start_process",
        {
            "command": [
                sys.executable,
                "-u",
                "-c",
                "import sys,time\nprint('not json', flush=True)\ntime.sleep(1)",
            ]
        },
    )
    session_id = started["session_id"]

    error = await _tool_error(
        server,
        2,
        "jsonl_request",
        {
            "session_id": session_id,
            "request": {"id": "req-1", "method": "ping", "params": {}},
            "timeout_ms": 200,
        },
    )
    await _tool_call(server, 3, "stop_process", {"session_id": session_id})

    assert error["error"] == "jsonl_response_timeout"
    assert error["request_id"] == "req-1"
    assert "exit_code" in error
    assert session_id not in server.sessions


async def test_mutsuki_test_io_drives_codex_backend_stdio_lifecycle() -> None:
    module = _load_test_io()
    server = module.TestIoServer()
    codex_backend = (
        _repo_root()
        / ".agents"
        / "plugins"
        / "plugins"
        / "mutsuki-codex-core"
        / "scripts"
        / "mutsuki_codex_strategy_backend.py"
    )
    envelope = {
        "id": "env-1",
        "timestamp": 1.0,
        "source": {"source_id": "codex:local", "kind": "codex.strategy", "metadata": {}},
        "payload_schema_id": "codex.input",
        "capabilities_required": [],
        "payload": {"prompt": "hello"},
    }

    started = await _tool_call(
        server,
        1,
        "start_process",
        {
            "command": [
                sys.executable,
                "-u",
                str(codex_backend),
                "--agent-id",
                "agent-a",
                "--stub-output",
                '{"status":"wait_input"}',
            ],
            "cwd": "python/mutsuki-runtime-python",
        },
    )
    session_id = started["session_id"]

    try:
        sources = await _tool_call(
            server,
            2,
            "jsonl_request",
            {
                "session_id": session_id,
                "request": {
                    "id": "req-1",
                    "method": "list_sources",
                    "params": {"agent_id": "agent-a"},
                },
                "timeout_ms": 5000,
            },
        )
        awake = await _tool_call(
            server,
            3,
            "jsonl_request",
            {
                "session_id": session_id,
                "request": {
                    "id": "req-2",
                    "method": "on_awake",
                    "params": {"agent_id": "agent-a"},
                },
                "timeout_ms": 5000,
            },
        )
        input_result = await _tool_call(
            server,
            4,
            "jsonl_request",
            {
                "session_id": session_id,
                "request": {
                    "id": "req-3",
                    "method": "on_input",
                    "params": {"agent_id": "agent-a", "envelope": envelope},
                },
                "timeout_ms": 15000,
            },
        )
        next_step = await _tool_call(
            server,
            5,
            "jsonl_request",
            {
                "session_id": session_id,
                "request": {
                    "id": "req-4",
                    "method": "next_step",
                    "params": {"agent_id": "agent-a"},
                },
                "timeout_ms": 15000,
            },
        )
        stop = await _tool_call(
            server,
            6,
            "jsonl_request",
            {
                "session_id": session_id,
                "request": {
                    "id": "req-5",
                    "method": "on_stop",
                    "params": {"agent_id": "agent-a"},
                },
                "timeout_ms": 5000,
            },
        )
    finally:
        if session_id in server.sessions:
            await _tool_call(server, 7, "stop_process", {"session_id": session_id})

    assert sources["response"]["ok"] is True
    assert sources["response"]["result"][0]["descriptor"]["source_id"] == "codex:local"
    assert awake["response"] == {"id": "req-2", "ok": True, "result": None}
    assert input_result["response"]["ok"] is True
    assert input_result["response"]["result"]["status"] == "wait_input"
    assert next_step["response"]["ok"] is True
    assert next_step["response"]["result"]["status"] == "wait_input"
    assert stop["response"] == {"id": "req-5", "ok": True, "result": None}
    assert session_id not in server.sessions
