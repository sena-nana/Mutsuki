from __future__ import annotations

import asyncio
import json
import os
import shlex
import signal
import sys
import time
import uuid
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path

JsonValue = None | bool | int | float | str | list["JsonValue"] | dict[str, "JsonValue"]

PLUGIN_ID = "mutsuki-plugin-dev-test-io"
DEFAULT_TIMEOUT_MS = 10_000
DEFAULT_READ_TIMEOUT_MS = 1_000
DEFAULT_MAX_BYTES = 64 * 1024
MAX_MAX_BYTES = 1024 * 1024
SESSION_BUFFER_LIMIT = 4 * MAX_MAX_BYTES


@dataclass
class StreamBuffer:
    data: bytearray = field(default_factory=bytearray)
    base_pos: int = 0
    read_pos: int = 0
    dropped_unread: int = 0

    @property
    def end_pos(self) -> int:
        return self.base_pos + len(self.data)

    def append(self, chunk: bytes) -> None:
        self.data.extend(chunk)
        overflow = len(self.data) - SESSION_BUFFER_LIMIT
        if overflow <= 0:
            return

        unread_before_drop = max(0, self.read_pos - self.base_pos)
        if overflow > unread_before_drop:
            lost_unread = overflow - unread_before_drop
            self.dropped_unread += lost_unread
            self.read_pos += lost_unread

        del self.data[:overflow]
        self.base_pos += overflow
        self.read_pos = max(self.read_pos, self.base_pos)

    def unread_bytes(self) -> bytes:
        return self.bytes_from(self.read_pos)

    def bytes_from(self, position: int) -> bytes:
        start = max(position, self.base_pos) - self.base_pos
        return bytes(self.data[start:])

    def consume(self, max_bytes: int) -> tuple[bytes, bool, int]:
        unread = self.unread_bytes()
        chunk = unread[:max_bytes]
        self.read_pos += len(chunk)
        self._prune_consumed()
        dropped = self.dropped_unread
        self.dropped_unread = 0
        return chunk, len(unread) > len(chunk), dropped

    def advance_to(self, position: int) -> None:
        self.read_pos = max(self.read_pos, min(position, self.end_pos))
        self._prune_consumed()

    def _prune_consumed(self) -> None:
        consumed = self.read_pos - self.base_pos
        if consumed <= SESSION_BUFFER_LIMIT // 2:
            return
        del self.data[:consumed]
        self.base_pos += consumed


@dataclass
class LimitedOutput:
    data: bytearray = field(default_factory=bytearray)
    total_bytes: int = 0

    def append(self, chunk: bytes, max_bytes: int) -> None:
        self.total_bytes += len(chunk)
        remaining = max_bytes - len(self.data)
        if remaining > 0:
            self.data.extend(chunk[:remaining])

    @property
    def truncated(self) -> bool:
        return self.total_bytes > len(self.data)


@dataclass
class ProcessSession:
    session_id: str
    process: asyncio.subprocess.Process
    cwd: str
    command: list[str]
    started_at: float
    stdout: StreamBuffer = field(default_factory=StreamBuffer)
    stderr: StreamBuffer = field(default_factory=StreamBuffer)
    stdout_task: asyncio.Task[None] | None = None
    stderr_task: asyncio.Task[None] | None = None

    def consume_output(self, max_bytes: int) -> dict[str, JsonValue]:
        stdout, stdout_truncated, stdout_dropped = self.stdout.consume(max_bytes)
        stderr, stderr_truncated, stderr_dropped = self.stderr.consume(max_bytes)
        return {
            "stdout": _decode_bytes(stdout),
            "stderr": _decode_bytes(stderr),
            "stdout_truncated": stdout_truncated,
            "stderr_truncated": stderr_truncated,
            "stdout_dropped_bytes": stdout_dropped,
            "stderr_dropped_bytes": stderr_dropped,
        }


