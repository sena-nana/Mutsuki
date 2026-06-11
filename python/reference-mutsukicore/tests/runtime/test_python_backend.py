from __future__ import annotations

from typing import ClassVar

import msgspec
import pytest

from mutsukicore import Capability, Caps, Perms, Plugin, command
from mutsukicore.contracts import BySchema, Envelope, SourceKinds, SourceRef
from mutsukicore.contracts.error import Errs
from mutsukicore.contracts.event import SpanStatus, TraceSpan
from mutsukicore.contracts.ids import AgentId, EnvelopeId, RefId
from mutsukicore.contracts.refpayload import RefDescriptor
from mutsukicore.core.agent import Agent
from mutsukicore.core.loader import PluginLoader
from mutsukicore.runtime import (
    BackendInvokeError,
    DeterministicIdGen,
    LeaseToken,
    PythonAgentBackend,
    PythonResourceBackend,
    SeededRng,
    SystemClock,
)


class _Config(msgspec.Struct, kw_only=True):
    pass


class _BackendPlugin(Plugin[_Config]):
    id: ClassVar[str] = "test-python-backend"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [
        Capability(name=Caps.READ_MESSAGE),
        Capability(name=Caps.SEND_MESSAGE),
    ]
    Config = _Config

    @command(perms=Perms.PUBLIC)
    async def ping(self, value: str = "pong") -> str:
        return value

    @command(perms=Perms.PUBLIC)
    async def crash(self) -> str:
        raise ValueError("boom")


class _BackendConsumerPlugin(Plugin[_Config]):
    id: ClassVar[str] = "test-python-backend-consumer"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [Capability(name=Caps.READ_MESSAGE)]
    consumes: ClassVar[tuple] = (BySchema("test.backend.input"),)
    Config = _Config
    received: ClassVar[list[Envelope]] = []

    async def on_envelope(self, envelope: Envelope) -> None:
        type(self).received.append(envelope)


class _BackendCrashConsumerPlugin(Plugin[_Config]):
    id: ClassVar[str] = "test-python-backend-crash-consumer"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [Capability(name=Caps.READ_MESSAGE)]
    consumes: ClassVar[tuple] = (BySchema("test.backend.input"),)
    Config = _Config

    async def on_envelope(self, envelope: Envelope) -> None:
        raise RuntimeError("backend consumer boom")


def _new_agent() -> Agent:
    return Agent(
        agent_id=AgentId("py-backend-agent"),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(seed=0),
    )


def _backend_envelope() -> Envelope:
    return Envelope(
        id=EnvelopeId("env-backend-1"),
        timestamp=0.0,
        source=SourceRef(
            source_id="backend:test",
            kind=SourceKinds.IM,
        ),
        payload_schema_id="test.backend.input",
    )


@pytest.mark.asyncio
async def test_python_backend_operation_snapshot_is_msgspec_serializable() -> None:
    agent = _new_agent()
    loader = PluginLoader(allow={_BackendPlugin.id})
    await loader.load_into(agent, [_BackendPlugin])
    backend = PythonAgentBackend({agent.agent_id: agent})

    snapshots = backend.list_operations(agent.agent_id)
    assert len(snapshots) == 2
    snapshot = next(item for item in snapshots if item.key.op_id == "test-python-backend.ping")
    encoded = msgspec.json.encode(snapshot)
    decoded = msgspec.json.decode(encoded)
    assert decoded["key"]["op_id"] == "test-python-backend.ping"
    assert "handler" not in decoded

    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_python_backend_invokes_operation_by_indirect_key() -> None:
    agent = _new_agent()
    loader = PluginLoader(allow={_BackendPlugin.id})
    await loader.load_into(agent, [_BackendPlugin])
    backend = PythonAgentBackend({agent.agent_id: agent})
    snapshot = next(
        item
        for item in backend.list_operations(agent.agent_id)
        if item.key.op_id == "test-python-backend.ping"
    )

    result = await backend.invoke(
        agent.agent_id,
        snapshot.key,
        {"value": "hello"},
    )

    assert result == "hello"
    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_python_backend_stale_generation_key_fails_loud_after_reload() -> None:
    agent = _new_agent()
    loader = PluginLoader(allow={_BackendPlugin.id})
    await loader.load_into(agent, [_BackendPlugin])
    backend = PythonAgentBackend({agent.agent_id: agent})
    stale_key = next(
        item
        for item in backend.list_operations(agent.agent_id)
        if item.key.op_id == "test-python-backend.ping"
    ).key
    await loader.unload_from(agent)
    await loader.load_into(agent, [_BackendPlugin])

    with pytest.raises(BackendInvokeError) as exc:
        await backend.invoke(agent.agent_id, stale_key, {"value": "stale"})

    assert exc.value.error.code == Errs.RUNTIME_BACKEND_GENERATION_MISMATCH
    assert exc.value.error.evidence["expected_generation"] == 0
    assert exc.value.error.evidence["actual_generation"] == 1

    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_python_backend_generation_advances_once_per_plugin_reload() -> None:
    agent = _new_agent()
    loader = PluginLoader(allow={_BackendPlugin.id})
    backend = PythonAgentBackend({agent.agent_id: agent})
    generations: list[int] = []

    for _ in range(3):
        await loader.load_into(agent, [_BackendPlugin])
        key = next(
            item
            for item in backend.list_operations(agent.agent_id)
            if item.key.op_id == "test-python-backend.ping"
        ).key
        generations.append(key.plugin_generation)
        await loader.unload_from(agent)

    assert generations == [0, 1, 2]


