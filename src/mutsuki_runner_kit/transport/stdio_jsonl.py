from __future__ import annotations

import asyncio
import json
from typing import Protocol, TextIO

from mutsuki_runner_kit.contracts.batch import WorkBatch
from mutsuki_runner_kit.contracts.codec import JsonValue, from_json_dict, to_json_dict
from mutsuki_runner_kit.contracts.errors import ERR_RUNTIME_HOST_FAILED, RuntimeError
from mutsuki_runner_kit.contracts.resource import (
    CommandBatch,
    CommandPlan,
    ExportPlan,
    PlanReceipt,
    SagaPlan,
)
from mutsuki_runner_kit.contracts.runner import RunnerContext
from mutsuki_runner_kit.runners.backend import PythonRunnerBackend
from mutsuki_runner_kit.runners.protocol import RunnerInvokeError


class ResourceRequestHandler(Protocol):
    def execute_export_plan(self, plan: ExportPlan) -> PlanReceipt: ...

    def execute_command_plan(self, plan: CommandPlan) -> PlanReceipt: ...

    def execute_command_batch(self, batch: CommandBatch) -> tuple[PlanReceipt, ...]: ...

    def execute_saga_plan(self, saga: SagaPlan) -> tuple[PlanReceipt, ...]: ...


class StdioJsonlBridge:
    def __init__(
        self,
        runner_backend: PythonRunnerBackend,
        resource_handler: ResourceRequestHandler | None = None,
    ) -> None:
        self._runner_backend = runner_backend
        self._resource_handler = resource_handler

    async def handle_request(self, request: object) -> dict[str, JsonValue]:
        try:
            raw = self.request_mapping(request)
            request_id = self.request_id(raw)
            method = self.method(raw)
            params = self.params(raw)
            result = await self.dispatch(method, params)
            return {"id": request_id, "ok": True, "result": result}
        except RunnerInvokeError as exc:
            return self.error_response(self.safe_request_id(request), exc.error)
        except Exception as exc:
            return self.error_response(
                self.safe_request_id(request),
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

    async def dispatch(self, method: str, params: dict[str, object]) -> JsonValue:
        if method.startswith("runner."):
            return await self.dispatch_runner_method(method, params)
        if method.startswith("resource."):
            if self._resource_handler is None:
                raise resource_handler_missing(method)
            return self.dispatch_resource_method(method, params)
        raise unknown_method(method)

    async def dispatch_runner_method(self, method: str, params: dict[str, object]) -> JsonValue:
        if method == "runner.run_batch":
            runner_id = self.str_param(params, "runner_id")
            ctx = from_json_dict(RunnerContext, self.mapping_param(params, "ctx"))
            batch = from_json_dict(WorkBatch, self.mapping_param(params, "batch"))
            return to_json_dict(await self._runner_backend.run_batch_runner(runner_id, ctx, batch))
        if method == "runner.cancel":
            await self._runner_backend.cancel_runner(
                self.str_param(params, "runner_id"),
                self.str_param(params, "invocation_id"),
            )
            return None
        if method == "runner.dispose":
            await self._runner_backend.dispose_runner(self.str_param(params, "runner_id"))
            return None
        raise unknown_method(method)

    def dispatch_resource_method(self, method: str, params: dict[str, object]) -> JsonValue:
        resources = self._resource_handler
        if resources is None:
            raise resource_handler_missing(method)
        if method == "resource.export":
            plan = from_json_dict(ExportPlan, self.mapping_param(params, "plan"))
            return to_json_dict(resources.execute_export_plan(plan))
        if method == "resource.command":
            plan = from_json_dict(CommandPlan, self.mapping_param(params, "plan"))
            return to_json_dict(resources.execute_command_plan(plan))
        if method == "resource.command_batch":
            batch = from_json_dict(CommandBatch, self.mapping_param(params, "batch"))
            return [to_json_dict(receipt) for receipt in resources.execute_command_batch(batch)]
        if method == "resource.saga":
            saga = from_json_dict(SagaPlan, self.mapping_param(params, "saga"))
            return [to_json_dict(receipt) for receipt in resources.execute_saga_plan(saga)]
        raise unknown_method(method)

    @staticmethod
    def request_mapping(request: object) -> dict[str, object]:
        if not isinstance(request, dict):
            raise TypeError("request expects mapping")
        return request

    @staticmethod
    def request_id(request: dict[str, object]) -> str:
        return StdioJsonlBridge.str_param(request, "id")

    @staticmethod
    def safe_request_id(request: object) -> str | None:
        if isinstance(request, dict) and isinstance(request.get("id"), str):
            return request["id"]
        return None

    @staticmethod
    def method(request: dict[str, object]) -> str:
        return StdioJsonlBridge.str_param(request, "method")

    @staticmethod
    def params(request: dict[str, object]) -> dict[str, object]:
        return StdioJsonlBridge.mapping_param(request, "params")

    @staticmethod
    def str_param(params: dict[str, object], key: str) -> str:
        value = params.get(key)
        if not isinstance(value, str):
            raise TypeError(f"{key} expects str")
        return value

    @staticmethod
    def mapping_param(params: dict[str, object], key: str) -> dict[str, object]:
        return StdioJsonlBridge.mapping(params.get(key), key)

    @staticmethod
    def mapping(value: object, key: str) -> dict[str, object]:
        if not isinstance(value, dict):
            raise TypeError(f"{key} expects mapping")
        return value

    @staticmethod
    def sequence_param(params: dict[str, object], key: str) -> tuple[object, ...]:
        value = params.get(key)
        if not isinstance(value, list | tuple):
            raise TypeError(f"{key} expects sequence")
        return tuple(value)

    @staticmethod
    def error_response(request_id: str | None, error: RuntimeError) -> dict[str, JsonValue]:
        return {"id": request_id, "ok": False, "error": to_json_dict(error)}


def run_stdio_bridge(
    runner_backend: PythonRunnerBackend,
    input_stream: TextIO,
    output_stream: TextIO,
) -> None:
    bridge = StdioJsonlBridge(runner_backend)
    asyncio.run(bridge.serve(input_stream, output_stream))


def run_stdio_provider_bridge(
    runner_backend: PythonRunnerBackend,
    resource_handler: ResourceRequestHandler,
    input_stream: TextIO,
    output_stream: TextIO,
) -> None:
    bridge = StdioJsonlBridge(runner_backend, resource_handler)
    asyncio.run(bridge.serve(input_stream, output_stream))


def unknown_method(method: str) -> RunnerInvokeError:
    return RunnerInvokeError(
        RuntimeError(
            code=ERR_RUNTIME_HOST_FAILED,
            source="python_stdio_jsonl",
            route=f"python.stdio.{method}",
            evidence={"reason": "unknown_method", "method": method},
        )
    )


def resource_handler_missing(method: str) -> RunnerInvokeError:
    return RunnerInvokeError(
        RuntimeError(
            code=ERR_RUNTIME_HOST_FAILED,
            source="python_stdio_jsonl",
            route=f"python.stdio.{method}",
            evidence={"reason": "resource_handler_missing", "method": method},
        )
    )
