from __future__ import annotations

import asyncio
import json
import sys
from pathlib import Path
from typing import Any

PLUGIN_ROOT = Path(__file__).resolve().parents[1]
SERVER = PLUGIN_ROOT / "scripts" / "mutsuki_test_io_mcp.py"
REPO_ROOT = PLUGIN_ROOT.parents[3]


async def main() -> int:
    process = await asyncio.create_subprocess_exec(
        sys.executable,
        str(SERVER),
        cwd=str(PLUGIN_ROOT),
        stdin=asyncio.subprocess.PIPE,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    assert process.stdin is not None
    assert process.stdout is not None
    try:
        initialize = await request(process, 1, "initialize", {})
        assert initialize["result"]["serverInfo"]["name"] == "mutsuki-test-io"

        tools = await request(process, 2, "tools/list", {})
        tool_names = {item["name"] for item in tools["result"]["tools"]}
        expected_tools = {
            "run_command",
            "start_process",
            "write_stdin",
            "read_output",
            "stop_process",
            "jsonl_request",
        }
        assert expected_tools <= tool_names

        echo = await tool_call(
            process,
            3,
            "run_command",
            {
                "command": [
                    sys.executable,
                    "-c",
                    "print('mutsuki-test-io-ok')",
                ],
                "timeout_ms": 5000,
            },
        )
        assert echo["exit_code"] == 0
        assert "mutsuki-test-io-ok" in echo["stdout"]

        large_output = await tool_call(
            process,
            4,
            "run_command",
            {
                "command": [
                    sys.executable,
                    "-c",
                    "import sys; sys.stdout.write('0123456789')",
                ],
                "max_bytes": 4,
                "timeout_ms": 5000,
            },
        )
        assert large_output["stdout"] == "0123"
        assert large_output["stdout_truncated"] is True
        assert large_output["stdout_total_bytes"] == 10

        interactive = await tool_call(
            process,
            5,
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
        session_id = interactive["session_id"]
        await tool_call(process, 6, "write_stdin", {"session_id": session_id, "text": "hello\n"})
        output = await tool_call(
            process,
            7,
            "read_output",
            {"session_id": session_id, "until_stdout_contains": "reply:hello", "timeout_ms": 5000},
        )
        assert "reply:hello" in output["stdout"]
        await tool_call(process, 8, "stop_process", {"session_id": session_id})

        paged = await tool_call(
            process,
            9,
            "start_process",
            {
                "command": [
                    sys.executable,
                    "-u",
                    "-c",
                    (
                        "import sys,time; "
                        "sys.stdout.write('abcdefghij'); "
                        "sys.stdout.flush(); "
                        "time.sleep(1)"
                    ),
                ]
            },
        )
        paged_session = paged["session_id"]
        first_page = await tool_call(
            process,
            10,
            "read_output",
            {
                "session_id": paged_session,
                "until_stdout_contains": "abcdefghij",
                "timeout_ms": 5000,
                "max_bytes": 4,
            },
        )
        second_page = await tool_call(
            process,
            11,
            "read_output",
            {"session_id": paged_session, "timeout_ms": 5000, "max_bytes": 4},
        )
        third_page = await tool_call(
            process,
            12,
            "read_output",
            {"session_id": paged_session, "timeout_ms": 5000, "max_bytes": 4},
        )
        assert first_page["stdout"] == "abcd"
        assert first_page["stdout_truncated"] is True
        assert second_page["stdout"] == "efgh"
        assert second_page["stdout_truncated"] is True
        assert third_page["stdout"] == "ij"
        await tool_call(process, 13, "stop_process", {"session_id": paged_session})

        backend = await tool_call(
            process,
            14,
            "start_process",
            {
                "command": [
                    "uv",
                    "run",
                    "python",
                    "-u",
                    str(
                        REPO_ROOT
                        / ".agents"
                        / "plugins"
                        / "plugins"
                        / "mutsuki-codex-core"
                        / "scripts"
                        / "mutsuki_codex_strategy_backend.py"
                    ),
                    "--agent-id",
                    "agent-a",
                ],
                "cwd": "python/mutsuki-runtime-python",
            },
        )
        backend_session = backend["session_id"]
        source_response = await tool_call(
            process,
            15,
            "jsonl_request",
            {
                "session_id": backend_session,
                "request": {
                    "id": "req-1",
                    "method": "list_sources",
                    "params": {"agent_id": "agent-a"},
                },
                "timeout_ms": 15000,
            },
        )
        assert source_response["response"]["ok"] is True
        assert source_response["response"]["result"][0]["descriptor"]["source_id"] == "codex:local"
        await tool_call(process, 16, "stop_process", {"session_id": backend_session})
    finally:
        process.terminate()
        try:
            await asyncio.wait_for(process.wait(), 2)
        except TimeoutError:
            process.kill()
            await process.wait()
    return 0


async def request(
    process: asyncio.subprocess.Process,
    request_id: int,
    method: str,
    params: dict[str, Any],
) -> dict[str, Any]:
    assert process.stdin is not None
    assert process.stdout is not None
    process.stdin.write(
        json.dumps(
            {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params},
            separators=(",", ":"),
        ).encode()
        + b"\n"
    )
    await process.stdin.drain()
    line = await asyncio.wait_for(process.stdout.readline(), 5)
    assert line
    response = json.loads(line)
    assert response["id"] == request_id
    assert "error" not in response, response
    return response


async def tool_call(
    process: asyncio.subprocess.Process,
    request_id: int,
    name: str,
    arguments: dict[str, Any],
) -> dict[str, Any]:
    response = await request(
        process,
        request_id,
        "tools/call",
        {"name": name, "arguments": arguments},
    )
    result = response["result"]
    assert result.get("isError") is False, result
    return json.loads(result["content"][0]["text"])


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