class TestIoServer:
    def __init__(self) -> None:
        self.repo_root = _find_repo_root()
        self.sessions: dict[str, ProcessSession] = {}
        self.tools = {
            "run_command": self._tool_run_command,
            "start_process": self._tool_start_process,
            "write_stdin": self._tool_write_stdin,
            "read_output": self._tool_read_output,
            "stop_process": self._tool_stop_process,
            "jsonl_request": self._tool_jsonl_request,
        }

    async def serve(self) -> None:
        while True:
            line = await asyncio.to_thread(sys.stdin.readline)
            if line == "":
                await self._cleanup()
                return
            if not line.strip():
                continue
            try:
                request = json.loads(line)
                response = await self.handle_request(request)
            except Exception as exc:
                response = _error_response(
                    None,
                    code=-32603,
                    message="internal error",
                    data=_exception_data(exc),
                )
            if response is None:
                continue
            sys.stdout.write(json.dumps(response, separators=(",", ":"), ensure_ascii=False))
            sys.stdout.write("\n")
            sys.stdout.flush()

    async def handle_request(self, request: object) -> dict[str, JsonValue] | None:
        raw = _expect_mapping(request, "request")
        request_id = raw.get("id")
        method = _expect_str(raw.get("method"), "method")
        params = _optional_mapping(raw.get("params"), "params")

        if method.startswith("notifications/"):
            return None
        try:
            if method == "initialize":
                result = self._initialize_result()
            elif method == "tools/list":
                result = {"tools": _tool_schemas()}
            elif method == "tools/call":
                result = await self._handle_tool_call(params)
            elif method == "ping":
                result = {}
            else:
                return _error_response(
                    request_id,
                    code=-32601,
                    message=f"unknown method: {method}",
                )
            return {"jsonrpc": "2.0", "id": request_id, "result": result}
        except ToolError as exc:
            return {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": _tool_result(exc.payload, is_error=True),
            }
        except Exception as exc:
            return _error_response(
                request_id,
                code=-32602,
                message="invalid request",
                data=_exception_data(exc),
            )

    def _initialize_result(self) -> dict[str, JsonValue]:
        return {
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": PLUGIN_ID, "version": "0.1.0"},
            "instructions": (
                "Use these dev-only tools for local Mutsuki process I/O tests. "
                "Prefer jsonl_request for stdio JSONL runner checks and always set timeouts."
            ),
        }

    async def _handle_tool_call(self, params: Mapping[str, object]) -> dict[str, JsonValue]:
        name = _expect_str(params.get("name"), "name")
        arguments = _optional_mapping(params.get("arguments"), "arguments")
        handler = self.tools.get(name)
        if handler is None:
            raise ToolError({"error": "unknown_tool", "tool": name})
        result = await handler(arguments)
        return _tool_result(result, is_error=False)

    async def _tool_run_command(self, args: Mapping[str, object]) -> dict[str, JsonValue]:
        command = _command(args.get("command"))
        cwd = _resolve_cwd(self.repo_root, args.get("cwd"))
        timeout = _timeout_seconds(args.get("timeout_ms"), DEFAULT_TIMEOUT_MS)
        max_bytes = _max_bytes(args.get("max_bytes"))
        started = time.monotonic()
        process = await asyncio.create_subprocess_exec(
            *command,
            cwd=str(cwd),
            stdin=asyncio.subprocess.DEVNULL,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stdout_task = asyncio.create_task(_read_limited(process.stdout, max_bytes))
        stderr_task = asyncio.create_task(_read_limited(process.stderr, max_bytes))
        timed_out = await _wait_or_terminate(process, timeout)
        stdout = await stdout_task
        stderr = await stderr_task
        return {
            "command": command,
            "cwd": str(cwd),
            "exit_code": process.returncode,
            "timed_out": timed_out,
            "duration_ms": int((time.monotonic() - started) * 1000),
            "stdout": _decode_bytes(stdout.data),
            "stderr": _decode_bytes(stderr.data),
            "stdout_truncated": stdout.truncated,
            "stderr_truncated": stderr.truncated,
            "stdout_total_bytes": stdout.total_bytes,
            "stderr_total_bytes": stderr.total_bytes,
        }

    async def _tool_start_process(self, args: Mapping[str, object]) -> dict[str, JsonValue]:
        command = _command(args.get("command"))
        cwd = _resolve_cwd(self.repo_root, args.get("cwd"))
        session_id = str(uuid.uuid4())
        process = await asyncio.create_subprocess_exec(
            *command,
            cwd=str(cwd),
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        session = ProcessSession(
            session_id=session_id,
            process=process,
            cwd=str(cwd),
            command=command,
            started_at=time.monotonic(),
        )
        session.stdout_task = asyncio.create_task(_pump(process.stdout, session.stdout))
        session.stderr_task = asyncio.create_task(_pump(process.stderr, session.stderr))
        self.sessions[session_id] = session
        return {
            "session_id": session_id,
            "pid": process.pid,
            "command": command,
            "cwd": str(cwd),
        }

    async def _tool_write_stdin(self, args: Mapping[str, object]) -> dict[str, JsonValue]:
        session = self._session(args.get("session_id"))
        text = _expect_str(args.get("text"), "text")
        if session.process.stdin is None or session.process.returncode is not None:
            raise ToolError({"error": "stdin_unavailable", "session_id": session.session_id})
        data = text.encode()
        session.process.stdin.write(data)
        await session.process.stdin.drain()
        return {"session_id": session.session_id, "bytes_written": len(data)}

    async def _tool_read_output(self, args: Mapping[str, object]) -> dict[str, JsonValue]:
        session = self._session(args.get("session_id"))
        timeout_ms = _int(args.get("timeout_ms"), "timeout_ms", DEFAULT_READ_TIMEOUT_MS)
        max_bytes = _max_bytes(args.get("max_bytes"))
        until_stdout = _optional_str(args.get("until_stdout_contains"), "until_stdout_contains")
        until_stderr = _optional_str(args.get("until_stderr_contains"), "until_stderr_contains")

        deadline = time.monotonic() + max(0, timeout_ms) / 1000
        while True:
            stdout = session.stdout.unread_bytes()
            stderr = session.stderr.unread_bytes()
            stdout_text = stdout.decode(errors="replace")
            stderr_text = stderr.decode(errors="replace")
            if until_stdout is None and until_stderr is None and (stdout or stderr):
                break
            if until_stdout is not None and until_stdout in stdout_text:
                break
            if until_stderr is not None and until_stderr in stderr_text:
                break
            if session.process.returncode is not None:
                break
            if time.monotonic() >= deadline:
                break
            await asyncio.sleep(0.02)

        stdout, stdout_truncated, stdout_dropped = session.stdout.consume(max_bytes)
        stderr, stderr_truncated, stderr_dropped = session.stderr.consume(max_bytes)
        return {
            "session_id": session.session_id,
            "exit_code": session.process.returncode,
            "running": session.process.returncode is None,
            "stdout": _decode_bytes(stdout),
            "stderr": _decode_bytes(stderr),
            "stdout_truncated": stdout_truncated,
            "stderr_truncated": stderr_truncated,
            "stdout_dropped_bytes": stdout_dropped,
            "stderr_dropped_bytes": stderr_dropped,
        }

    async def _tool_stop_process(self, args: Mapping[str, object]) -> dict[str, JsonValue]:
        session = self._session(args.get("session_id"))
        timeout = _timeout_seconds(args.get("timeout_ms"), 2_000)
        await _terminate_and_wait(session.process, timeout)
        await self._finish_pumps(session)
        self.sessions.pop(session.session_id, None)
        max_bytes = _max_bytes(args.get("max_bytes"))
        return {
            "session_id": session.session_id,
            "exit_code": session.process.returncode,
            "duration_ms": int((time.monotonic() - session.started_at) * 1000),
            **session.consume_output(max_bytes),
        }

    async def _tool_jsonl_request(self, args: Mapping[str, object]) -> dict[str, JsonValue]:
        session = self._session(args.get("session_id"))
        request = _expect_mapping(args.get("request"), "request")
        request_id = request.get("id")
        if not isinstance(request_id, str):
            raise ToolError({"error": "request_id_required"})
        if session.process.stdin is None or session.process.returncode is not None:
            raise ToolError({"error": "stdin_unavailable", "session_id": session.session_id})

        before = session.stdout.end_pos
        line = json.dumps(_json_value(request), separators=(",", ":"), ensure_ascii=False) + "\n"
        session.process.stdin.write(line.encode())
        await session.process.stdin.drain()

        timeout_ms = _int(args.get("timeout_ms"), "timeout_ms", DEFAULT_TIMEOUT_MS)
        deadline = time.monotonic() + max(0, timeout_ms) / 1000
        scanned = before
        while time.monotonic() < deadline:
            if scanned < session.stdout.base_pos:
                raise ToolError(
                    {
                        "error": "jsonl_response_dropped",
                        "session_id": session.session_id,
                        "request_id": request_id,
                    }
                )
            buffer = session.stdout.bytes_from(scanned)
            buffer_base = scanned
            newline = buffer.find(b"\n")
            while newline != -1:
                raw_line = buffer[:newline].decode(errors="replace")
                scanned = buffer_base + newline + 1
                try:
                    response = json.loads(raw_line)
                except json.JSONDecodeError:
                    buffer = session.stdout.bytes_from(scanned)
                    buffer_base = scanned
                    newline = buffer.find(b"\n")
                    continue
                if isinstance(response, dict) and response.get("id") == request_id:
                    session.stdout.advance_to(scanned)
                    return {
                        "session_id": session.session_id,
                        "response": _json_value(response),
                        "raw": raw_line,
                    }
                buffer = session.stdout.bytes_from(scanned)
                buffer_base = scanned
                newline = buffer.find(b"\n")
            if session.process.returncode is not None:
                break
            await asyncio.sleep(0.02)

        raise ToolError(
            {
                "error": "jsonl_response_timeout",
                "session_id": session.session_id,
                "request_id": request_id,
                "exit_code": session.process.returncode,
                "stderr": session.stderr.unread_bytes().decode(errors="replace"),
            }
        )

    def _session(self, raw_session_id: object) -> ProcessSession:
        session_id = _expect_str(raw_session_id, "session_id")
        session = self.sessions.get(session_id)
        if session is None:
            raise ToolError({"error": "session_not_found", "session_id": session_id})
        return session

    async def _finish_pumps(self, session: ProcessSession) -> None:
        for task in (session.stdout_task, session.stderr_task):
            if task is None:
                continue
            try:
                await asyncio.wait_for(task, 1)
            except TimeoutError:
                task.cancel()

    async def _cleanup(self) -> None:
        for session_id in list(self.sessions):
            session = self.sessions[session_id]
            await _terminate_and_wait(session.process, 1)
            await self._finish_pumps(session)
            self.sessions.pop(session_id, None)


class ToolError(Exception):
    def __init__(self, payload: dict[str, JsonValue]) -> None:
        super().__init__(str(payload.get("error", "tool_error")))
        self.payload = payload


async def _pump(
    stream: asyncio.StreamReader | None,
    target: StreamBuffer,
) -> None:
    if stream is None:
        return
    while True:
        chunk = await stream.read(4096)
        if not chunk:
            return
        target.append(chunk)


async def _read_limited(
    stream: asyncio.StreamReader | None,
    max_bytes: int,
) -> LimitedOutput:
    output = LimitedOutput()
    if stream is None:
        return output
    while True:
        chunk = await stream.read(4096)
        if not chunk:
            return output
        output.append(chunk, max_bytes)


def _tool_schemas() -> list[dict[str, JsonValue]]:
    return [
        {
            "name": "run_command",
            "description": "Run a one-shot local command and capture stdout/stderr.",
            "inputSchema": {
                "type": "object",
                "properties": _common_command_properties(),
                "required": ["command"],
                "additionalProperties": False,
            },
        },
        {
            "name": "start_process",
            "description": "Start a long-running stdio process and return a session id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": _command_schema(),
                    "cwd": {"type": "string"},
                },
                "required": ["command"],
                "additionalProperties": False,
            },
        },
        {
            "name": "write_stdin",
            "description": "Write text to a running process stdin.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string"},
                    "text": {"type": "string"},
                },
                "required": ["session_id", "text"],
                "additionalProperties": False,
            },
        },
        {
            "name": "read_output",
            "description": "Read incremental stdout/stderr from a running process.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string"},
                    "timeout_ms": {"type": "integer", "minimum": 0},
                    "max_bytes": {"type": "integer", "minimum": 1},
                    "until_stdout_contains": {"type": "string"},
                    "until_stderr_contains": {"type": "string"},
                },
                "required": ["session_id"],
                "additionalProperties": False,
            },
        },
        {
            "name": "stop_process",
            "description": "Terminate a running process session.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string"},
                    "timeout_ms": {"type": "integer", "minimum": 0},
                    "max_bytes": {"type": "integer", "minimum": 1},
                },
                "required": ["session_id"],
                "additionalProperties": False,
            },
        },
        {
            "name": "jsonl_request",
            "description": "Send one JSONL request to a session and wait for the matching id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string"},
                    "request": {"type": "object"},
                    "timeout_ms": {"type": "integer", "minimum": 0},
                },
                "required": ["session_id", "request"],
                "additionalProperties": False,
            },
        },
    ]


