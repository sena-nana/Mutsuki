from __future__ import annotations

from typing import Protocol

from mutsukicore_runtime_python.contracts import (
    Envelope,
    JsonValue,
    LeaseToken,
    OperationHandlerKey,
    OperationSnapshot,
    OperationStatus,
    PluginSnapshot,
    RefDescriptor,
    ResourceRecord,
    RuntimeError,
    SourceSnapshot,
    StrategyResult,
)


class BackendInvokeError(Exception):
    """Structured backend failure wrapper."""

    def __init__(self, error: RuntimeError) -> None:
        super().__init__(f"runtime backend failed: {error.code}")
        self.error = error


class StrategyBackend(Protocol):
    async def on_awake(self, agent_id: str) -> None: ...

    async def on_input(self, agent_id: str, envelope: Envelope) -> StrategyResult: ...

    async def next_step(self, agent_id: str) -> StrategyResult: ...

    async def on_stop(self, agent_id: str) -> None: ...


class OperationBackend(Protocol):
    def list_plugins(self) -> tuple[PluginSnapshot, ...]: ...

    def list_operations(
        self, enabled_plugin_ids: tuple[str, ...] | list[str]
    ) -> tuple[OperationSnapshot, ...]: ...

    def list_sources(
        self, enabled_plugin_ids: tuple[str, ...] | list[str]
    ) -> tuple[SourceSnapshot, ...]: ...

    async def invoke(
        self,
        agent_id: str,
        key: OperationHandlerKey,
        payload: JsonValue = None,
    ) -> JsonValue: ...

    def operation_status(self, agent_id: str, key: OperationHandlerKey) -> OperationStatus: ...


class ResourceBackend(Protocol):
    async def register_resource(self, descriptor: RefDescriptor, owner: str) -> str: ...

    async def acquire_resource(self, ref_id: str, requester: str) -> LeaseToken: ...

    async def release_resource(self, token: LeaseToken) -> None: ...

    def list_records(self, owner: str | None = None) -> tuple[ResourceRecord, ...]: ...
