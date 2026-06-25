from __future__ import annotations

import importlib

import pytest

from mutsuki_runtime_python.contracts.resource import ValueRef
from mutsuki_runtime_python.runners.protocol import RunnerInvokeError

PythonResourceManager = importlib.import_module(
    "mutsuki_runtime_python." + "resources.manager"
).PythonResourceManager


def test_resource_manager_packs_small_and_large_values() -> None:
    manager = PythonResourceManager(inline_value_max_bytes=16)

    assert manager.pack_value("small.v1", {"a": 1}) == {"a": 1}
    large = manager.pack_value("large.v1", {"blob": "x" * 100})

    assert isinstance(large, ValueRef)
    assert manager.get_value(large) == {"blob": "x" * 100}


def test_resource_manager_supports_mmap_cow_and_exclusive_write_lease() -> None:
    manager = PythonResourceManager()
    resource = manager.create_mmap_resource("bytes.v1", b"abc")

    assert manager.read_resource(resource) == b"abc"
    blob = manager.create_blob_resource("blob.v1", b"blob-data")
    assert blob.access.type == "blob"
    assert manager.read_resource(blob) == b"blob-data"
    cow = manager.copy_on_write(resource, b"xyz")
    assert cow.ref_id != resource.ref_id
    lease = manager.acquire_write_lease(resource.ref_id, "runner-a", expires_at_step=5)
    updated = manager.write_with_lease(lease, b"def", current_step=2)

    assert updated.generation == resource.generation + 1
    assert manager.read_resource(updated) == b"def"


def test_expired_write_lease_fails_loudly() -> None:
    manager = PythonResourceManager()
    resource = manager.create_mmap_resource("bytes.v1", b"abc")
    lease = manager.acquire_write_lease(resource.ref_id, "runner-a", expires_at_step=1)

    with pytest.raises(RunnerInvokeError):
        manager.write_with_lease(lease, b"late", current_step=2)