def _common_command_properties() -> dict[str, JsonValue]:
    return {
        "command": _command_schema(),
        "cwd": {"type": "string"},
        "timeout_ms": {"type": "integer", "minimum": 0},
        "max_bytes": {"type": "integer", "minimum": 1},
    }


def _command_schema() -> dict[str, JsonValue]:
    return {
        "oneOf": [
            {"type": "array", "items": {"type": "string"}, "minItems": 1},
            {"type": "string"},
        ]
    }


def _tool_result(payload: dict[str, JsonValue], *, is_error: bool) -> dict[str, JsonValue]:
    return {
        "content": [
            {
                "type": "text",
                "text": json.dumps(payload, separators=(",", ":"), ensure_ascii=False),
            }
        ],
        "isError": is_error,
    }


def _error_response(
    request_id: object,
    *,
    code: int,
    message: str,
    data: dict[str, JsonValue] | None = None,
) -> dict[str, JsonValue]:
    error: dict[str, JsonValue] = {"code": code, "message": message}
    if data is not None:
        error["data"] = data
    response_id = request_id if isinstance(request_id, str | int) else None
    return {"jsonrpc": "2.0", "id": response_id, "error": error}


def _exception_data(exc: Exception) -> dict[str, JsonValue]:
    return {"exception_type": type(exc).__qualname__, "exception_repr": repr(exc)}


