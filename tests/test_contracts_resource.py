from __future__ import annotations

from mutsuki_runner_kit.contracts.codec import to_json_dict
from mutsuki_runner_kit.contracts.resource import (
    CommandBatch,
    CommandPlan,
    PatchDescriptor,
    PlanReceipt,
    ReadPlan,
    ResourceAccess,
    ResourceCellRef,
    ResourceId,
    ResourceLease,
    ResourceLifetime,
    ResourceProviderCompatibility,
    ResourceProviderReloadPolicy,
    ResourceRef,
    ResourceSealState,
    ResourceSemantic,
    ResourceTypeDescriptor,
    ResourceValue,
    SagaPlan,
    SnapshotDescriptor,
    TransactionPlan,
    ValueRef,
    ValueStorage,
    WritePlan,
)
from mutsuki_runner_kit.contracts.state import StateRef
from mutsuki_runner_kit.testing.assertions import assert_json_roundtrip


def _resource_ref(
    ref_id: str = "resource:1",
    kind_id: str = "bytes",
    semantic: ResourceSemantic = ResourceSemantic.FROZEN_VALUE,
) -> ResourceRef:
    return ResourceRef(
        ref_id=ref_id,
        resource_id=ResourceId(
            kind_id=kind_id,
            slot_id=ref_id,
            generation=1,
            version=1,
        ),
        semantic=semantic,
        provider_id="python.resource",
        resource_kind=kind_id,
        schema="bytes.v1",
        version=1,
        generation=1,
        access=ResourceAccess.blob("blob-store", ref_id),
        size_hint=4,
        content_hash="hash:resource",
        lifetime=ResourceLifetime.PERSISTENT,
        lease=None,
        seal_state=ResourceSealState.SEALED,
    )


def _provider_compatibility() -> ResourceProviderCompatibility:
    return ResourceProviderCompatibility(
        schema_version="1.0.0",
        required_operations=("read", "export"),
        preserves_resource_type_id=True,
        accepts_older_generations=True,
        lease_drain_required=True,
    )


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
    resource_ref = _resource_ref()

    assert_json_roundtrip(StateRef, StateRef(ref_id="state:1", schema="state.v1", version=3))
    assert_json_roundtrip(ResourceValue, ResourceValue.inline("value.v1", {"a": 1}, 1))
    assert_json_roundtrip(ResourceValue, ResourceValue.value_ref_value(value_ref))
    assert_json_roundtrip(ResourceValue, ResourceValue.resource_ref_value(resource_ref))
    assert_json_roundtrip(ResourceId, resource_ref.resource_id)
    assert_json_roundtrip(
        ResourceTypeDescriptor,
        ResourceTypeDescriptor(
            kind_id="bytes",
            semantic=ResourceSemantic.FROZEN_VALUE,
            schema="bytes.v1",
            provider_id="python.resource",
            operations=("read", "export"),
            reload_policy=ResourceProviderReloadPolicy.COMPATIBLE_WITHOUT_LEASES,
            compatibility=_provider_compatibility(),
        ),
    )
    assert_json_roundtrip(ResourceProviderCompatibility, _provider_compatibility())


def test_resource_cell_and_resource_lease_roundtrip() -> None:
    cell = ResourceCellRef(
        cell_id="cell:http:default",
        resource_kind="http.connection_pool",
        owner_plugin_id="plugin-http",
        schema="http.connection_pool.v1",
        generation=1,
        health="healthy",
        reload_policy="drain",
    )
    lease = ResourceLease(
        lease_id="resource-lease-1",
        cell_id=cell.cell_id,
        borrower_task_id="task-http",
        borrower_executor_id="executor-http",
        mode="shared",
        expires_at_step=None,
        generation=1,
    )

    assert_json_roundtrip(ResourceCellRef, cell)
    assert_json_roundtrip(ResourceLease, lease)


def test_stream_resource_ref_roundtrips_endpoint() -> None:
    stream_ref = _resource_ref(
        ref_id="resource:stream:1",
        kind_id="chat.events",
        semantic=ResourceSemantic.STREAM_RESOURCE,
    )

    assert_json_roundtrip(ResourceRef, stream_ref)


def test_resource_plan_contracts_roundtrip() -> None:
    resource = _resource_ref(semantic=ResourceSemantic.COW_VERSIONED_STATE)
    read_plan = ReadPlan(
        plan_id="read-plan:1",
        resource=resource,
        operation="collect",
        args={"range": "all"},
    )
    patch = PatchDescriptor(
        patch_id="patch:1",
        target_ref=resource,
        base_version=1,
        conflict_policy="fail",
        operations={"replace": "all"},
    )
    write_plan = WritePlan(
        plan_id="write-plan:1",
        resource=resource,
        base_version=1,
        conflict_policy="fail",
        patch=patch,
        returning=read_plan,
    )
    command = CommandPlan(
        plan_id="command:1",
        capability=_resource_ref(kind_id="db_pool", semantic=ResourceSemantic.CAPABILITY_RESOURCE),
        operation="query",
        args={"sql": "select 1"},
        idempotency_key=None,
    )
    snapshot = SnapshotDescriptor(
        snapshot_ref=_resource_ref(
            "snapshot:1", "ast_snapshot", ResourceSemantic.VERSIONED_SNAPSHOT
        ),
        source_ref=resource,
        source_version=1,
        snapshot_version=1,
        is_stale=False,
        is_latest=True,
    )

    assert_json_roundtrip(ReadPlan, read_plan)
    assert_json_roundtrip(PatchDescriptor, patch)
    assert_json_roundtrip(WritePlan, write_plan)
    assert_json_roundtrip(TransactionPlan, TransactionPlan("tx:1", (write_plan,), True))
    assert_json_roundtrip(CommandPlan, command)
    assert_json_roundtrip(CommandBatch, CommandBatch("batch:1", (command,), False))
    assert_json_roundtrip(SagaPlan, SagaPlan("saga:1", (command,), (command,)))
    assert_json_roundtrip(
        PlanReceipt,
        PlanReceipt(
            plan_id="write-plan:1",
            status="committed",
            resource_ref=resource,
            snapshot=snapshot,
            descriptor_updates=(resource,),
            new_version=2,
            output=None,
        ),
    )
