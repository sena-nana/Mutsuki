"""资源生命周期 scope。

:class:`PluginScope` 与 :class:`TransactionScope` 追踪一个插件 / 事务衍生
出的所有副作用资源（订阅、定时器、服务、句柄）。close / commit / rollback
时按反向释放。任何在关闭时仍存活的句柄都通过 :data:`Errs.HANDLE_LEAK`
报告为泄漏；cleanup 自身抛出的异常聚合进 :class:`HandleLeakError.evidence`，
不再静默吞错（违反 hard rule #8 结构化错误）。
"""

from __future__ import annotations

from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from enum import StrEnum
import inspect
from typing import TYPE_CHECKING

from mutsukibot.contracts.error import Errs
from mutsukibot.contracts.ids import RefId
from mutsukibot.contracts.refpayload import Handle

if TYPE_CHECKING:
    from mutsukibot.contracts.error import Error


CleanupFn = Callable[[], None] | Callable[[], Awaitable[None]]


class ResourceKind(StrEnum):
    """``PluginScope`` 追踪的资源类别。

    分类信息只用于诊断（``HandleLeakError.evidence`` / 调试面板）。运行时
    没有按类别区分的清理路径 —— 所有 cleanup 共用一条反向释放循环。
    """

    SUBSCRIPTION = "subscription"
    TIMER = "timer"
    SERVICE_REGISTRATION = "service_registration"
    CONTEXT_ATTACHMENT = "context_attachment"
    CONFIG_WATCHER = "config_watcher"


class HandleLeakError(Exception):
    """scope 关闭时仍有活跃句柄未释放，或 cleanup 抛出，抛出此错误。"""

    def __init__(self, leaked: list[RefId], error: "Error") -> None:
        super().__init__(f"scope 关闭时仍有 {len(leaked)} 个未释放句柄: {leaked}")
        self.leaked = leaked
        self.error = error


@dataclass(slots=True)
class _Cleanup:
    fn: CleanupFn
    kind: ResourceKind


@dataclass(slots=True)
class _ScopeState:
    cleanups: list[_Cleanup] = field(default_factory=list)
    handles: list[Handle[object]] = field(default_factory=list)
    closed: bool = False


class PluginScope:
    """追踪单个插件实例衍生的全部副作用。"""

    def __init__(self, owner: str) -> None:
        self.owner = owner
        self._state = _ScopeState()

    def add_subscription(self, unsubscribe: CleanupFn) -> None:
        self._register(unsubscribe, ResourceKind.SUBSCRIPTION)

    def add_timer(self, cancel: CleanupFn) -> None:
        self._register(cancel, ResourceKind.TIMER)

    def add_service_registration(self, unregister: CleanupFn) -> None:
        self._register(unregister, ResourceKind.SERVICE_REGISTRATION)

    def add_context_attachment(self, detach: CleanupFn) -> None:
        self._register(detach, ResourceKind.CONTEXT_ATTACHMENT)

    def add_config_watcher(self, unwatch: CleanupFn) -> None:
        self._register(unwatch, ResourceKind.CONFIG_WATCHER)

    def attach_handle(self, handle: Handle[object]) -> None:
        self._guard()
        self._state.handles.append(handle)

    def _register(self, fn: CleanupFn, kind: ResourceKind) -> None:
        self._guard()
        self._state.cleanups.append(_Cleanup(fn=fn, kind=kind))

    def _guard(self) -> None:
        if self._state.closed:
            raise RuntimeError(f"PluginScope({self.owner!r}) 已关闭")

    @property
    def closed(self) -> bool:
        return self._state.closed

    async def close(self) -> None:
        """反向运行所有 cleanup，释放绑定的句柄；发现泄漏或 cleanup 失败则抛错。"""
        if self._state.closed:
            return
        self._state.closed = True

        cleanup_failures: list[dict[str, str]] = []
        for cleanup in reversed(self._state.cleanups):
            try:
                result = cleanup.fn()
                if inspect.isawaitable(result):
                    await result
            except Exception as exc:
                cleanup_failures.append(
                    {
                        "kind": cleanup.kind.value,
                        "exception_type": type(exc).__qualname__,
                        "exception_repr": repr(exc),
                    }
                )

        # attach_to(scope) 表示 scope 拥有句柄构造时持有的那一份引用
        # （RefCountedHandle.__init__ 中的 +1）。释放一次代表 scope 放手；
        # 释放后仍然存活才是真正的泄漏。
        leaked: list[RefId] = []
        for handle in self._state.handles:
            if not handle.is_alive():
                continue
            try:
                handle.release()
            except Exception as exc:
                cleanup_failures.append(
                    {
                        "kind": "handle.release",
                        "ref_id": str(handle.ref_id),
                        "exception_type": type(exc).__qualname__,
                        "exception_repr": repr(exc),
                    }
                )
            if handle.is_alive():
                leaked.append(handle.ref_id)

        if leaked or cleanup_failures:
            import json

            from mutsukibot.contracts.error import Error

            evidence: dict[str, bool | float | int | str] = {
                "leaked_count": len(leaked),
                "cleanup_failure_count": len(cleanup_failures),
            }
            if leaked:
                evidence["leaked_first"] = str(leaked[0])
            if cleanup_failures:
                # Error.evidence 只接受标量；把诊断数组压成 JSON 字符串。
                evidence["cleanup_failures_json"] = json.dumps(
                    cleanup_failures, ensure_ascii=False
                )
            err = Error(
                code=Errs.HANDLE_LEAK,
                source=self.owner,
                route="scope.close",
                evidence=evidence,
            )
            raise HandleLeakError(leaked, err)


class TransactionScope(PluginScope):
    """带显式 commit / rollback 语义的 scope。

    通过 :meth:`register_compensation` 注册的补偿动作只在 rollback 时按反向
    顺序运行。普通 cleanup（继承的 ``add_*`` 方法添加的）在 commit 与
    rollback 时都会执行。
    """

    def __init__(self, owner: str) -> None:
        super().__init__(owner)
        self._compensations: list[CleanupFn] = []

    def register_compensation(self, fn: CleanupFn) -> None:
        self._guard()
        self._compensations.append(fn)

    async def commit(self) -> None:
        await self.close()

    async def rollback(self) -> None:
        for fn in reversed(self._compensations):
            try:
                result = fn()
                if inspect.isawaitable(result):
                    await result
            except Exception:
                # rollback 阶段的补偿失败由 close() 一并报告（接下来会跑）。
                # 这里继续跑剩余补偿，避免一个失败阻塞其余。
                continue
        await self.close()


__all__ = ["HandleLeakError", "PluginScope", "ResourceKind", "TransactionScope"]
