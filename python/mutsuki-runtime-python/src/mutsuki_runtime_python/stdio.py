from __future__ import annotations

import asyncio
import json
from typing import Any, TextIO

from mutsuki_runtime_python.backend import BackendInvokeError
from mutsuki_runtime_python.contracts import (
    ERR_RUNTIME_BACKEND_FAILED,
    Envelope,
    JsonValue,
    LeaseToken,
    OperationHandlerKey,
    RefDescriptor,
    RuntimeError,
    from_json_dict,
    to_json_dict,
)
from mutsuki_runtime_python.host import PythonBackendHost
from mutsuki_runtime_python.resource import PythonResourceBackend


class StdioJsonlBackendServer:
    def __init__(
        self,
        host: PythonBackendHost,
        resource_backend: PythonResourceBackend | None = None,
    ) -> None:
        self._host = host
        self._resource_backend = resource_backend or PythonResourceBackend()

    async def handle_request(self, request: object) -> dict[str, JsonValue]:
        try:
            raw = self._request_mapping(request)
            request_id = self._request_id(raw)
            method = self._method(raw)
            params = self._params(raw)
            result = await self._dispatch(method, params)
            return {"id": request_id, "ok": True, "result": result}
        except BackendInvokeError as exc:
            return self._error_response(self._safe_request_id(request), exc.error)
        except Exception as exc:
            return self._error_response(
                self._safe_request_id(request),
                RuntimeError(
                    code=ERR_RUNTIME_BACKEND_FAILED,
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
            try:
                request = json.loads(line)
            except json.JSONDecodeError as exc:
                response = self._error_response(
                    None,
                    RuntimeError(
                        code=ERR_RUNTIME_BACKEND_FAILED,
                        source="python_stdio_jsonl",
                        route="python.stdio.decode",
                        evidence={"exception_repr": str(exc)},
                    ),
                )
            else:
                response = await self.handle_request(request)
            output_stream.write(json.dumps(response, separators=(",", ":"), ensure_ascii=False))
            output_stream.write("\n")
            output_stream.flush()

    async def _dispatch(self, method: str, params: dict[str, object]) -> JsonValue:
        if method == "on_awake":
            await self._host.on_awake(self._str_param(params, "agent_id"))
            return None
        if method == "on_input":
            result = await self._host.on_input(
                self._str_param(params, "agent_id"),
                from_json_dict(Envelope, self._mapping_param(params, "envelope")),
            )
            return to_json_dict(result)
        if method == "next_step":
            result = await self._host.next_step(self._str_param(params, "agent_id"))
            return to_json_dict(result)
        if method == "on_stop":
            await self._host.on_stop(self._str_param(params, "agent_id"))
            return None
        if method == "list_operations":
            agent_id = self._str_param(params, "agent_id")
            return [to_json_dict(item) for item in self._host.list_operations(agent_id)]
        if method == "list_sources":
            agent_id = self._str_param(params, "agent_id")
            return [to_json_dict(item) for item in self._host.list_sources(agent_id)]
        if method == "invoke":
            result = await self._host.invoke(
                self._str_param(params, "agent_id"),
                from_json_dict(OperationHandlerKey, self._mapping_param(params, "key")),
                self._json_param(params, "payload", None),
            )
            return result
        if method == "operation_status":
            status = self._host.operation_status(
                self._str_param(params, "agent_id"),
                from_json_dict(OperationHandlerKey, self._mapping_param(params, "key")),
            )
            return status.value
        if method == "resource.register":
            return await self._resource_backend.register_resource(
                from_json_dict(RefDescriptor, self._mapping_param(params, "descriptor")),
                self._str_param(params, "owner"),
            )
        if method == "resource.acquire":
            token = await self._resource_backend.acquire_resource(
                self._str_param(params, "ref_id"),
                self._str_param(params, "requester"),
            )
            return to_json_dict(token)
        if method == "resource.release":
            await self._resource_backend.release_resource(
                from_json_dict(LeaseToken, self._mapping_param(params, "token"))
            )
            return None
        if method == "resource.list":
            owner = params.get("owner")
            if owner is not None and not isinstance(owner, str):
                raise TypeError("owner expects str or null")
            return [to_json_dict(item) for item in self._resource_backend.list_records(owner)]
        raise BackendInvokeError(
            RuntimeError(
                code=ERR_RUNTIME_BACKEND_FAILED,
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
        request_id = request.get("id")
        if not isinstance(request_id, str):
            raise TypeError("id expects str")
        return request_id

    @staticmethod
    def _safe_request_id(request: object) -> str | None:
        if isinstance(request, dict) and isinstance(request.get("id"), str):
            return request["id"]
        return None

    @staticmethod
    def _method(request: dict[str, object]) -> str:
        method = request.get("method")
        if not isinstance(method, str):
            raise TypeError("method expects str")
        return method

    @staticmethod
    def _params(request: dict[str, object]) -> dict[str, object]:
        params = request.get("params", {})
        if not isinstance(params, dict):
            raise TypeError("params expects mapping")
        return params

    @staticmethod
    def _str_param(params: dict[str, object], key: str) -> str:
        value = params.get(key)
        if not isinstance(value, str):
            raise TypeError(f"{key} expects str")
        return value

    @staticmethod
    def _mapping_param(params: dict[str, object], key: str) -> dict[str, object]:
        value = params.get(key)
        if not isinstance(value, dict):
            raise TypeError(f"{key} expects mapping")
        return value

    @staticmethod
    def _json_param(params: dict[str, object], key: str, default: JsonValue = None) -> JsonValue:
        value = params.get(key, default)
        return _as_json_value(value)

    @staticmethod
    def _error_response(request_id: str | None, error: RuntimeError) -> dict[str, JsonValue]:
        return {"id": request_id, "ok": False, "error": to_json_dict(error)}


def run_stdio_server(
    host: PythonBackendHost,
    input_stream: TextIO,
    output_stream: TextIO,
    resource_backend: PythonResourceBackend | None = None,
) -> None:
    asyncio.run(StdioJsonlBackendServer(host, resource_backend).serve(input_stream, output_stream))


def _as_json_value(value: Any) -> JsonValue:
    if value is None or isinstance(value, bool | int | float | str):
        return value
    if isinstance(value, list):
        return [_as_json_value(item) for item in value]
    if isinstance(value, tuple):
        return [_as_json_value(item) for item in value]
    if isinstance(value, dict):
        return {str(key): _as_json_value(item) for key, item in value.items()}
    raise TypeError(f"value is not JSON serializable: {type(value).__qualname__}")


__all__ = ["StdioJsonlBackendServer", "run_stdio_server"]
