"""ResourceHost —— v0.3 进程内资源托管与最小资源租约。"""

from __future__ import annotations

from collections.abc import Awaitable, Callable
from dataclasses import dataclass, replace
import inspect
from typing import TYPE_CHECKING, Any, TypeVar

from mutsukibot.contracts.capability import CapabilityName
from mutsukibot.contracts.error import Error, Errs
from mutsukibot.contracts.ids import RefId
from mutsukibot.contracts.refpayload import RefDescriptor
from mutsukibot.core.handle import RefCountedHandle
from mutsukibot.core.scope import PluginScope
from mutsukibot.core.trace import trace_span

if TYPE_CHECKING:
    from mutsukibot.core.context import AgentContext

T = TypeVar("T")


class CapabilityExhaustedError(Exception):
    """ResourceHost 容量不足时的结构化错误载体。"""

    def __init__(self, error: Error) -> None:
        super().__init__(f"capability exhausted: {error.evidence}")
        self.error = error


class ResourceHandleNotFoundError(Exception):
    """ResourceHost 查找句柄失败时的结构化错误载体。"""

    def __init__(self, error: Error) -> None:
        super().__init__(f"resource handle not found: {error.evidence}")
        self.error = error


@dataclass(slots=True)
class _Capacity:
    total: int
    used: int = 0


@dataclass(frozen=True, slots=True)
class ResourceRecord:
    """ResourceHost 暴露给策略函数的只读资源快照。"""

    ref_id: RefId
    kind: str
    schema_id_target: str
    schema_version_target: str
    attributes: dict[str, str | int | float | bool]
    last_touched_tick: int


ResourceEvictionPolicy = Callable[[ResourceRecord], bool]
ResourceKeepalivePolicy = Callable[[ResourceRecord], bool | Awaitable[bool]]


@dataclass(slots=True)
class _ResourceEntry:
    handle: RefCountedHandle[Any]
    record: ResourceRecord


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

    def __init__(
        self,
        *,
        owner: str = "resource-host",
        eviction_policy: ResourceEvictionPolicy | None = None,
        keepalive_policy: ResourceKeepalivePolicy | None = None,
    ) -> None:
        self.owner = owner
        self._scope = PluginScope(owner)
        self._capacities: dict[CapabilityName, _Capacity] = {}
        self._leases: set[ResourceLease] = set()
        self._handles: dict[RefId, _ResourceEntry] = {}
        self._eviction_policy = eviction_policy
        self._keepalive_policy = keepalive_policy
        self._tick = 0
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
        if ref_id in self._handles:
            raise ValueError(f"ResourceHost handle {ref_id!r} already exists")
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
        self._handles[ref_id] = _ResourceEntry(
            handle=handle,
            record=ResourceRecord(
                ref_id=ref_id,
                kind=kind,
                schema_id_target=schema_id_target,
                schema_version_target=schema_version_target,
                attributes=dict(attributes or {}),
                last_touched_tick=self._next_tick(),
            ),
        )
        return handle

    def get_handle(
        self,
        ref_id: RefId,
        *,
        kind: str | None = None,
    ) -> RefCountedHandle[Any]:
        """按 ref_id 查找 ResourceHost 托管的 Handle，并可校验 kind。"""
        self._guard()
        entry = self._handles.get(ref_id)
        if entry is None or not entry.handle.is_alive():
            err = Error(
                code=Errs.REF_NOT_FOUND,
                source=self.owner,
                route="resource_host.get_handle",
                evidence={"ref_id": ref_id, "expected_kind": kind or ""},
            )
            raise ResourceHandleNotFoundError(err)
        actual_kind = entry.record.kind
        if kind is not None and actual_kind != kind:
            err = Error(
                code=Errs.REF_KIND_MISMATCH,
                source=self.owner,
                route="resource_host.get_handle",
                evidence={
                    "ref_id": ref_id,
                    "expected_kind": kind,
                    "actual_kind": actual_kind,
                },
            )
            raise ResourceHandleNotFoundError(err)
        self._touch(ref_id)
        return entry.handle

    async def get_handle_for(
        self,
        ctx: "AgentContext",
        ref_id: RefId,
        *,
        kind: str | None = None,
    ) -> RefCountedHandle[Any]:
        """带 trace 的句柄解析入口，供 RefArg(ResourceHost) 使用。"""
        async with trace_span(
            ctx,
            "resource_host.get_handle",
            attributes={
                "host": self.owner,
                "ref_id": ref_id,
                "expected_kind": kind or "",
            },
        ):
            return self.get_handle(ref_id, kind=kind)

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

    async def acquire_for(
        self,
        ctx: "AgentContext",
        capability: CapabilityName,
        *,
        amount: int = 1,
        owner: str,
    ) -> ResourceLease:
        """带 trace 的资源租约申请入口。"""
        async with trace_span(
            ctx,
            "resource_host.acquire",
            attributes={
                "host": self.owner,
                "capability": str(capability),
                "owner": owner,
                "amount": amount,
            },
        ):
            return self.acquire(capability, amount=amount, owner=owner)

    async def release_for(self, ctx: "AgentContext", lease: ResourceLease) -> None:
        """带 trace 的资源租约释放入口。"""
        async with trace_span(
            ctx,
            "resource_host.release",
            attributes={
                "host": self.owner,
                "capability": str(lease.capability),
                "owner": lease.owner,
                "amount": lease.amount,
            },
        ):
            lease.release()

    def evict(
        self,
        policy: ResourceEvictionPolicy | None = None,
    ) -> tuple[RefId, ...]:
        """按策略释放托管句柄；返回被移除的 ref_id。"""
        self._guard()
        active_policy = policy or self._eviction_policy
        if active_policy is None:
            return ()
        evicted: list[RefId] = []
        for ref_id, entry in tuple(self._handles.items()):
            if active_policy(entry.record):
                self._evict_ref(ref_id)
                evicted.append(ref_id)
        return tuple(evicted)

    async def keepalive(
        self,
        policy: ResourceKeepalivePolicy | None = None,
    ) -> tuple[RefId, ...]:
        """执行 keepalive 策略；返回心跳失败并被移除的 ref_id。"""
        self._guard()
        active_policy = policy or self._keepalive_policy
        if active_policy is None:
            return ()
        evicted: list[RefId] = []
        for ref_id, entry in tuple(self._handles.items()):
            result = active_policy(entry.record)
            alive = await result if inspect.isawaitable(result) else result
            if alive:
                self._touch(ref_id)
            else:
                self._evict_ref(ref_id)
                evicted.append(ref_id)
        return tuple(evicted)

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
        self._handles.clear()

    def _guard(self) -> None:
        if self._closed:
            raise RuntimeError(f"ResourceHost({self.owner!r}) 已关闭")

    def _next_tick(self) -> int:
        self._tick += 1
        return self._tick

    def _touch(self, ref_id: RefId) -> None:
        entry = self._handles.get(ref_id)
        if entry is None:
            return
        entry.record = replace(entry.record, last_touched_tick=self._next_tick())

    def _evict_ref(self, ref_id: RefId) -> None:
        entry = self._handles.pop(ref_id, None)
        if entry is None:
            return
        entry.handle.release()


__all__ = [
    "CapabilityExhaustedError",
    "ResourceEvictionPolicy",
    "ResourceHandleNotFoundError",
    "ResourceHost",
    "ResourceKeepalivePolicy",
    "ResourceLease",
    "ResourceRecord",
]
