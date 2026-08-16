from __future__ import annotations

from collections.abc import Callable
from typing import TypeVar

from mutsuki_runner_kit.contracts.errors import RuntimeError
from mutsuki_runner_kit.contracts.resource import ExclusiveWriteLease, LeaseToken, ResourceLease
from mutsuki_runner_kit.runners.protocol import RunnerInvokeError

ERR_RESOURCE_LEASE_RELEASED = "resource.lease_released"

TRelease = TypeVar("TRelease")


class ExclusiveWriteGuard:
    """Exclusive write lease. Re-using after ``release`` fails."""

    __slots__ = ("_lease",)

    def __init__(self, lease: ExclusiveWriteLease) -> None:
        self._lease: ExclusiveWriteLease | None = lease

    def as_lease(self) -> ExclusiveWriteLease:
        return self._require_lease()

    def token(self) -> LeaseToken:
        return self._require_lease().token

    def release(self, releaser: Callable[[ExclusiveWriteLease], TRelease]) -> TRelease:
        lease = self._require_lease()
        self._lease = None
        return releaser(lease)

    def _require_lease(self) -> ExclusiveWriteLease:
        if self._lease is None:
            raise _already_released("resource.write_lease.release")
        return self._lease


class ResourceLeaseGuard:
    """Exclusive cell lease. Re-using after ``release`` fails."""

    __slots__ = ("_lease",)

    def __init__(self, lease: ResourceLease) -> None:
        self._lease: ResourceLease | None = lease

    def as_lease(self) -> ResourceLease:
        return self._require_lease()

    def release(self, releaser: Callable[[ResourceLease], TRelease]) -> TRelease:
        lease = self._require_lease()
        self._lease = None
        return releaser(lease)

    def _require_lease(self) -> ResourceLease:
        if self._lease is None:
            raise _already_released("resource.cell_lease.release")
        return self._lease


def _already_released(route: str) -> RunnerInvokeError:
    return RunnerInvokeError(
        RuntimeError(
            code=ERR_RESOURCE_LEASE_RELEASED,
            source="runtime.sdk",
            route=route,
        )
    )


__all__ = (
    "ERR_RESOURCE_LEASE_RELEASED",
    "ExclusiveWriteGuard",
    "ResourceLeaseGuard",
)
