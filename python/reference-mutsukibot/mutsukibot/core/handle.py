"""引用计数句柄实现。

框架内置一个具体实现 :class:`RefCountedHandle`。领域插件可以子类化以提供
自定义 finalizer（释放后端资源、归还槽位到池），子类通过
``__init_subclass__`` 自动注册到 :data:`HandleRegistry`。
"""

from __future__ import annotations

from collections.abc import Callable
from contextlib import contextmanager
from typing import TYPE_CHECKING, Any, Generic, TypeVar, cast

from mutsukibot.contracts.error import Error, Errs
from mutsukibot.contracts.ids import RefId
from mutsukibot.contracts.refpayload import Handle, RefDescriptor
from mutsukibot.core.registry import HandleRegistry

if TYPE_CHECKING:
    from collections.abc import Generator

    from mutsukibot.core.scope import PluginScope, TransactionScope


T = TypeVar("T")


class HandleUseAfterReleaseError(Exception):
    def __init__(self, ref_id: RefId, error: Error) -> None:
        super().__init__(f"句柄 {ref_id!r} 在释放后仍被使用")
        self.ref_id = ref_id
        self.error = error


class HandleNotAttachedError(Exception):
    def __init__(self, ref_id: RefId, error: Error) -> None:
        super().__init__(f"句柄 {ref_id!r} 未先调用 attach_to(scope)")
        self.ref_id = ref_id
        self.error = error


class HandleImpl(Handle[T], Generic[T]):
    """具体 Handle 实现的基类。

    子类自动登记到 :data:`HandleRegistry`。重写 :meth:`_finalize` 提供
    自定义清理逻辑（默认：调用用户传入的 finalizer，如果有的话）。
    """

    handle_kind: str = "generic"

    def __init_subclass__(cls, **kwargs: object) -> None:
        super().__init_subclass__(**kwargs)
        HandleRegistry.register(cls.handle_kind, cls)


class RefCountedHandle(HandleImpl[T], Generic[T]):
    """通用的引用计数句柄。供测试与 stub fixture 使用。

    按 :doc:`contracts §11.2 <plans/contracts>` 的约定，每个句柄在第一次
    非构造性使用之前必须 :meth:`attach_to` 一个 scope。本实现会在第一次
    :meth:`acquire` / :meth:`borrow` 时强制检查。
    """

    handle_kind = "generic"

    def __init__(
        self,
        target: T,
        descriptor: RefDescriptor,
        finalizer: Callable[[T], None] | None = None,
    ) -> None:
        self._target: T | None = target
        self._descriptor = descriptor
        self._finalizer = finalizer
        self._refcount = 1  # 构造持有 1
        self._attached = False
        self._released = False

    @property
    def ref_id(self) -> RefId:
        return self._descriptor.ref_id

    @property
    def descriptor(self) -> RefDescriptor:
        return self._descriptor

    def attach_to(self, scope: "PluginScope | TransactionScope") -> None:
        scope.attach_handle(cast(Handle[Any], self))
        self._attached = True

    def _check_attached(self) -> None:
        if not self._attached:
            err = Error(
                code=Errs.HANDLE_LEAK,
                source="core.handle",
                route="handle.acquire",
                evidence={"ref_id": self._descriptor.ref_id},
            )
            raise HandleNotAttachedError(self._descriptor.ref_id, err)

    def acquire(self) -> T:
        self._check_attached()
        if self._released or self._target is None:
            err = Error(
                code=Errs.HANDLE_USE_AFTER_RELEASE,
                source="core.handle",
                route="handle.acquire",
                evidence={"ref_id": self._descriptor.ref_id},
            )
            raise HandleUseAfterReleaseError(self._descriptor.ref_id, err)
        self._refcount += 1
        return self._target

    def release(self) -> None:
        if self._released:
            return
        self._refcount -= 1
        if self._refcount <= 0:
            self._released = True
            target = self._target
            self._target = None
            if target is not None and self._finalizer is not None:
                self._finalizer(target)

    @contextmanager
    def borrow(self) -> "Generator[T]":
        target = self.acquire()
        try:
            yield target
        finally:
            self.release()

    def is_alive(self) -> bool:
        return not self._released


def make_stub_handle(
    ref_id: RefId,
    *,
    kind: str = "test.stub",
    schema_id_target: str = "test.stub/v1",
    schema_version_target: str = "1.0.0",
    target: object = None,
    attributes: dict[str, str | int | float | bool] | None = None,
) -> RefCountedHandle[object]:
    """测试辅助：在没有真实后端时生成可观测的假句柄。"""
    descriptor = RefDescriptor(
        ref_id=ref_id,
        kind=kind,
        schema_id_target=schema_id_target,
        schema_version_target=schema_version_target,
        attributes=attributes or {},
    )
    return RefCountedHandle(
        target=target if target is not None else object(),
        descriptor=descriptor,
    )


__all__ = [
    "HandleImpl",
    "HandleNotAttachedError",
    "HandleUseAfterReleaseError",
    "RefCountedHandle",
    "make_stub_handle",
]
