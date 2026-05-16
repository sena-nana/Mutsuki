"""v0.3: ResourceHost and resource leases."""

from __future__ import annotations

import pytest

from mutsukibot.contracts import CapabilityName, RefId
from mutsukibot.contracts.error import Errs
from mutsukibot.core.resource_host import CapabilityExhaustedError, ResourceHost
from mutsukibot.core.scope import PluginScope


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
