"""Runtime backend boundary for Rust/Python split experiments.

This module deliberately contains protocols and serializable snapshots only.
It does not call Rust and it does not expose Python callables across the
boundary. A Rust runtime may keep the snapshot keys and route execution back
to the Python backend by indirection.
"""

from __future__ import annotations

from collections.abc import Awaitable, Callable
from enum import StrEnum
from typing import Any, ClassVar, Protocol, TypeVar
from uuid import uuid4

from mutsukibot.contracts.agent_profile import StrategyResult, StrategyResultStatus
from mutsukibot.contracts.base import Contract
from mutsukibot.contracts.envelope import Envelope
from mutsukibot.contracts.error import Error, Errs
from mutsukibot.contracts.event import SpanStatus
from mutsukibot.contracts.ids import AgentId, RefId
from mutsukibot.contracts.operation import OperationDescriptor
from mutsukibot.contracts.refpayload import RefDescriptor
from mutsukibot.contracts.source import SourceDescriptor
from mutsukibot.core.dispatcher import OperationInvokeError

_T = TypeVar("_T")


class BackendOperationStatus(StrEnum):
    ACTIVE = "active"
    UNHEALTHY = "unhealthy"
    UNREGISTERING = "unregistering"
    NOT_FOUND = "not_found"


class BackendInvokeError(Exception):
    """Structured error raised by runtime backend adapters."""

    def __init__(self, error: Error) -> None:
        super().__init__(f"runtime backend failed: {error.code}")
        self.error = error


def _backend_failed_error(
    *,
    route: str,
    evidence: dict[str, str | int | float | bool],
) -> Error:
    return Error(
        code=Errs.RUNTIME_BACKEND_FAILED,
        source="runtime.python_backend",
        route=route,
        evidence=evidence,
    )


def _call_backend_boundary(
    route: str,
    *,
    agent_id: AgentId,
    fn: Callable[[], _T],
) -> _T:
    try:
        return fn()
    except BackendInvokeError:
        raise
    except Exception as exc:
        raise BackendInvokeError(
            _backend_failed_error(
                route=route,
                evidence={
                    "agent_id": str(agent_id),
                    "exception_type": type(exc).__qualname__,
                    "exception_repr": repr(exc),
                },
            )
        ) from exc


class OperationHandlerKey(Contract):
    """Serializable indirection key for a Python Operation handler.

    It is never routed as an inbound envelope.
    """

    schema_id: ClassVar[str] = "mutsukibot.runtime.operation_handler_key"
    schema_version: ClassVar[str] = "1.0.0"

    plugin_id: str
    plugin_generation: int
    op_id: str
    handler_id: str


class OperationSnapshot(Contract):
    """Serializable Operation metadata visible to an external runtime."""

    schema_id: ClassVar[str] = "mutsukibot.runtime.operation_snapshot"
    schema_version: ClassVar[str] = "1.0.0"

    descriptor: OperationDescriptor
    status: BackendOperationStatus
    key: OperationHandlerKey


class SourceSnapshot(Contract):
    """Serializable Source metadata visible to an external runtime."""

    schema_id: ClassVar[str] = "mutsukibot.runtime.source_snapshot"
    schema_version: ClassVar[str] = "1.0.0"

    descriptor: SourceDescriptor
    plugin_id: str
    plugin_generation: int


class LeaseToken(Contract):
    """Serializable resource lease token.

    The token names a lease. It is not a Python ``Handle`` and must not expose
    the actual referenced object.
    """

    schema_id: ClassVar[str] = "mutsukibot.runtime.lease_token"
    schema_version: ClassVar[str] = "1.0.0"

    token_id: str
    ref_id: RefId
    owner: str


class ResourceSnapshot(Contract):
    """Serializable resource governance record."""

    schema_id: ClassVar[str] = "mutsukibot.runtime.resource_snapshot"
    schema_version: ClassVar[str] = "1.0.0"

    descriptor: RefDescriptor
    owner: str
    lease_count: int = 0


class StrategyBackend(Protocol):
    def on_awake(self, agent_id: AgentId) -> Awaitable[None]: ...

    def on_input(
        self,
        agent_id: AgentId,
        envelope: Envelope,
    ) -> Awaitable[StrategyResult]: ...

    def next_step(self, agent_id: AgentId) -> Awaitable[StrategyResult]: ...

    def on_stop(self, agent_id: AgentId) -> Awaitable[None]: ...