@pytest.mark.asyncio
async def test_python_backend_wraps_operation_snapshot_failure(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    agent = _new_agent()
    loader = PluginLoader(allow={_BackendPlugin.id})
    await loader.load_into(agent, [_BackendPlugin])
    backend = PythonAgentBackend({agent.agent_id: agent})

    def fail_snapshot() -> object:
        raise RuntimeError("snapshot failed")

    monkeypatch.setattr(agent.dispatch, "list_operation_snapshots", fail_snapshot)

    with pytest.raises(BackendInvokeError) as exc:
        backend.list_operations(agent.agent_id)

    assert exc.value.error.code == Errs.RUNTIME_BACKEND_FAILED
    assert exc.value.error.route == "runtime.backend.list_operations"
    assert exc.value.error.evidence["exception_type"] == "RuntimeError"
    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_python_backend_wraps_operation_invoke_error() -> None:
    agent = _new_agent()
    loader = PluginLoader(allow={_BackendPlugin.id})
    await loader.load_into(agent, [_BackendPlugin])
    backend = PythonAgentBackend({agent.agent_id: agent})
    snapshot = next(
        item
        for item in backend.list_operations(agent.agent_id)
        if item.key.op_id == "test-python-backend.crash"
    )

    with pytest.raises(BackendInvokeError) as exc:
        await backend.invoke(agent.agent_id, snapshot.key, {})

    assert exc.value.error.code == Errs.OPERATION_HANDLER_RAISED
    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_python_backend_on_input_uses_shared_consumer_fanout() -> None:
    _BackendConsumerPlugin.received = []
    agent = _new_agent()
    loader = PluginLoader(
        allow={_BackendConsumerPlugin.id, _BackendCrashConsumerPlugin.id}
    )
    await loader.load_into(
        agent,
        [_BackendCrashConsumerPlugin, _BackendConsumerPlugin],
    )
    backend = PythonAgentBackend({agent.agent_id: agent})
    spans: list[TraceSpan] = []

    async def _on_span(payload: object) -> None:
        if isinstance(payload, TraceSpan):
            spans.append(payload)

    agent.bus.subscribe("trace.span", _on_span)
    result = await backend.on_input(agent.agent_id, _backend_envelope())

    assert result.status.value == "wait_input"
    assert len(_BackendConsumerPlugin.received) == 1
    crash_spans = [
        span
        for span in spans
        if span.name == f"plugin.{_BackendCrashConsumerPlugin.id}.on_envelope"
    ]
    consumer_spans = [
        span
        for span in spans
        if span.name == f"plugin.{_BackendConsumerPlugin.id}.on_envelope"
    ]
    assert len(crash_spans) == 1
    assert crash_spans[0].status == SpanStatus.ERROR
    assert crash_spans[0].attributes["exception_type"] == "RuntimeError"
    assert len(consumer_spans) == 1
    assert consumer_spans[0].status == SpanStatus.OK

    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_python_resource_backend_tracks_descriptor_leases_without_handle() -> None:
    backend = PythonResourceBackend()
    descriptor = RefDescriptor(
        ref_id=RefId("ref-1"),
        kind="domain.resource",
        schema_id_target="domain.resource",
        schema_version_target="1.0.0",
    )

    ref_id = await backend.register(descriptor, owner="resource-host")
    lease = await backend.acquire(ref_id, requester="agent-a")
    records = backend.list_records()

    assert lease.ref_id == ref_id
    assert not lease.token_id.startswith("lease-1")
    assert records[0].descriptor == descriptor
    assert records[0].lease_count == 1
    encoded = msgspec.json.encode(records[0])
    decoded = msgspec.json.decode(encoded)
    assert "handle" not in decoded

    await backend.release(lease)
    assert backend.list_records()[0].lease_count == 0


@pytest.mark.asyncio
async def test_python_resource_backend_unknown_ref_fails_structured() -> None:
    backend = PythonResourceBackend()

    with pytest.raises(BackendInvokeError) as exc:
        await backend.acquire(RefId("missing"), requester="agent-a")

    assert exc.value.error.code == Errs.REF_NOT_FOUND


@pytest.mark.asyncio
async def test_python_resource_backend_rejects_forged_lease_token() -> None:
    backend = PythonResourceBackend()
    descriptor = RefDescriptor(
        ref_id=RefId("ref-1"),
        kind="domain.resource",
        schema_id_target="domain.resource",
        schema_version_target="1.0.0",
    )
    ref_id = await backend.register(descriptor, owner="resource-host")
    lease = await backend.acquire(ref_id, requester="agent-a")
    forged = LeaseToken(
        token_id=lease.token_id,
        ref_id=RefId("ref-other"),
        owner="agent-b",
    )

    with pytest.raises(BackendInvokeError) as exc:
        await backend.release(forged)

    assert exc.value.error.code == Errs.REF_NOT_FOUND
    assert exc.value.error.evidence["reason"] == "lease_token_mismatch"
    assert backend.list_records()[0].lease_count == 1

    await backend.release(lease)
    assert backend.list_records()[0].lease_count == 0
