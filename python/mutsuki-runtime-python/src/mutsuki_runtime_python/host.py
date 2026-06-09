from __future__ import annotations

import inspect
from collections.abc import Awaitable, Callable

from mutsuki_runtime_python.backend import BackendInvokeError, StrategyBackend
from mutsuki_runtime_python.contracts import (
    ERR_AGENT_NOT_FOUND,
    ERR_OPERATION_NOT_FOUND,
    ERR_RUNTIME_BACKEND_FAILED,
    ERR_RUNTIME_BACKEND_GENERATION_MISMATCH,
    Envelope,
    JsonValue,
    OperationDescriptor,
    OperationHandlerKey,
    OperationSnapshot,
    OperationStatus,
    RuntimeError,
    SourceSnapshot,
    StrategyResult,
)

OperationHandler = Callable[[JsonValue], JsonValue | Awaitable[JsonValue]]


class _WaitInputStrategy:
    async def on_awake(self, agent_id: str) -> None:
        _ = agent_id
        return None

    async def on_input(self, agent_id: str, envelope: Envelope) -> StrategyResult:
        _ = (agent_id, envelope)
        return StrategyResult.wait_input()

    async def next_step(self, agent_id: str) -> StrategyResult:
        _ = agent_id
        return StrategyResult.wait_input()

    async def on_stop(self, agent_id: str) -> None:
        _ = agent_id
        return None


