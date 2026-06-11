from __future__ import annotations

import argparse
import asyncio
import json
import sys
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Protocol

for parent in Path(__file__).resolve().parents:
    package_src = parent / "python" / "mutsukicore-runtime-python" / "src"
    if package_src.is_dir():
        sys.path.insert(0, str(package_src))
        break

from mutsukicore_runtime_python.backend import BackendInvokeError
from mutsukicore_runtime_python.contracts import (
    ERR_RUNTIME_BACKEND_FAILED,
    Envelope,
    JsonValue,
    OperationSnapshot,
    RuntimeError as MutsukiRuntimeError,
    SourceDescriptor,
    SourceSnapshot,
    StrategyResult,
    StrategyResultStatus,
    to_json_dict,
)
from mutsukicore_runtime_python.host import PythonBackendHost
from mutsukicore_runtime_python.stdio import run_stdio_server

PLUGIN_ID = "mutsukicore-codex-core"
DEFAULT_SOURCE_ID = "codex:local"
DEFAULT_SOURCE_KIND = "codex.strategy"


class CodexRunner(Protocol):
    async def run_decision(self, prompt: str) -> str: ...


@dataclass
class _StaticStrategyRunner:
    output: str

    async def run_decision(self, prompt: str) -> str:
        _ = prompt
        return self.output


