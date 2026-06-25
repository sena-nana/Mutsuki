from __future__ import annotations

import pytest

from mutsuki_runtime_python.contracts.codec import from_json_dict, to_json_dict
from mutsuki_runtime_python.contracts.plugin import (
    ArtifactType,
    LifecyclePolicy,
    PermissionGrant,
    PluginArtifact,
    PluginManifest,
    PluginProvides,
    RuntimeLoadPlan,
    RuntimeProfile,
)
from mutsuki_runtime_python.contracts.resource import (
    ResourceAccess,
    ResourceLifetime,
    ResourceRef,
    ResourceSealState,
    ResourceValue,
    ValueRef,
    ValueStorage,
)
from mutsuki_runtime_python.contracts.runner import (
    RunnerDescriptor,
    RunnerPurity,
    RunnerResult,
)
from mutsuki_runtime_python.contracts.state import StateRef, VersionExpectation
from mutsuki_runtime_python.contracts.surface import (
    ContractSurface,
    ContractSurfaceKind,
    SurfaceOccupancy,
    SurfaceOccupancyHandle,
    SurfaceOccupancyHandleKind,
)
from mutsuki_runtime_python.contracts.task import (
    Task,
    TaskDemand,
    TaskMatchRule,
)
from mutsuki_runtime_python.testing.assertions import assert_json_roundtrip


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
        access=ResourceAccess.mmap_file(
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
    result = RunnerResult(
        task_id="task-1",
        values=(value_ref,),
        resources=(resource_ref,),
    )

    assert_json_roundtrip(RunnerResult, result)


def test_resource_access_variants_match_rust_tagged_shape() -> None:
    cases = [
        (ResourceAccess.inline(), {"type": "inline"}),
        (
            ResourceAccess.mmap_file("resource.bin", offset=0, len=3, readonly=True),
            {
                "type": "mmap_file",
                "path": "resource.bin",
                "offset": 0,
                "len": 3,
                "readonly": True,
            },
        ),
        (
            ResourceAccess.shared_memory("segment-a", offset=4, len=8, readonly=False),
            {
                "type": "shared_memory",
                "name": "segment-a",
                "offset": 4,
                "len": 8,
                "readonly": False,
            },
        ),
        (
            ResourceAccess.blob("blob-store", "key-1"),
            {"type": "blob", "store_id": "blob-store", "key": "key-1"},
        ),
        (
            ResourceAccess.stream("stream://chat/events"),
            {"type": "stream", "endpoint": "stream://chat/events"},
        ),
        (
            ResourceAccess.provider_rpc("provider-a", "read"),
            {"type": "provider_rpc", "provider_id": "provider-a", "method": "read"},
        ),
    ]

    for access, expected in cases:
        assert to_json_dict(access) == expected
        assert_json_roundtrip(ResourceAccess, access)


def test_resource_lifetime_lease_until_roundtrips_external_tag_shape() -> None:
    value_ref = ValueRef(
        ref_id="value:lease",
        provider_id="python.resource",
        schema="value.v1",
        version=1,
        generation=1,
        size_hint=None,
        content_hash=None,
        lifetime=ResourceLifetime.lease_until(9),
        storage=ValueStorage.LOCAL_VALUE_STORE,
    )

    encoded = to_json_dict(value_ref)
    assert encoded["lifetime"] == {"lease_until": 9}
    assert_json_roundtrip(ValueRef, value_ref)


def test_resource_value_and_state_ref_roundtrip() -> None:
    value_ref = ValueRef(
        ref_id="value:1",
        provider_id="python.resource",
        schema="value.v1",
        version=1,
        generation=1,
        size_hint=4,
        content_hash="hash:value",
        lifetime=ResourceLifetime.PERSISTENT,
        storage=ValueStorage.LOCAL_VALUE_STORE,
    )
    resource_ref = ResourceRef(
        ref_id="resource:1",
        provider_id="python.resource",
        resource_kind="blob",
        schema="bytes.v1",
        version=1,
        generation=1,
        access=ResourceAccess.blob("blob-store", "resource:1"),
        size_hint=4,
        content_hash="hash:resource",
        lifetime=ResourceLifetime.PERSISTENT,
        lease=None,
        seal_state=ResourceSealState.SEALED,
    )

    assert_json_roundtrip(StateRef, StateRef(ref_id="state:1", schema="state.v1", version=3))
    assert_json_roundtrip(ResourceValue, ResourceValue.inline("value.v1", {"a": 1}, 1))
    assert_json_roundtrip(ResourceValue, ResourceValue.value_ref_value(value_ref))
    assert_json_roundtrip(ResourceValue, ResourceValue.resource_ref_value(resource_ref))


def test_plugin_load_plan_profile_and_task_demand_roundtrip() -> None:
    descriptor = RunnerDescriptor(
        runner_id="runner-a",
        plugin_id="plugin-a",
        plugin_generation=1,
        accepted_task_kinds=("raw.input",),
        purity=RunnerPurity.PURE,
        input_schema={"type": "object"},
        output_schema={"type": "object"},
        metadata={},
        contract_surfaces=("runner:runner-a",),
    )
    demand = TaskDemand(
        demand_id="demand-1",
        plugin_id="plugin-a",
        match_rule=TaskMatchRule.kind_prefix("raw."),
        target_task_kind="raw.input",
        target_runner_hint="runner-a",
        priority=5,
        payload_projection={"copy": True},
        input_ref_policy="forward",
    )
    provides = PluginProvides(
        runners=(descriptor,),
        task_demands=(demand,),
        resource_schemas=("bytes.v1",),
        resource_providers=("python.resource",),
        effects=("effect.chat.send",),
        streams=("chat.events",),
        subscriptions=("chat.messages",),
        timers=("heartbeat",),
        state_schemas=("state.actor.v1",),
    )
    manifest = PluginManifest(
        plugin_id="plugin-a",
        version="0.1.0",
        api_version="mutsuki-plugin-v1",
        artifact=PluginArtifact(
            artifact_type=ArtifactType.PYTHON,
            path="plugin.py",
            sha256="sha256:plugin",
        ),
        provides=provides,
        requires=(),
        permissions=PermissionGrant(effects=("effect.chat.send",), resources=("read",)),
        lifecycle=LifecyclePolicy(
            reload_policy="drain_and_swap",
            unload_timeout_ms=5000,
            supports_cancel=True,
            supports_dispose=True,
            supports_snapshot=False,
        ),
        metadata={"rank": 1},
    )
    plan = RuntimeLoadPlan(
        lock_version=1,
        core_api_version="mutsuki-core-v1",
        profile_id="default",
        profile_hash="sha256:profile",
        registry_generation=1,
        plugins=(manifest,),
        load_order=("plugin-a",),
        runner_bindings={"raw.input": "runner-a"},
        contract_surfaces=(
            ContractSurface(
                surface_id="runner:runner-a",
                kind=ContractSurfaceKind.RUNNER,
                owner_plugin_id="plugin-a",
                fingerprint="sha256:runner",
                deprecated=False,
            ),
        ),
    )

    assert_json_roundtrip(TaskMatchRule, TaskMatchRule.any())
    assert_json_roundtrip(TaskMatchRule, TaskMatchRule.kind_rule("raw.input"))
    assert_json_roundtrip(TaskDemand, demand)
    assert_json_roundtrip(PluginProvides, provides)
    assert_json_roundtrip(PluginManifest, manifest)
    assert_json_roundtrip(RuntimeLoadPlan, plan)
    assert_json_roundtrip(
        RuntimeProfile,
        RuntimeProfile(
            profile_id="default",
            enabled_plugins=("plugin-a",),
            bindings={"raw.input": "plugin-a"},
            allow_dynamic_registration=False,
            allow_hot_reload=True,
        ),
    )


def test_surface_occupancy_roundtrips() -> None:
    occupancy = SurfaceOccupancy(
        surface_id="runner:runner-a",
        pending_tasks=0,
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


def test_stream_resource_ref_roundtrips_endpoint() -> None:
    stream_ref = ResourceRef(
        ref_id="resource:stream:1",
        provider_id="python.resource",
        resource_kind="chat.events",
        schema="event.v1",
        version=1,
        generation=1,
        access=ResourceAccess.stream("stream://chat/events"),
        size_hint=None,
        content_hash=None,
        lifetime=ResourceLifetime.EXTERNAL_MANAGED,
        lease=None,
        seal_state=ResourceSealState.SEALED,
    )

    assert_json_roundtrip(ResourceRef, stream_ref)


def test_missing_required_contract_fields_fail() -> None:
    with pytest.raises(TypeError):
        from_json_dict(Task, {"task_id": "task-1", "kind": "raw.input"})
    with pytest.raises(TypeError):
        from_json_dict(RunnerDescriptor, {"runner_id": "runner-a"})
    with pytest.raises(TypeError):
        from_json_dict(RuntimeLoadPlan, {"lock_version": 1})
    with pytest.raises(TypeError):
        from_json_dict(
            PluginProvides,
            {
                "runners": [],
                "task_demands": [],
                "resource_schemas": [],
                "resource_providers": [],
                "effects": [],
            },
        )
    with pytest.raises(TypeError):
        from_json_dict(SurfaceOccupancyHandle, {"handle_id": "timer:1"})
    with pytest.raises(TypeError):
        from_json_dict(ResourceRef, {"ref_id": "resource:1"})
    with pytest.raises(TypeError):
        from_json_dict(ResourceAccess, {"type": "mmap_file", "path": "resource.bin"})


def test_to_json_dict_rejects_non_object_top_level() -> None:
    with pytest.raises(TypeError):
        to_json_dict("not-an-object")
