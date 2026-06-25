from __future__ import annotations

import asyncio
import json
from typing import TextIO

from mutsuki_runtime_python.contracts.codec import JsonValue, from_json_dict, to_json_dict
from mutsuki_runtime_python.contracts.errors import ERR_RUNTIME_HOST_FAILED, RuntimeError
from mutsuki_runtime_python.contracts.runner import RunnerContext
from mutsuki_runtime_python.contracts.task import Task
from mutsuki_runtime_python.runners.host import PythonRunnerHost
from mutsuki_runtime_python.runners.protocol import RunnerInvokeError


class StdioJsonlRunnerServer:
    def __init__(self, host: PythonRunnerHost) -> None:
        self._host = host

    async def handle_request(self, request: object) -> dict[str, JsonValue]:
        try:
            raw = self._request_mapping(request)
            request_id = self._request_id(raw)
            method = self._method(raw)
            params = self._params(raw)
            result = await self._dispatch(method, params)
            return {"id": request_id, "ok": True, "result": result}
        except RunnerInvokeError as exc:
            return self._error_response(self._safe_request_id(request), exc.error)
        except Exception as exc:
            return self._error_response(
                self._safe_request_id(request),
                RuntimeError(
                    code=ERR_RUNTIME_HOST_FAILED,
                    source="python_stdio_jsonl",
                    route="python.stdio.request",
                    evidence={
                        "exception_type": type(exc).__qualname__,
                        "exception_repr": repr(exc),
                    },
                ),
            )

    async def serve(self, input_stream: TextIO, output_stream: TextIO) -> None:
        for line in input_stream:
            if not line.strip():
                continue
            response = await self.handle_request(json.loads(line))
            output_stream.write(json.dumps(response, separators=(",", ":"), ensure_ascii=False))
            output_stream.write("\n")
            output_stream.flush()

    async def _dispatch(self, method: str, params: dict[str, object]) -> JsonValue:
        if method == "runner.step":
            runner_id = self._str_param(params, "runner_id")
            ctx = from_json_dict(RunnerContext, self._mapping_param(params, "ctx"))
            tasks = tuple(
                from_json_dict(Task, self._mapping(item, "Task"))
                for item in self._sequence_param(params, "tasks")
            )
            return [
                to_json_dict(result)
                for result in await self._host.step_runner(runner_id, ctx, tasks)
            ]
        if method == "runner.cancel":
            await self._host.cancel_runner(
                self._str_param(params, "runner_id"),
                self._str_param(params, "invocation_id"),
            )
            return None
        if method == "runner.dispose":
            await self._host.dispose_runner(self._str_param(params, "runner_id"))
            return None
        raise RunnerInvokeError(
            RuntimeError(
                code=ERR_RUNTIME_HOST_FAILED,
                source="python_stdio_jsonl",
                route=f"python.stdio.{method}",
                evidence={"reason": "unknown_method", "method": method},
            )
        )

    @staticmethod
    def _request_mapping(request: object) -> dict[str, object]:
        if not isinstance(request, dict):
            raise TypeError("request expects mapping")
        return request

    @staticmethod
    def _request_id(request: dict[str, object]) -> str:
        return StdioJsonlRunnerServer._str_param(request, "id")

    @staticmethod
    def _safe_request_id(request: object) -> str | None:
        if isinstance(request, dict) and isinstance(request.get("id"), str):
            return request["id"]
        return None

    @staticmethod
    def _method(request: dict[str, object]) -> str:
        return StdioJsonlRunnerServer._str_param(request, "method")

    @staticmethod
    def _params(request: dict[str, object]) -> dict[str, object]:
        return StdioJsonlRunnerServer._mapping_param(request, "params")

    @staticmethod
    def _str_param(params: dict[str, object], key: str) -> str:
        value = params.get(key)
        if not isinstance(value, str):
            raise TypeError(f"{key} expects str")
        return value

    @staticmethod
    def _mapping_param(params: dict[str, object], key: str) -> dict[str, object]:
        return StdioJsonlRunnerServer._mapping(params.get(key), key)

    @staticmethod
    def _mapping(value: object, key: str) -> dict[str, object]:
        if not isinstance(value, dict):
            raise TypeError(f"{key} expects mapping")
        return value

    @staticmethod
    def _sequence_param(params: dict[str, object], key: str) -> tuple[object, ...]:
        value = params.get(key)
        if not isinstance(value, list | tuple):
            raise TypeError(f"{key} expects sequence")
        return tuple(value)

    @staticmethod
    def _error_response(request_id: str | None, error: RuntimeError) -> dict[str, JsonValue]:
        return {"id": request_id, "ok": False, "error": to_json_dict(error)}


def run_stdio_server(host: PythonRunnerHost, input_stream: TextIO, output_stream: TextIO) -> None:
    asyncio.run(StdioJsonlRunnerServer(host).serve(input_stream, output_stream))
