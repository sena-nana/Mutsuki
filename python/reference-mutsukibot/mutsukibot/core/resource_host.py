"""ResourceHost —— v0.3 进程内资源托管与最小资源租约。"""

from __future__ import annotations

from collections.abc import Awaitable, Callable, Mapping
from dataclasses import dataclass, replace
import inspect
from typing import TYPE_CHECKING, Any, TypeVar

import msgspec

from mutsukibot.contracts.capability import CapabilityName
from mutsukibot.contracts.error import Error, Errs
from mutsukibot.contracts.ids import RefId
from mutsukibot.contracts.refpayload import RefDescriptor
from mutsukibot.contracts.resource_host import (
    ResourceHostPolicyConfig,
    ResourceRecordSelector,
)
from mutsukibot.core.handle import RefCountedHandle
from mutsukibot.core.scope import PluginScope
from mutsukibot.core.trace import trace_span

if TYPE_CHECKING:
    from mutsukibot.core.context import AgentContext

T = TypeVar("T")


class ResourcePolicyConfigError(Exception):
    """ResourceHost 策略配置无效时抛出的结构化错误载体。"""

    def __init__(self, error: Error) -> None:
        super().__init__(f"resource policy invalid: {error.evidence}")
        self.error = error


class ResourcePolicyConflictError(Exception):
    """策略配置与显式 callable 冲突时抛出的结构化错误载体。"""

    def __init__(self, error: Error) -> None:
        super().__init__(f"resource policy conflict: {error.evidence}")
        self.error = error


def _struct_field_names(struct_type: type[msgspec.Struct]) -> set[str]:
    return {field.name for field in msgspec.structs.fields(struct_type)}


_SELECTOR_FIELDS = _struct_field_names(ResourceRecordSelector)
_HOST_POLICY_FIELDS = _struct_field_names(ResourceHostPolicyConfig)


def _validate_selector_mapping(
    raw: Mapping[str, object],
    *,
    policy_name: str,
    host_owner: str,
) -> None:
    unknown = sorted(set(raw) - _SELECTOR_FIELDS)
    if unknown:
        err = Error(
            code=Errs.RESOURCE_POLICY_INVALID,
            source=host_owner,
            route=f"resource_host.policy_config.{policy_name}",
            evidence={
                "policy": policy_name,
                "unknown_keys": ",".join(unknown),
            },
        )
        raise ResourcePolicyConfigError(err)


def _validate_host_policy_mapping(
    raw: Mapping[str, object],
    *,
    host_owner: str,
) -> None:
    unknown = sorted(set(raw) - _HOST_POLICY_FIELDS)
    if unknown:
        err = Error(
            code=Errs.RESOURCE_POLICY_INVALID,
            source=host_owner,
            route="resource_host.policy_config",
            evidence={"unknown_keys": ",".join(unknown)},
        )
        raise ResourcePolicyConfigError(err)

    for policy_name in ("eviction", "keepalive"):
        nested = raw.get(policy_name)
        if isinstance(nested, Mapping):
            _validate_selector_mapping(
                nested,
                policy_name=policy_name,
                host_owner=host_owner,
            )


