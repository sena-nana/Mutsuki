from __future__ import annotations

from mutsuki_runtime_python.contracts.codec import to_json_dict
from mutsuki_runtime_python.contracts.resource import (
    ResourceAccess,
    ResourceLifetime,
    ResourceRef,
    ResourceSealState,
    ResourceValue,
    ValueRef,
    ValueStorage,
)
from mutsuki_runtime_python.contracts.state import StateRef
from mutsuki_runtime_python.testing.assertions import assert_json_roundtrip


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

