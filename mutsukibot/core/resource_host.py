"""ResourceHost —— v0.3 进程内资源托管与最小资源租约。"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from typing import TypeVar

from mutsukibot.contracts.capability import CapabilityName
from mutsukibot.contracts.error import Error, Errs
from mutsukibot.contracts.ids import RefId
from mutsukibot.contracts.refpayload import RefDescriptor
from mutsukibot.core.handle import RefCountedHandle
from mutsukibot.core.scope import PluginScope

T = TypeVar("T")


class CapabilityExhaustedError(Exception):
    """ResourceHost 容量不足时的结构化错误载体。"""

    def __init__(self, error: Error) -> None:
        super().__init__(f"capability exhausted: {error.evidence}")
        self.error = error


@dataclass(slots=True)
class _Capacity:
    total: int
    used: int = 0


class ResourceLease:
    """一次资源容量租约。释放幂等。"""

    def __init__(
        self,
        *,
        host: "ResourceHost",
        capability: CapabilityName,
        amount: int,
        owner: str,
    ) -> None:
        self._host = host
        self.capability = capability
        self.amount = amount
        self.owner = owner
        self._alive = True

    @property
    def alive(self) -> bool:
        return self._alive

    def release(self) -> None:
        if not self._alive:
            return
        self._alive = False
        self._host._release_lease(self)


class ResourceHost:
    """进程内资源所有者。

    ResourceHost 用自己的内部 scope 持有物理资源 Handle。plugin reload 时只要
    不关闭 ResourceHost，底层资源就不会因为 plugin scope 关闭而释放。
    """

    def __init__(self, *, owner: str = "resource-host") -> None:
        self.owner = owner
        self._scope = PluginScope(owner)
        self._capacities: dict[CapabilityName, _Capacity] = {}
        self._leases: set[ResourceLease] = set()
        self._closed = False

    def create_handle(
        self,
        ref_id: RefId,
        *,
        target: T,
        kind: str,
        schema_id_target: str,
        schema_version_target: str,
        attributes: dict[str, str | int | float | bool] | None = None,
        finalizer: Callable[[T], None] | None = None,
    ) -> RefCountedHandle[T]:
        """创建由 ResourceHost 拥有的 Handle。"""
        self._guard()
        handle = RefCountedHandle(
            target=target,
            descriptor=RefDescriptor(
                ref_id=ref_id,
                kind=kind,
                schema_id_target=schema_id_target,
                schema_version_target=schema_version_target,
                attributes=attributes or {},
            ),
            finalizer=finalizer,
        )
        handle.attach_to(self._scope)
        return handle

    def declare_capacity(self, capability: CapabilityName, *, total: int) -> None:
        self._guard()
        if total < 0:
            raise ValueError("ResourceHost capacity total must be >= 0")
        used = self._capacities.get(capability, _Capacity(total=0)).used
        self._capacities[capability] = _Capacity(total=total, used=used)

    def acquire(
        self,
        capability: CapabilityName,
        *,
        amount: int = 1,
        owner: str,
    ) -> ResourceLease:
        self._guard()
        if amount <= 0:
            raise ValueError("ResourceHost lease amount must be > 0")
        capacity = self._capacities.get(capability)
        total = capacity.total if capacity is not None else 0
        used = capacity.used if capacity is not None else 0
        available = total - used
        if capacity is None or available < amount:
            err = Error(
                code=Errs.CAPABILITY_EXHAUSTED,
                source=self.owner,
                route="resource_host.acquire",
                evidence={
                    "capability": str(capability),
                    "owner": owner,
                    "requested": amount,
                    "available": max(available, 0),
                    "total": total,
                    "used": used,
                },
            )
            raise CapabilityExhaustedError(err)

        capacity.used += amount
        lease = ResourceLease(
            host=self,
            capability=capability,
            amount=amount,
            owner=owner,
        )
        self._leases.add(lease)
        return lease

    def _release_lease(self, lease: ResourceLease) -> None:
        self._leases.discard(lease)
        capacity = self._capacities.get(lease.capability)
        if capacity is not None:
            capacity.used = max(0, capacity.used - lease.amount)

    async def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        for lease in tuple(self._leases):
            lease.release()
        await self._scope.close()

    def _guard(self) -> None:
        if self._closed:
            raise RuntimeError(f"ResourceHost({self.owner!r}) 已关闭")


__all__ = ["CapabilityExhaustedError", "ResourceHost", "ResourceLease"]