class OperationBackend(Protocol):
    def list_operations(self, agent_id: AgentId) -> tuple[OperationSnapshot, ...]: ...

    def invoke(
        self,
        agent_id: AgentId,
        key: OperationHandlerKey,
        payload: dict[str, Any] | None = None,
    ) -> Awaitable[Any]: ...

    def operation_status(
        self,
        agent_id: AgentId,
        key: OperationHandlerKey,
    ) -> BackendOperationStatus: ...


class ResourceBackend(Protocol):
    def register(
        self,
        descriptor: RefDescriptor,
        *,
        owner: str,
    ) -> Awaitable[RefId]: ...

    def acquire(
        self,
        ref_id: RefId,
        *,
        requester: str,
    ) -> Awaitable[LeaseToken]: ...

    def release(self, token: LeaseToken) -> Awaitable[None]: ...

    def list_records(self, owner: str | None = None) -> tuple[ResourceSnapshot, ...]: ...


class PythonAgentBackend:
    """Adapter that exposes existing Python Agents as runtime backends.

    The adapter is intentionally small: it delegates operation execution to the
    existing dispatcher and keeps lifecycle/strategy hooks in Python. It is a
    bridge for a future Rust runtime, not a replacement for ``PluginLoader``.
    """

    def __init__(self, agents: dict[AgentId, Any] | None = None) -> None:
        self._agents: dict[AgentId, Any] = dict(agents or {})

    def register_agent(self, agent: Any) -> None:
        self._agents[getattr(agent, "agent_id")] = agent

    def _agent(self, agent_id: AgentId) -> Any:
        agent = self._agents.get(agent_id)
        if agent is None:
            err = Error(
                code=Errs.AGENT_NOT_FOUND,
                source="runtime.python_backend",
                route=f"runtime.backend.agent.{agent_id}",
                evidence={"agent_id": str(agent_id)},
            )
            raise BackendInvokeError(err)
        return agent

    async def on_awake(self, agent_id: AgentId) -> None:
        agent = self._agent(agent_id)
        ctx = agent.make_context()
        await agent.lifespan.fire("awake", ctx)

    async def on_input(
        self,
        agent_id: AgentId,
        envelope: Envelope,
    ) -> StrategyResult:
        agent = self._agent(agent_id)
        for entry in agent.plugins:
            plugin = entry.plugin
            consumes: tuple = plugin.__class__.consumes
            if not consumes or not any(rule.check(envelope) for rule in consumes):
                continue
            attributes: dict[str, str | int | float | bool] = {
                "agent_id": str(agent.agent_id),
                "envelope_id": str(envelope.id),
                "envelope_schema": envelope.payload_schema_id,
                "source_id": envelope.source.source_id,
            }
            ctx = agent.make_context()
            from mutsukibot.core.trace import trace_span

            async with trace_span(
                ctx,
                f"plugin.{plugin.id}.on_envelope",
                attributes=attributes,
            ) as span:
                try:
                    await plugin.on_envelope(envelope)
                except Exception as exc:
                    span.status = SpanStatus.ERROR
                    span.attributes["exception_type"] = type(exc).__qualname__
                    span.attributes["exception_repr"] = repr(exc)
        return StrategyResult(status=StrategyResultStatus.WAIT_INPUT)

    async def next_step(self, agent_id: AgentId) -> StrategyResult:
        self._agent(agent_id)
        return StrategyResult(status=StrategyResultStatus.WAIT_INPUT)

    async def on_stop(self, agent_id: AgentId) -> None:
        agent = self._agent(agent_id)
        ctx = agent.make_context()
        await agent.lifespan.fire("stop", ctx)

    def list_operations(self, agent_id: AgentId) -> tuple[OperationSnapshot, ...]:
        agent = self._agent(agent_id)
        return _call_backend_boundary(
            "runtime.backend.list_operations",
            agent_id=agent_id,
            fn=agent.dispatch.list_operation_snapshots,
        )

    def list_sources(self, agent_id: AgentId) -> tuple[SourceSnapshot, ...]:
        agent = self._agent(agent_id)
        return _call_backend_boundary(
            "runtime.backend.list_sources",
            agent_id=agent_id,
            fn=agent.dispatch.list_source_snapshots,
        )

    async def invoke(
        self,
        agent_id: AgentId,
        key: OperationHandlerKey,
        payload: dict[str, Any] | None = None,
    ) -> Any:
        agent = self._agent(agent_id)
        ctx = agent.make_context()
        try:
            return await agent.dispatch.invoke_with_backend_key(key, payload or {}, ctx=ctx)
        except BackendInvokeError:
            raise
        except OperationInvokeError as exc:
            raise BackendInvokeError(exc.error) from exc
        except Exception as exc:
            err = _backend_failed_error(
                route=f"runtime.backend.invoke.{key.op_id}",
                evidence={
                    "agent_id": str(agent_id),
                    "op_id": key.op_id,
                    "exception_type": type(exc).__qualname__,
                    "exception_repr": repr(exc),
                },
            )
            raise BackendInvokeError(err) from exc

    def operation_status(
        self,
        agent_id: AgentId,
        key: OperationHandlerKey,
    ) -> BackendOperationStatus:
        agent = self._agent(agent_id)
        for snapshot in agent.dispatch.list_operation_snapshots():
            if snapshot.key == key:
                return snapshot.status
        return BackendOperationStatus.NOT_FOUND