def _find_repo_root() -> Path:
    env_root = os.environ.get("MUTSUKI_TEST_IO_ROOT")
    if env_root:
        return Path(env_root).resolve()
    for parent in Path(__file__).resolve().parents:
        if (parent / "Cargo.toml").is_file() and (parent / "AGENTS.md").is_file():
            return parent
    return Path.cwd().resolve()


def _resolve_cwd(repo_root: Path, raw_cwd: object) -> Path:
    if raw_cwd is None:
        candidate = repo_root
    else:
        cwd = _expect_str(raw_cwd, "cwd")
        candidate = Path(cwd)
        if not candidate.is_absolute():
            candidate = repo_root / candidate
    resolved = candidate.resolve()
    if not _is_relative_to(resolved, repo_root.resolve()):
        raise ToolError(
            {
                "error": "cwd_outside_repo",
                "cwd": str(resolved),
                "repo_root": str(repo_root.resolve()),
            }
        )
    return resolved


def _is_relative_to(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
    except ValueError:
        return False
    return True


def _command(value: object) -> list[str]:
    if isinstance(value, str):
        parts = shlex.split(value, posix=os.name != "nt")
    elif isinstance(value, Sequence) and not isinstance(value, bytes | bytearray | str):
        parts = [_expect_str(item, "command[]") for item in value]
    else:
        raise TypeError("command expects string or string array")
    if not parts:
        raise TypeError("command must not be empty")
    return parts


def _timeout_seconds(value: object, default_ms: int) -> float:
    return max(0, _int(value, "timeout_ms", default_ms)) / 1000


def _max_bytes(value: object) -> int:
    return min(MAX_MAX_BYTES, max(1, _int(value, "max_bytes", DEFAULT_MAX_BYTES)))


def _int(value: object, field: str, default: int) -> int:
    if value is None:
        return default
    if not isinstance(value, int) or isinstance(value, bool):
        raise TypeError(f"{field} expects integer")
    return value


def _optional_str(value: object, field: str) -> str | None:
    if value is None:
        return None
    return _expect_str(value, field)


def _expect_str(value: object, field: str) -> str:
    if not isinstance(value, str):
        raise TypeError(f"{field} expects string")
    return value


def _expect_mapping(value: object, field: str) -> Mapping[str, object]:
    if not isinstance(value, Mapping):
        raise TypeError(f"{field} expects object")
    return value


def _optional_mapping(value: object, field: str) -> Mapping[str, object]:
    if value is None:
        return {}
    return _expect_mapping(value, field)


def _decode_bytes(data: bytes | bytearray) -> str:
    return bytes(data).decode(errors="replace")


async def _wait_or_terminate(
    process: asyncio.subprocess.Process,
    timeout_seconds: float,
) -> bool:
    try:
        await asyncio.wait_for(process.wait(), timeout_seconds)
        return False
    except TimeoutError:
        await _terminate_and_wait(process, 1)
        return True


async def _terminate_and_wait(
    process: asyncio.subprocess.Process,
    timeout_seconds: float,
) -> None:
    if process.returncode is not None:
        return
    _terminate_process(process)
    try:
        await asyncio.wait_for(process.wait(), timeout_seconds)
    except TimeoutError:
        process.kill()
        await process.wait()


def _terminate_process(process: asyncio.subprocess.Process) -> None:
    if process.returncode is not None:
        return
    if os.name == "nt":
        process.terminate()
    else:
        process.send_signal(signal.SIGTERM)


def _json_value(value: object) -> JsonValue:
    if value is None or isinstance(value, bool | int | float | str):
        return value
    if isinstance(value, Mapping):
        return {str(key): _json_value(item) for key, item in value.items()}
    if isinstance(value, Sequence) and not isinstance(value, bytes | bytearray | str):
        return [_json_value(item) for item in value]
    return str(value)


def main() -> int:
    asyncio.run(TestIoServer().serve())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
