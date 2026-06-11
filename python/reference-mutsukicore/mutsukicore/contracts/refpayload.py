"""通用按引用 payload 协议 —— 领域中立。

核心只知道按引用传输的「形状」：``Handle[T]`` 持有非可序列化对象的所有权，
``RefDescriptor`` 携带可观测元数据，codec 必须拒绝序列化 :class:`RefPayload`。
具体的领域语义（latent 张量、KV 缓存槽、显存）完全由领域契约包定义，不在此处。

完整设计参见 :doc:`contracts §11 <plans/contracts>`。
"""

from __future__ import annotations

from abc import ABC, abstractmethod
from contextlib import AbstractContextManager
from enum import StrEnum
from typing import TYPE_CHECKING, Any, ClassVar, Generic, TypeVar

import msgspec

from mutsukicore.contracts.base import Contract
from mutsukicore.contracts.ids import RefId

if TYPE_CHECKING:
    from mutsukicore.core.scope import PluginScope, TransactionScope


T = TypeVar("T")


class Replayability(StrEnum):
    FULL = "full"
    INPUT_SEED_ONLY = "input_seed_only"
    NONE = "none"


class RefDescriptor(Contract):
    """按引用 payload 的可观测元数据。永远可序列化。"""

    schema_id: ClassVar[str] = "mutsukicore.ref_descriptor"
    schema_version: ClassVar[str] = "1.0.0"

    ref_id: RefId
    kind: str
    schema_id_target: str
    schema_version_target: str
    attributes: dict[str, str | int | float | bool] = {}
    lineage: tuple[RefId, ...] = ()


class Handle(ABC, Generic[T]):
    """按引用 payload 的所有权抽象句柄。

    具体实现见 :mod:`mutsukicore.core.handle`。子类通过 ``__init_subclass__``
    自动注册到 :class:`HandleRegistry`。
    """

    @abstractmethod
    def acquire(self) -> T:
        """获取强引用；引用计数 +1。"""

    @abstractmethod
    def release(self) -> None:
        """释放一次引用；归零时触发 finalizer。"""

    @abstractmethod
    def borrow(self) -> AbstractContextManager[T]:
        """短期借用；上下文退出自动释放。"""

    @abstractmethod
    def is_alive(self) -> bool:
        """底层对象未被回收时返回 True。"""

    @abstractmethod
    def attach_to(self, scope: "PluginScope | TransactionScope") -> None:
        """把本句柄绑定到 scope；scope.close() 会负责释放。"""

    @property
    @abstractmethod
    def ref_id(self) -> RefId: ...

    @property
    @abstractmethod
    def descriptor(self) -> RefDescriptor: ...


class RefPayload(Contract, Generic[T]):
    """字段标记：表明 payload 通过引用持有。

    ``handle`` 字段携带不可序列化的 :class:`Handle`。检查到此结构的 codec
    必须二选一：

    * 拒绝编码（``Errs.REF_SERIALIZE_ATTEMPT``）；
    * 或者把 handle 替换为 descriptor（仅在显式降级用于 trace/audit 时，
      详见 :doc:`contracts §11.8 <plans/contracts>`）。
    """

    schema_id: ClassVar[str] = "mutsukicore.ref_payload"
    schema_version: ClassVar[str] = "1.0.0"

    ref_id: RefId
    handle: Handle[Any] = msgspec.field()
    descriptor: RefDescriptor


class BackpressureChannel(ABC, Generic[T]):
    """带高/低水位反压的有界异步通道。"""

    high_watermark: int
    low_watermark: int

    @abstractmethod
    async def send(self, item: T) -> None: ...

    @abstractmethod
    async def recv(self) -> T: ...

    @property
    @abstractmethod
    def closed(self) -> bool: ...

    @abstractmethod
    def close(self) -> None: ...


__all__ = [
    "BackpressureChannel",
    "Handle",
    "RefDescriptor",
    "RefPayload",
    "Replayability",
]
