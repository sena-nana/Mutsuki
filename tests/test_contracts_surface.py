from __future__ import annotations

from mutsuki_runner_kit.contracts.surface import (
    SurfaceOccupancy,
    SurfaceOccupancyHandle,
    SurfaceOccupancyHandleKind,
)
from mutsuki_runner_kit.testing.assertions import assert_json_roundtrip


def test_surface_occupancy_roundtrips() -> None:
    occupancy = SurfaceOccupancy(
        surface_id="runner:runner-a",
        ready_tasks=0,
        running_invocations=0,
        resource_refs=0,
        state_refs=0,
        active_leases=0,
        open_streams=0,
        subscriptions=0,
        timers=0,
        effect_inflight=0,
    )
    handle = SurfaceOccupancyHandle(
        handle_id="timer:heartbeat:1",
        surface_id="timer:heartbeat",
        owner_plugin_id="plugin-a",
        plugin_generation=2,
        registry_generation=7,
        kind=SurfaceOccupancyHandleKind.TIMER,
    )

    assert occupancy.is_zero()
    assert_json_roundtrip(SurfaceOccupancy, occupancy)
    assert_json_roundtrip(SurfaceOccupancyHandle, handle)