@dataclass
class SubprocessCodexRunner:
    command: Sequence[str] = ("codex", "exec")
    cwd: str | None = None

    async def run_decision(self, prompt: str) -> str:
        process = await asyncio.create_subprocess_exec(
            *self.command,
            prompt,
            cwd=self.cwd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stdout, stderr = await process.communicate()
        if process.returncode != 0:
            raise BackendInvokeError(
                backend_error(
                    route="codex.exec",
                    evidence={
                        "exit_code": process.returncode,
                        "stderr": stderr.decode(errors="replace"),
                    },
                )
            )
        return stdout.decode(errors="replace")


@dataclass
class CodexStrategyBackend:
    runner: CodexRunner
    operation_snapshots: Sequence[OperationSnapshot] = ()
    source_snapshots: Sequence[SourceSnapshot] = ()
    sessions: dict[str, list[dict[str, JsonValue]]] = field(default_factory=dict)

    async def on_awake(self, agent_id: str) -> None:
        self.sessions.setdefault(agent_id, [])

    async def on_input(self, agent_id: str, envelope: Envelope) -> StrategyResult:
        self.sessions.setdefault(agent_id, []).append(
            {"event": "input", "envelope": to_json_dict(envelope)}
        )
        return await self._decide(agent_id, "on_input", envelope)

    async def next_step(self, agent_id: str) -> StrategyResult:
        self.sessions.setdefault(agent_id, [])
        return await self._decide(agent_id, "next_step", None)

    async def on_stop(self, agent_id: str) -> None:
        self.sessions.pop(agent_id, None)

    async def _decide(
        self,
        agent_id: str,
        phase: str,
        envelope: Envelope | None,
    ) -> StrategyResult:
        prompt = self._build_prompt(agent_id, phase, envelope)
        try:
            raw = await self.runner.run_decision(prompt)
        except BackendInvokeError as exc:
            return StrategyResult(status=StrategyResultStatus.FAILED, error=exc.error)
        except Exception as exc:
            return failed_result(
                route="codex.runner",
                evidence={
                    "exception_type": type(exc).__qualname__,
                    "exception_repr": repr(exc),
                },
            )
        return parse_strategy_result(raw)

    def _build_prompt(
        self,
        agent_id: str,
        phase: str,
        envelope: Envelope | None,
    ) -> str:
        context: dict[str, JsonValue] = {
            "role": "mutsukicore_strategy_backend",
            "plugin_id": PLUGIN_ID,
            "agent_id": agent_id,
            "phase": phase,
            "contract": {
                "allowed_status": ["wait_input", "continue", "failed"],
                "output": "Return one JSON object matching StrategyResult.",
                "tool_rule": "Do not call tools directly; put intended Operation use in decision.",
            },
            "operations": [to_json_dict(item) for item in self.operation_snapshots],
            "sources": [to_json_dict(item) for item in self.source_snapshots],
            "history": self.sessions.get(agent_id, []),
        }
        if envelope is not None:
            context["envelope"] = to_json_dict(envelope)
        return json.dumps(context, ensure_ascii=False, separators=(",", ":"))


def parse_strategy_result(raw: str) -> StrategyResult:
    try:
        payload = _first_json_object(raw)
    except ValueError as exc:
        return failed_result(
            route="codex.output.decode",
            evidence={"reason": str(exc), "output": raw},
        )
    try:
        status = StrategyResultStatus(_str_field(payload, "status"))
    except (TypeError, ValueError) as exc:
        return failed_result(
            route="codex.output.status",
            evidence={"reason": str(exc), "output": raw},
        )
    if status not in {
        StrategyResultStatus.WAIT_INPUT,
        StrategyResultStatus.CONTINUE,
        StrategyResultStatus.FAILED,
    }:
        return failed_result(
            route="codex.output.status",
            evidence={"reason": "unsupported_status", "status": status.value},
        )
    error = _runtime_error(payload.get("error")) if payload.get("error") is not None else None
    if status == StrategyResultStatus.FAILED and error is None:
        error = backend_error(
            route="codex.output.failed",
            evidence={"reason": "failed_status_missing_error", "output": raw},
        )
    return StrategyResult(
        status=status,
        decision=_json_value(payload.get("decision")),
        emitted=(),
        error=error,
    )


def failed_result(
    route: str,
    evidence: Mapping[str, str | int | float | bool] | None = None,
) -> StrategyResult:
    return StrategyResult(
        status=StrategyResultStatus.FAILED,
        error=backend_error(route=route, evidence=evidence),
    )


def build_source_snapshot(
    source_id: str = DEFAULT_SOURCE_ID,
    kind: str = DEFAULT_SOURCE_KIND,
) -> SourceSnapshot:
    return SourceSnapshot(
        descriptor=SourceDescriptor(
            source_id=source_id,
            kind=kind,
            capabilities=("strategy",),
            description="Local Codex strategy backend source",
        ),
        plugin_id=PLUGIN_ID,
        plugin_generation=0,
    )


def build_backend_host(
    agent_ids: Iterable[str],
    runner: CodexRunner | None = None,
    operation_snapshots: Sequence[OperationSnapshot] = (),
) -> PythonBackendHost:
    source_snapshot = build_source_snapshot()
    strategy = CodexStrategyBackend(
        runner=runner or SubprocessCodexRunner(),
        operation_snapshots=operation_snapshots,
        source_snapshots=(source_snapshot,),
    )
    host = PythonBackendHost()
    host.register_source(source_snapshot)
    for agent_id in agent_ids:
        host.register_agent(agent_id, strategy)
    return host


def backend_error(
    route: str,
    evidence: Mapping[str, str | int | float | bool] | None = None,
) -> MutsukiRuntimeError:
    return MutsukiRuntimeError(
        code=ERR_RUNTIME_BACKEND_FAILED,
        source=PLUGIN_ID,
        route=route,
        evidence=dict(evidence or {}),
    )


def _load_operation_snapshot_list(raw: object) -> list[OperationSnapshot]:
    """Parse a JSON list of OperationSnapshot objects from CLI input."""
    if not isinstance(raw, list):
        raise TypeError("expected a JSON array of OperationSnapshot")
    return [OperationSnapshot.from_json_dict(item) for item in raw]


def _first_json_object(raw: str) -> dict[str, JsonValue]:
    decoder = json.JSONDecoder()
    text = raw.strip()
    if not text:
        raise ValueError("empty_output")
    for index, character in enumerate(text):
        if character != "{":
            continue
        try:
            value, _end = decoder.raw_decode(text[index:])
        except json.JSONDecodeError:
            continue
        if not isinstance(value, dict):
            raise ValueError("top_level_not_object")
        return _json_object(value)
    raise ValueError("missing_json_object")


def _runtime_error(value: object) -> MutsukiRuntimeError:
    if not isinstance(value, Mapping):
        return backend_error(
            route="codex.output.error",
            evidence={"reason": "error_not_object"},
        )
    return MutsukiRuntimeError.from_json_dict(_json_object(value))


def _json_object(value: Mapping[object, object]) -> dict[str, JsonValue]:
    return {str(key): _json_value(item) for key, item in value.items()}


def _json_value(value: object) -> JsonValue:
    if value is None or isinstance(value, bool | int | float | str):
        return value
    if isinstance(value, Mapping):
        return _json_object(value)
    if isinstance(value, Sequence) and not isinstance(value, str | bytes | bytearray):
        return [_json_value(item) for item in value]
    return str(value)


def _str_field(payload: Mapping[str, JsonValue], field_name: str) -> str:
    value = payload.get(field_name)
    if not isinstance(value, str):
        raise TypeError(f"{field_name} expects str")
    return value


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Run Codex as a MutsukiCore StrategyBackend.")
    parser.add_argument("--agent-id", action="append", required=True)
    parser.add_argument(
        "--stub-output",
        help="Use deterministic StrategyResult JSON for smoke tests instead of codex exec.",
    )
    parser.add_argument(
        "--operation-snapshots",
        type=Path,
        action="append",
        dest="operation_snapshot_paths",
        default=[],
        help="Path to a JSON file containing a list of OperationSnapshot objects. Can be specified multiple times.",
    )
    parser.add_argument(
        "--operation-snapshots-stdin",
        action="store_true",
        dest="operation_snapshots_stdin",
        default=False,
        help="Read a JSON array of OperationSnapshot objects from the first line of stdin, then continue with JSONL protocol on remaining stdin.",
    )
    args = parser.parse_args(argv)
    runner = _StaticStrategyRunner(args.stub_output) if args.stub_output is not None else None
    operations: list[OperationSnapshot] = []
    for path in args.operation_snapshot_paths:
        raw = json.loads(path.read_text(encoding="utf-8"))
        operations.extend(_load_operation_snapshot_list(raw))
    if args.operation_snapshots_stdin:
        first_line = sys.stdin.readline()
        raw = json.loads(first_line)
        operations.extend(_load_operation_snapshot_list(raw))
    host = build_backend_host(args.agent_id, runner, operations)
    run_stdio_server(host, sys.stdin, sys.stdout)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
