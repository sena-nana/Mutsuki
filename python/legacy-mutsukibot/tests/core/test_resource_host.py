"""v0.3: ResourceHost and resource leases."""

from __future__ import annotations

from typing import cast

import pytest

from mutsukibot.contracts import CapabilityName, RefId, SpanStatus
from mutsukibot.contracts.error import Errs
from mutsukibot.contracts.ids import AgentId, SpanId, TraceId
from mutsukibot.contracts.resource_host import (
    ResourceHostPolicyConfig,
    ResourceRecordSelector,
)
from mutsukibot.core.bus import Bus
from mutsukibot.core.container import ServiceContainer
from mutsukibot.core.context import AgentContext, TraceContext
from mutsukibot.core.dispatcher import Dispatcher
from mutsukibot.core.resource_host import (
    CapabilityExhaustedError,
    ResourceHandleNotFoundError,
    ResourceHost,
    ResourcePolicyConfigError,
    ResourcePolicyConflictError,
)
from mutsukibot.core.scope import PluginScope
from mutsukibot.runtime import DeterministicIdGen, SeededRng, SystemClock


def _ctx() -> AgentContext:
    return AgentContext(
        agent_id=AgentId("resource-agent"),
        agent_owner=None,
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(0),
        services=ServiceContainer(),
        scope=PluginScope("resource-agent"),
        bus=Bus(),
        dispatch=cast(Dispatcher, None),
        trace_ctx=TraceContext(trace_id=TraceId("trace-1"), span_id=SpanId("root")),
    )


@pytest.mark.asyncio
async def test_handle_survives_plugin_scope_until_resource_host_closes() -> None:
    finalized: list[str] = []
    host = ResourceHost(owner="test-host")
    plugin_scope = PluginScope("test-plugin")

    handle = host.create_handle(
        RefId("resource-1"),
        target={"value": 1},
        kind="test.resource",
        schema_id_target="test.resource",
        schema_version_target="1.0.0",
        finalizer=lambda target: finalized.append(str(target["value"])),
    )
    assert handle.is_alive()

    # Plugin reload/unload drops plugin-local state, but ResourceHost still owns the resource.
    await plugin_scope.close()
    assert handle.is_alive()

    await host.close()
    assert not handle.is_alive()
    assert finalized == ["1"]


@pytest.mark.asyncio
async def test_resource_lease_enforces_capacity_and_releases() -> None:
    cap = CapabilityName.register("test.resource.capacity", declared_by="tests")
    host = ResourceHost(owner="test-host")
    host.declare_capacity(cap, total=2)

    lease1 = host.acquire(cap, amount=1, owner="a")
    lease2 = host.acquire(cap, amount=1, owner="b")

    with pytest.raises(CapabilityExhaustedError) as exc:
        host.acquire(cap, amount=1, owner="c")

    assert exc.value.error.code == Errs.CAPABILITY_EXHAUSTED
    assert exc.value.error.evidence["available"] == 0

    lease1.release()
    lease3 = host.acquire(cap, amount=1, owner="c")

    assert lease2.alive
    assert lease3.alive
    await host.close()
    assert not lease2.alive
    assert not lease3.alive


@pytest.mark.asyncio
async def test_resource_host_resolves_handles_and_reports_kind_mismatch() -> None:
    host = ResourceHost(owner="test-host")
    host.create_handle(
        RefId("resource-1"),
        target={"value": 1},
        kind="test.resource",
        schema_id_target="test.resource",
        schema_version_target="1.0.0",
    )

    handle = host.get_handle(RefId("resource-1"), kind="test.resource")
    assert handle.descriptor.kind == "test.resource"

    with pytest.raises(ResourceHandleNotFoundError) as exc:
        host.get_handle(RefId("resource-1"), kind="test.other")
    assert exc.value.error.code == Errs.REF_KIND_MISMATCH
    assert exc.value.error.evidence["actual_kind"] == "test.resource"

    await host.close()


