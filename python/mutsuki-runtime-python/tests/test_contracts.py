from __future__ import annotations

import pytest

from mutsuki_runtime_python.contracts import (
    ResourceAccess,
    ResourceLifetime,
    ResourceRef,
    ResourceSealState,
    RunnerDescriptor,
    RunnerPurity,
    RunnerResult,
    SurfaceOccupancyHandle,
    SurfaceOccupancyHandleKind,
    Task,
    ValueRef,
    ValueStorage,
    VersionExpectation,
    from_json_dict,
    to_json_dict,
)
from mutsuki_runtime_python.testing import assert_json_roundtrip


def test_task_and_runner_descriptor_roundtrip() -> None:
    task = Task(
        task_id="task-1",
        kind="raw.input",
        priority=10,
        ready_at_step=2,
        payload={"actor_id": "actor-a"},
        input_refs=("value:raw-1",),
        expected_versions=(VersionExpectation(ref_id="state:actor", expected_version=1),),
        correlation_id="corr-1",
        idempotency_key="idem-1",
        runner_hint="runner-a",
        registry_generation=3,
        required_surfaces=("task_kind:raw.input",),
        created_sequence=4,
    )
    assert_json_roundtrip(Task, task)

    descriptor = RunnerDescriptor(
        runner_id="runner-a",
        plugin_id="plugin-a",
        plugin_generation=1,
        accepted_task_kinds=("raw.input",),
        purity=RunnerPurity.PURE,
        input_schema={"type": "object"},
        output_schema={"type": "object"},
        metadata={"rank": 1},
        contract_surfaces=("runner:runner-a",),
    )
    assert_json_roundtrip(RunnerDescriptor, descriptor)


def test_runner_result_roundtrips_value_and_resource_refs() -> None:
    value_ref = ValueRef(
        ref_id="value:1",
        provider_id="python.resource",
        schema="value.small.v1",
        version=1,
        generation=1,
        size_hint=12,
        content_hash="hash:value",
        lifetime=ResourceLifetime.PERSISTENT,
        storage=ValueStorage.LOCAL_VALUE_STORE,
    )
    resource_ref = ResourceRef(
        ref_id="resource:1",
        provider_id="python.resource",
        resource_kind="bytes",
        schema="bytes.v1",
        version=1,
        generation=1,
        access=ResourceAccess(
            type="mmap_file",
            path="resource.bin",
            offset=0,
            len=3,
            readonly=True,
        ),
        size_hint=3,
        content_hash="hash:resource",
        lifetime=ResourceLifetime.PERSISTENT,
        lease=None,
        seal_state=ResourceSealState.SEALED,
    )
    result = RunnerResult.completed("task-1")
    result = RunnerResult(
        task_id=result.task_id,
        values=(value_ref,),
        resources=(resource_ref,),
    )

    assert_json_roundtrip(RunnerResult, result)


def test_stream_resource_ref_roundtrips_endpoint() -> None:
    stream_ref = ResourceRef(
        ref_id="resource:stream:1",
        provider_id="python.resource",
        resource_kind="chat.events",
        schema="event.v1",
        version=1,
        generation=1,
        access=ResourceAccess(type="stream", endpoint="stream://chat/events"),
        size_hint=None,
        content_hash=None,
        lifetime=ResourceLifetime.EXTERNAL_MANAGED,
        lease=None,
        seal_state=ResourceSealState.SEALED,
    )

    assert_json_roundtrip(ResourceRef, stream_ref)


def test_surface_occupancy_handle_roundtrips() -> None:
    handle = SurfaceOccupancyHandle(
        handle_id="timer:heartbeat:1",
        surface_id="timer:heartbeat",
        owner_plugin_id="plugin-a",
        plugin_generation=2,
        registry_generation=7,
        kind=SurfaceOccupancyHandleKind.TIMER,
    )

    assert_json_roundtrip(SurfaceOccupancyHandle, handle)


def test_missing_required_contract_fields_fail() -> None:
    with pytest.raises(TypeError):
        from_json_dict(Task, {"task_id": "task-1", "kind": "raw.input"})
    with pytest.raises(TypeError):
        from_json_dict(RunnerDescriptor, {"runner_id": "runner-a"})
    with pytest.raises(TypeError):
        from_json_dict(SurfaceOccupancyHandle, {"handle_id": "timer:1"})


def test_to_json_dict_rejects_non_object_top_level() -> None:
    with pytest.raises(TypeError):
        to_json_dict("not-an-object")
