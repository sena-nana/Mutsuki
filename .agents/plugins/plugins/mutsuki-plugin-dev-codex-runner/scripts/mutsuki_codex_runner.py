from __future__ import annotations

# ruff: noqa: E402

import argparse
import asyncio
import json
import sys
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

for parent in Path(__file__).resolve().parents:
    package_src = parent / "python" / "mutsuki-runtime-python" / "src"
    if package_src.is_dir():
        sys.path.insert(0, str(package_src))
        break

from mutsuki_runtime_python.contracts.codec import JsonValue, as_json_value, to_json_dict
from mutsuki_runtime_python.contracts.errors import ERR_RUNTIME_HOST_FAILED
from mutsuki_runtime_python.contracts.errors import RuntimeError as MutsukiRuntimeError
from mutsuki_runtime_python.contracts.event import DomainEvent
from mutsuki_runtime_python.contracts.runner import (
    RunnerContext,
    RunnerDescriptor,
    RunnerPurity,
    RunnerResult,
    RunnerStatus,
)
from mutsuki_runtime_python.contracts.task import Task
from mutsuki_runtime_python.runners.host import PythonRunnerHost
from mutsuki_runtime_python.transport.stdio_jsonl import run_stdio_server

PLUGIN_ID = "mutsuki.experimental.dev.codex_runner"
RUNNER_ID = "mutsuki.dev.codex.runner"
PROTOCOL_ID = "mutsuki.dev.codex.run"
RESULT_EVENT_KIND = "mutsuki.dev.codex.result"


class CodexRunner(Protocol):
    async def run_decision(self, prompt: str) -> str: ...


@dataclass
class _StaticCodexRunner:
    output: str

    async def run_decision(self, _prompt: str) -> str:
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
            raise RuntimeError(
                f"codex exec failed with {process.returncode}: {stderr.decode(errors='replace')}"
            )
        return stdout.decode(errors="replace")


@dataclass
class CodexEffectRunner:
    runner: CodexRunner

    @property
    def descriptor(self) -> RunnerDescriptor:
        return RunnerDescriptor(
            runner_id=RUNNER_ID,
            plugin_id=PLUGIN_ID,
            plugin_generation=1,
            accepted_protocol_ids=(PROTOCOL_ID,),
            purity=RunnerPurity.EFFECTFUL,
            metadata={"event_kind": RESULT_EVENT_KIND},
            contract_surfaces=(f"runner:{RUNNER_ID}", f"task_protocol:{PROTOCOL_ID}"),
        )

    async def step(
        self,
        ctx: RunnerContext,
        tasks: tuple[Task, ...],
    ) -> tuple[RunnerResult, ...]:
        results: list[RunnerResult] = []
        for task in tasks:
            results.append(await self._step_one(ctx, task))
        return tuple(results)

    async def cancel(self, _invocation_id: str) -> None:
        return

    async def dispose(self) -> None:
        return

    async def _step_one(self, _ctx: RunnerContext, task: Task) -> RunnerResult:
        if task.protocol_id != PROTOCOL_ID:
            return _failed_result(
                task.task_id,
                route="codex.protocol_id",
                evidence={
                    "reason": "unsupported_protocol_id",
                    "protocol_id": task.protocol_id,
                },
            )
        try:
            payload = _json_object(task.payload)
            prompt = _str_field(payload, "prompt")
            raw = await self.runner.run_decision(_build_prompt(payload, prompt))
            result_payload = _parse_strategy_payload(raw)
        except Exception as exc:
            return _failed_result(
                task.task_id,
                route="codex.runner",
                evidence={
                    "exception_type": type(exc).__qualname__,
                    "exception_repr": repr(exc),
                },
            )
        return RunnerResult(
            task_id=task.task_id,
            events=(
                DomainEvent(
                    event_id=f"{task.task_id}:{RESULT_EVENT_KIND}",
                    kind=RESULT_EVENT_KIND,
                    payload=result_payload,
                ),
            ),
            status=RunnerStatus.COMPLETED,
        )


def _build_prompt(payload: Mapping[str, JsonValue], prompt: str) -> str:
    context: dict[str, JsonValue] = {
        "role": "mutsuki_effect_runner",
        "plugin_id": PLUGIN_ID,
        "protocol_id": PROTOCOL_ID,
        "contract": {
            "allowed_status": ["wait_input", "continue", "failed"],
            "output": "Return one JSON object with status, optional decision, optional emitted, optional error.",
        },
        "prompt": prompt,
    }
    for key in ("agent_id", "phase", "context"):
        if key in payload:
            context[key] = payload[key]
    return json.dumps(context, ensure_ascii=False, separators=(",", ":"))


def _parse_strategy_payload(raw: str) -> JsonValue:
    payload = _first_json_object(raw)
    status = _str_field(payload, "status")
    if status not in {"wait_input", "continue", "failed"}:
        raise ValueError(f"unsupported_status:{status}")
    result: dict[str, JsonValue] = {
        "status": status,
        "decision": as_json_value(payload.get("decision")),
        "emitted": as_json_value(payload.get("emitted", [])),
    }
    if payload.get("error") is not None:
        result["error"] = as_json_value(payload["error"])
    if status == "failed" and "error" not in result:
        result["error"] = to_json_dict(
            _runtime_error(
                route="codex.output.failed",
                evidence={"reason": "failed_status_missing_error"},
            )
        )
    return result


def _failed_result(
    task_id: str,
    *,
    route: str,
    evidence: Mapping[str, str | int | float | bool] | None = None,
) -> RunnerResult:
    error = _runtime_error(route=route, evidence=evidence)
    return RunnerResult(
        task_id=task_id,
        events=(
            DomainEvent(
                event_id=f"{task_id}:{RESULT_EVENT_KIND}",
                kind=RESULT_EVENT_KIND,
                payload={"status": "failed", "error": to_json_dict(error)},
            ),
        ),
        status=RunnerStatus.FAILED,
    )


def _runtime_error(
    route: str,
    evidence: Mapping[str, str | int | float | bool] | None = None,
) -> MutsukiRuntimeError:
    return MutsukiRuntimeError(
        code=ERR_RUNTIME_HOST_FAILED,
        source=PLUGIN_ID,
        route=route,
        evidence=dict(evidence or {}),
    )


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


def _json_object(value: object) -> dict[str, JsonValue]:
    if not isinstance(value, Mapping):
        raise TypeError("value expects object")
    return {str(key): as_json_value(item) for key, item in value.items()}


def _str_field(payload: Mapping[str, JsonValue], field_name: str) -> str:
    value = payload.get(field_name)
    if not isinstance(value, str):
        raise TypeError(f"{field_name} expects str")
    return value


def build_runner_host(runner: CodexRunner | None = None) -> PythonRunnerHost:
    host = PythonRunnerHost()
    host.register_runner(CodexEffectRunner(runner=runner or SubprocessCodexRunner()))
    return host


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Run Codex as a Mutsuki effect runner.")
    parser.add_argument(
        "--stub-output",
        help="Use deterministic strategy JSON for smoke tests instead of codex exec.",
    )
    args = parser.parse_args(argv)
    runner = _StaticCodexRunner(args.stub_output) if args.stub_output is not None else None
    run_stdio_server(build_runner_host(runner), sys.stdin, sys.stdout)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
