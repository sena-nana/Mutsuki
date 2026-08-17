from __future__ import annotations

import importlib

import pytest

from mutsuki_runner_kit.contracts.ids import (
    ExecutorId,
    RefId,
    ResourceCellId,
    ResourceLeaseId,
    TaskId,
)
from mutsuki_runner_kit.contracts.resource import ExclusiveWriteLease, LeaseToken, ResourceLease
from mutsuki_runner_kit.resources.lease import (
    ERR_RESOURCE_LEASE_RELEASED,
    ExclusiveWriteGuard,
    ResourceLeaseGuard,
)
from mutsuki_runner_kit.runners.protocol import RunnerInvokeError

FakeResourceProvider = importlib.import_module(
    "mutsuki_runner_kit." + "testing.fake_resource_provider"
).FakeResourceProvider


def _write_lease(token_id: str = "lease-token-1") -> ExclusiveWriteLease:
    return ExclusiveWriteLease(
        token=LeaseToken(
            token_id=token_id,
            ref_id=RefId("resource:state"),
            owner="runner-a",
            mode="exclusive_write",
            expires_at_step=5,
            generation=1,
        )
    )


def _cell_lease(lease_id: str = "cell-lease-1") -> ResourceLease:
    return ResourceLease(
        lease_id=ResourceLeaseId(lease_id),
        cell_id=ResourceCellId("cell-1"),
        borrower_task_id=TaskId("task-1"),
        borrower_executor_id=ExecutorId("executor-1"),
        mode="exclusive",
        expires_at_step=5,
        generation=1,
    )


def test_write_guard_consumes_on_release() -> None:
    lease = _write_lease()
    guard = ExclusiveWriteGuard(lease)

    assert guard.as_lease() is lease
    assert guard.token() is lease.token

    released: list[str] = []
    guard.release(lambda inner: released.append(inner.token.token_id))

    assert released == [lease.token.token_id]
    with pytest.raises(RunnerInvokeError) as already_released:
        guard.release(lambda _inner: released.append("again"))
    assert already_released.value.error.code == ERR_RESOURCE_LEASE_RELEASED
    assert released == [lease.token.token_id]


def test_resource_lease_guard_consumes_on_release() -> None:
    lease = _cell_lease()
    guard = ResourceLeaseGuard(lease)

    released: list[str] = []
    guard.release(lambda inner: released.append(inner.lease_id))

    assert released == [lease.lease_id]
    with pytest.raises(RunnerInvokeError) as already_released:
        guard.as_lease()
    assert already_released.value.error.code == ERR_RESOURCE_LEASE_RELEASED


def test_write_guard_can_wrap_provider_acquire_and_release() -> None:
    manager = FakeResourceProvider()
    resource = manager.create_mmap_resource("bytes.v1", b"abc")
    token = manager.acquire_write_lease(resource.ref_id, "runner-a", expires_at_step=5)
    guard = ExclusiveWriteGuard(ExclusiveWriteLease(token=token))

    updated = guard.release(
        lambda lease: manager.write_with_lease(lease.token, b"def", current_step=2)
    )

    assert updated.generation == resource.generation + 1
    assert manager.read_resource(updated) == b"def"
    with pytest.raises(RunnerInvokeError) as already_released:
        guard.release(lambda lease: manager.write_with_lease(lease.token, b"ghi", current_step=3))
    assert already_released.value.error.code == ERR_RESOURCE_LEASE_RELEASED