def _validate_policy_semantics(
    config: ResourceHostPolicyConfig,
    *,
    host_owner: str,
) -> None:
    if config.is_empty():
        err = Error(
            code=Errs.RESOURCE_POLICY_INVALID,
            source=host_owner,
            route="resource_host.policy_config",
            evidence={"reason": "empty_policy_config"},
        )
        raise ResourcePolicyConfigError(err)

    for policy_name, selector in (
        ("eviction", config.eviction),
        ("keepalive", config.keepalive),
    ):
        if selector is None:
            continue

        issues: list[str] = []
        if selector.is_empty():
            issues.append("empty_selector")
        if selector.ref_id is not None and selector.ref_id == "":
            issues.append("ref_id_empty")
        if selector.ref_id_prefix is not None:
            if selector.ref_id_prefix == "":
                issues.append("ref_id_prefix_empty")
            elif selector.ref_id is not None and not str(selector.ref_id).startswith(
                selector.ref_id_prefix
            ):
                issues.append("ref_id_prefix_mismatch")
        if selector.kind is not None and selector.kind == "":
            issues.append("kind_empty")
        if selector.kind_prefix is not None:
            if selector.kind_prefix == "":
                issues.append("kind_prefix_empty")
            elif selector.kind is not None and not selector.kind.startswith(
                selector.kind_prefix
            ):
                issues.append("kind_prefix_mismatch")
        if selector.schema_id_target is not None and selector.schema_id_target == "":
            issues.append("schema_id_target_empty")
        if selector.schema_id_target_prefix is not None:
            if selector.schema_id_target_prefix == "":
                issues.append("schema_id_target_prefix_empty")
            elif selector.schema_id_target is not None and not selector.schema_id_target.startswith(
                selector.schema_id_target_prefix
            ):
                issues.append("schema_id_target_prefix_mismatch")
        if (
            selector.schema_version_target is not None
            and selector.schema_version_target == ""
        ):
            issues.append("schema_version_target_empty")
        if selector.schema_version_target_prefix is not None:
            if selector.schema_version_target_prefix == "":
                issues.append("schema_version_target_prefix_empty")
            elif selector.schema_version_target is not None and not (
                selector.schema_version_target.startswith(
                    selector.schema_version_target_prefix
                )
            ):
                issues.append("schema_version_target_prefix_mismatch")

        if issues:
            err = Error(
                code=Errs.RESOURCE_POLICY_INVALID,
                source=host_owner,
                route=f"resource_host.policy_config.{policy_name}",
                evidence={
                    "policy": policy_name,
                    "issues": ",".join(issues),
                    "selector": repr(selector),
                },
            )
            raise ResourcePolicyConfigError(err)


def _normalize_policy_config(
    raw: ResourceHostPolicyConfig | Mapping[str, object] | None,
    *,
    host_owner: str,
) -> ResourceHostPolicyConfig | None:
    if raw is None:
        return None
    if isinstance(raw, Mapping):
        _validate_host_policy_mapping(raw, host_owner=host_owner)
    try:
        config = (
            raw
            if isinstance(raw, ResourceHostPolicyConfig)
            else msgspec.convert(raw, type=ResourceHostPolicyConfig)
        )
    except Exception as exc:
        err = Error(
            code=Errs.RESOURCE_POLICY_INVALID,
            source=host_owner,
            route="resource_host.policy_config",
            evidence={
                "exception_type": type(exc).__qualname__,
                "exception_repr": repr(exc),
            },
        )
        raise ResourcePolicyConfigError(err) from exc

    _validate_policy_semantics(config, host_owner=host_owner)
    return config


def _resolve_policy(
    *,
    host_owner: str,
    policy_name: str,
    selector: ResourceRecordSelector | None,
    explicit_policy: ResourceEvictionPolicy | ResourceKeepalivePolicy | None,
) -> ResourceEvictionPolicy | ResourceKeepalivePolicy | None:
    if selector is not None and explicit_policy is not None:
        err = Error(
            code=Errs.RESOURCE_POLICY_CONFLICT,
            source=host_owner,
            route=f"resource_host.policy_config.{policy_name}",
            evidence={
                "policy": policy_name,
                "reason": "selector_and_callable_both_provided",
                "callable_type": type(explicit_policy).__qualname__,
            },
        )
        raise ResourcePolicyConflictError(err)
    if explicit_policy is not None:
        return explicit_policy
    if selector is None:
        return None
    return selector.matches


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
        policy_config: ResourceHostPolicyConfig | Mapping[str, object] | None = None,
        eviction_policy: ResourceEvictionPolicy | None = None,
        keepalive_policy: ResourceKeepalivePolicy | None = None,
    ) -> None:
        self.owner = owner
        self._scope = PluginScope(owner)
        self._capacities: dict[CapabilityName, _Capacity] = {}
        self._leases: set[ResourceLease] = set()
        self._handles: dict[RefId, _ResourceEntry] = {}
        self.policy_config = _normalize_policy_config(
            policy_config,
            host_owner=owner,
        )
        self._eviction_policy = _resolve_policy(
            host_owner=owner,
            policy_name="eviction",
            selector=None if self.policy_config is None else self.policy_config.eviction,
            explicit_policy=eviction_policy,
        )
        self._keepalive_policy = _resolve_policy(
            host_owner=owner,
            policy_name="keepalive",
            selector=None if self.policy_config is None else self.policy_config.keepalive,
            explicit_policy=keepalive_policy,
        )
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
    "ResourcePolicyConfigError",
    "ResourcePolicyConflictError",
    "ResourceRecord",
]