class PythonResourceBackend:
    """In-process resource governance backend for boundary tests.

    It mirrors the Rust ``ResourceGate`` first slice: only descriptors, owners,
    lease tokens, and lease counts are tracked. The actual object and finalizer
    remain with Python ``ResourceHost`` / ``Handle`` owners.
    """

    def __init__(self) -> None:
        self._records: dict[RefId, ResourceSnapshot] = {}
        self._leases: dict[str, LeaseToken] = {}

    async def register(
        self,
        descriptor: RefDescriptor,
        *,
        owner: str,
    ) -> RefId:
        self._records[descriptor.ref_id] = ResourceSnapshot(
            descriptor=descriptor,
            owner=owner,
            lease_count=0,
        )
        return descriptor.ref_id

    async def acquire(
        self,
        ref_id: RefId,
        *,
        requester: str,
    ) -> LeaseToken:
        record = self._records.get(ref_id)
        if record is None:
            err = Error(
                code=Errs.REF_NOT_FOUND,
                source="runtime.python_resource_backend",
                route=f"runtime.resource.acquire.{ref_id}",
                evidence={"ref_id": str(ref_id), "requester": requester},
            )
            raise BackendInvokeError(err)
        token = LeaseToken(
            token_id=f"lease-{uuid4()}",
            ref_id=ref_id,
            owner=requester,
        )
        self._leases[token.token_id] = token
        self._records[ref_id] = ResourceSnapshot(
            descriptor=record.descriptor,
            owner=record.owner,
            lease_count=record.lease_count + 1,
        )
        return token

    async def release(self, token: LeaseToken) -> None:
        stored = self._leases.get(token.token_id)
        if stored is None:
            err = Error(
                code=Errs.REF_NOT_FOUND,
                source="runtime.python_resource_backend",
                route=f"runtime.resource.release.{token.token_id}",
                evidence={"token_id": token.token_id, "ref_id": str(token.ref_id)},
            )
            raise BackendInvokeError(err)
        if stored != token:
            err = Error(
                code=Errs.REF_NOT_FOUND,
                source="runtime.python_resource_backend",
                route=f"runtime.resource.release.{token.token_id}",
                evidence={
                    "reason": "lease_token_mismatch",
                    "token_id": token.token_id,
                    "expected_ref_id": str(stored.ref_id),
                    "actual_ref_id": str(token.ref_id),
                    "expected_owner": stored.owner,
                    "actual_owner": token.owner,
                },
            )
            raise BackendInvokeError(err)
        stored = self._leases.pop(token.token_id)
        record = self._records.get(stored.ref_id)
        if record is None:
            return
        self._records[stored.ref_id] = ResourceSnapshot(
            descriptor=record.descriptor,
            owner=record.owner,
            lease_count=max(0, record.lease_count - 1),
        )

    def list_records(self, owner: str | None = None) -> tuple[ResourceSnapshot, ...]:
        records = self._records.values()
        if owner is not None:
            records = tuple(record for record in records if record.owner == owner)
        return tuple(sorted(records, key=lambda record: str(record.descriptor.ref_id)))


def generation_mismatch_error(
    *,
    plugin_id: str,
    op_id: str,
    expected: int,
    actual: int,
) -> Error:
    """Build the standard fail-loud error for stale handler keys."""

    return Error(
        code=Errs.RUNTIME_BACKEND_GENERATION_MISMATCH,
        source=plugin_id,
        route=f"runtime.backend.invoke.{op_id}",
        evidence={
            "plugin_id": plugin_id,
            "op_id": op_id,
            "expected_generation": expected,
            "actual_generation": actual,
        },
    )


__all__ = [
    "BackendInvokeError",
    "BackendOperationStatus",
    "LeaseToken",
    "OperationBackend",
    "OperationHandlerKey",
    "OperationSnapshot",
    "PythonAgentBackend",
    "PythonResourceBackend",
    "ResourceBackend",
    "ResourceSnapshot",
    "SourceSnapshot",
    "StrategyBackend",
    "generation_mismatch_error",
]