@pytest.mark.asyncio
async def test_resource_host_eviction_and_keepalive_policies() -> None:
    finalized: list[str] = []
    host = ResourceHost(
        owner="test-host",
        eviction_policy=lambda record: record.kind == "test.stale",
        keepalive_policy=lambda record: record.ref_id != "dead",
    )
    stale = host.create_handle(
        RefId("stale"),
        target="stale",
        kind="test.stale",
        schema_id_target="test.resource",
        schema_version_target="1.0.0",
        finalizer=finalized.append,
    )
    live = host.create_handle(
        RefId("live"),
        target="live",
        kind="test.live",
        schema_id_target="test.resource",
        schema_version_target="1.0.0",
        finalizer=finalized.append,
    )
    dead = host.create_handle(
        RefId("dead"),
        target="dead",
        kind="test.live",
        schema_id_target="test.resource",
        schema_version_target="1.0.0",
        finalizer=finalized.append,
    )

    assert host.evict() == (RefId("stale"),)
    assert not stale.is_alive()
    assert live.is_alive()
    assert dead.is_alive()

    assert await host.keepalive() == (RefId("dead"),)
    assert live.is_alive()
    assert not dead.is_alive()
    assert finalized == ["stale", "dead"]

    await host.close()
    assert finalized == ["stale", "dead", "live"]


@pytest.mark.asyncio
async def test_resource_host_policy_config_drives_eviction_and_keepalive() -> None:
    finalized: list[str] = []
    host = ResourceHost(
        owner="test-host",
        policy_config={
            "eviction": {"kind": "test.stale"},
            "keepalive": {"ref_id": "dead", "invert": True},
        },
    )
    stale = host.create_handle(
        RefId("stale"),
        target="stale",
        kind="test.stale",
        schema_id_target="test.resource",
        schema_version_target="1.0.0",
        finalizer=finalized.append,
    )
    live = host.create_handle(
        RefId("live"),
        target="live",
        kind="test.live",
        schema_id_target="test.resource",
        schema_version_target="1.0.0",
        finalizer=finalized.append,
    )
    dead = host.create_handle(
        RefId("dead"),
        target="dead",
        kind="test.live",
        schema_id_target="test.resource",
        schema_version_target="1.0.0",
        finalizer=finalized.append,
    )

    assert host.policy_config == ResourceHostPolicyConfig(
        eviction=ResourceRecordSelector(kind="test.stale"),
        keepalive=ResourceRecordSelector(ref_id=RefId("dead"), invert=True),
    )
    assert host.evict() == (RefId("stale"),)
    assert not stale.is_alive()
    assert live.is_alive()
    assert dead.is_alive()

    assert await host.keepalive() == (RefId("dead"),)
    assert live.is_alive()
    assert not dead.is_alive()
    assert finalized == ["stale", "dead"]

    await host.close()
    assert finalized == ["stale", "dead", "live"]


@pytest.mark.asyncio
async def test_resource_host_policy_config_rejects_unknown_keys() -> None:
    with pytest.raises(ResourcePolicyConfigError) as exc:
        ResourceHost(
            owner="test-host",
            policy_config={
                "eviction": {"kind": "test.stale", "unknown": "boom"},
            },
        )

    assert exc.value.error.code == Errs.RESOURCE_POLICY_INVALID
    assert exc.value.error.evidence["policy"] == "eviction"
    assert exc.value.error.evidence["unknown_keys"] == "unknown"


@pytest.mark.asyncio
async def test_resource_host_policy_config_conflicts_with_callable_override() -> None:
    with pytest.raises(ResourcePolicyConflictError) as exc:
        ResourceHost(
            owner="test-host",
            policy_config={"eviction": {"kind": "test.stale"}},
            eviction_policy=lambda record: True,
        )

    assert exc.value.error.code == Errs.RESOURCE_POLICY_CONFLICT
    assert exc.value.error.evidence["policy"] == "eviction"


@pytest.mark.asyncio
async def test_resource_host_acquire_and_release_emit_trace_spans() -> None:
    cap = CapabilityName.register("test.resource.trace", declared_by="tests")
    ctx = _ctx()
    spans: list[object] = []

    async def collect(payload: object) -> None:
        spans.append(payload)

    ctx.bus.subscribe("trace.span", collect, direct=True)
    host = ResourceHost(owner="test-host")
    host.declare_capacity(cap, total=1)

    lease = await host.acquire_for(ctx, cap, amount=1, owner="plugin-a")
    await host.release_for(ctx, lease)

    assert [getattr(span, "name") for span in spans] == [
        "resource_host.acquire",
        "resource_host.release",
    ]
    assert all(getattr(span, "status") == SpanStatus.OK for span in spans)
    assert getattr(spans[0], "attributes")["capability"] == str(cap)
    assert getattr(spans[1], "attributes")["owner"] == "plugin-a"

    await host.close()