class PythonBackendHost:
    """In-process Python backend host for Rust runtime integration tests."""

    def __init__(self) -> None:
        self._agents: set[str] = set()
        self._strategies: dict[str, StrategyBackend] = {}
        self._operations: dict[str, tuple[OperationSnapshot, OperationHandler]] = {}
        self._sources: list[SourceSnapshot] = []
        self._plugin_generations: dict[str, int] = {}
        self._received_inputs: dict[str, list[Envelope]] = {}
        self._awake_count: dict[str, int] = {}
        self._stop_count: dict[str, int] = {}

    def register_agent(self, agent_id: str, strategy: StrategyBackend | None = None) -> None:
        self._agents.add(agent_id)
        self._strategies[agent_id] = strategy or _WaitInputStrategy()
        self._received_inputs.setdefault(agent_id, [])
        self._awake_count.setdefault(agent_id, 0)
        self._stop_count.setdefault(agent_id, 0)

    def register_source(self, source: SourceSnapshot) -> None:
        self._sources.append(source)
        self._plugin_generations[source.plugin_id] = max(
            source.plugin_generation,
            self._plugin_generations.get(source.plugin_id, 0),
        )

    def register_operation(
        self,
        descriptor: OperationDescriptor,
        handler: OperationHandler,
        *,
        plugin_generation: int = 0,
        status: OperationStatus = OperationStatus.ACTIVE,
    ) -> OperationSnapshot:
        plugin_id = descriptor.plugin_id
        generation = max(plugin_generation, self._plugin_generations.get(plugin_id, 0))
        self._plugin_generations[plugin_id] = generation
        snapshot = OperationSnapshot(
            descriptor=descriptor,
            status=status,
            key=OperationHandlerKey(
                plugin_id=plugin_id,
                plugin_generation=generation,
                op_id=descriptor.op_id,
                handler_id=self._handler_id(plugin_id, descriptor.op_id, generation),
            ),
        )
        self._operations[descriptor.op_id] = (snapshot, handler)
        return snapshot

    def advance_plugin_generation(self, plugin_id: str) -> int:
        generation = self._plugin_generations.get(plugin_id, 0) + 1
        self._plugin_generations[plugin_id] = generation
        updated: dict[str, tuple[OperationSnapshot, OperationHandler]] = {}
        for op_id, (snapshot, handler) in self._operations.items():
            if snapshot.key.plugin_id != plugin_id:
                updated[op_id] = (snapshot, handler)
                continue
            key = OperationHandlerKey(
                plugin_id=plugin_id,
                plugin_generation=generation,
                op_id=snapshot.key.op_id,
                handler_id=self._handler_id(plugin_id, snapshot.key.op_id, generation),
            )
            updated[op_id] = (
                OperationSnapshot(
                    descriptor=snapshot.descriptor,
                    status=snapshot.status,
                    key=key,
                ),
                handler,
            )
        self._operations = updated
        return generation

    async def on_awake(self, agent_id: str) -> None:
        strategy = self._strategy(agent_id)
        await strategy.on_awake(agent_id)
        self._awake_count[agent_id] = self._awake_count.get(agent_id, 0) + 1

    async def on_input(self, agent_id: str, envelope: Envelope) -> StrategyResult:
        strategy = self._strategy(agent_id)
        self._received_inputs.setdefault(agent_id, []).append(envelope)
        return await strategy.on_input(agent_id, envelope)

    async def next_step(self, agent_id: str) -> StrategyResult:
        return await self._strategy(agent_id).next_step(agent_id)

    async def on_stop(self, agent_id: str) -> None:
        strategy = self._strategy(agent_id)
        await strategy.on_stop(agent_id)
        self._stop_count[agent_id] = self._stop_count.get(agent_id, 0) + 1

    def list_operations(self, agent_id: str) -> tuple[OperationSnapshot, ...]:
        self._ensure_agent(agent_id)
        return tuple(snapshot for snapshot, _handler in self._operations.values())

    def list_sources(self, agent_id: str) -> tuple[SourceSnapshot, ...]:
        self._ensure_agent(agent_id)
        return tuple(self._sources)

    async def invoke(
        self,
        agent_id: str,
        key: OperationHandlerKey,
        payload: JsonValue = None,
    ) -> JsonValue:
        self._ensure_agent(agent_id)
        operation = self._operations.get(key.op_id)
        if operation is None:
            raise BackendInvokeError(
                RuntimeError(
                    code=ERR_OPERATION_NOT_FOUND,
                    source="python_backend_host",
                    route=f"python.invoke.{key.op_id}",
                    evidence={"agent_id": agent_id, "op_id": key.op_id},
                )
            )
        snapshot, handler = operation
        if snapshot.key != key:
            raise BackendInvokeError(
                RuntimeError(
                    code=ERR_RUNTIME_BACKEND_GENERATION_MISMATCH,
                    source="python_backend_host",
                    route=f"python.invoke.{key.op_id}",
                    evidence={
                        "plugin_id": key.plugin_id,
                        "op_id": key.op_id,
                        "expected_generation": snapshot.key.plugin_generation,
                        "actual_generation": key.plugin_generation,
                    },
                )
            )
        try:
            result = handler(payload)
            if inspect.isawaitable(result):
                result = await result
            return self._as_json_value(result)
        except BackendInvokeError:
            raise
        except Exception as exc:
            raise BackendInvokeError(
                RuntimeError(
                    code=ERR_RUNTIME_BACKEND_FAILED,
                    source="python_backend_host",
                    route=f"python.invoke.{key.op_id}",
                    evidence={
                        "agent_id": agent_id,
                        "op_id": key.op_id,
                        "exception_type": type(exc).__qualname__,
                        "exception_repr": repr(exc),
                    },
                )
            ) from exc

    def operation_status(self, agent_id: str, key: OperationHandlerKey) -> OperationStatus:
        self._ensure_agent(agent_id)
        operation = self._operations.get(key.op_id)
        if operation is None:
            return OperationStatus.NOT_FOUND
        snapshot, _handler = operation
        if snapshot.key != key:
            return OperationStatus.NOT_FOUND
        return snapshot.status

    def received_inputs(self, agent_id: str) -> tuple[Envelope, ...]:
        self._ensure_agent(agent_id)
        return tuple(self._received_inputs.get(agent_id, ()))

    def awake_count(self, agent_id: str) -> int:
        self._ensure_agent(agent_id)
        return self._awake_count.get(agent_id, 0)

    def stop_count(self, agent_id: str) -> int:
        self._ensure_agent(agent_id)
        return self._stop_count.get(agent_id, 0)

    def _strategy(self, agent_id: str) -> StrategyBackend:
        self._ensure_agent(agent_id)
        return self._strategies[agent_id]

    def _ensure_agent(self, agent_id: str) -> None:
        if agent_id in self._agents:
            return
        raise BackendInvokeError(
            RuntimeError(
                code=ERR_AGENT_NOT_FOUND,
                source="python_backend_host",
                route=f"python.agent.{agent_id}",
                evidence={"agent_id": agent_id},
            )
        )

    @staticmethod
    def _handler_id(plugin_id: str, op_id: str, generation: int) -> str:
        return f"{plugin_id}:{op_id}:{generation}"

    @staticmethod
    def _as_json_value(value: object) -> JsonValue:
        if value is None or isinstance(value, bool | int | float | str):
            return value
        if isinstance(value, list):
            return [PythonBackendHost._as_json_value(item) for item in value]
        if isinstance(value, tuple):
            return [PythonBackendHost._as_json_value(item) for item in value]
        if isinstance(value, dict):
            return {str(key): PythonBackendHost._as_json_value(item) for key, item in value.items()}
        raise TypeError(f"handler returned non-JSON value: {type(value).__qualname__}")
