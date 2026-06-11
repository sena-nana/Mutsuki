"""Contract 基类与全局 SchemaRegistry。

每个契约对象都继承 :class:`Contract`，它是 ``msgspec.Struct`` 的子类，
要求所有具体子类声明 ``schema_id`` 与 ``schema_version`` 这两个 ClassVar。
``__init_subclass__`` 与 :func:`SchemaRegistry.register` 协作，确保 schema
标识符在进程内唯一且可按 id 解析。
"""

from __future__ import annotations

from typing import ClassVar, Final

import msgspec


class _SchemaRegistry:
    """进程内的 Contract 子类全局注册表。

    映射：``schema_id`` → 类型。兼容回调存放在
    :mod:`mutsukicore.contracts.schema`。
    """

    def __init__(self) -> None:
        self._by_id: dict[str, type[Contract]] = {}

    def register(self, schema_id: str, schema_version: str, cls: type[Contract]) -> None:
        existing = self._by_id.get(schema_id)
        if existing is not None and existing is not cls:
            raise SchemaConflictError(
                f"schema_id {schema_id!r} 已被 "
                f"{existing.__module__}.{existing.__qualname__} 注册，"
                f"{cls.__module__}.{cls.__qualname__} 不可重复注册"
            )
        self._by_id[schema_id] = cls

    def get(self, schema_id: str) -> type[Contract] | None:
        return self._by_id.get(schema_id)

    def all(self) -> list[tuple[str, str, type[Contract]]]:
        return [(sid, cls.schema_version, cls) for sid, cls in self._by_id.items()]


class SchemaConflictError(Exception):
    """两个不同的类型试图注册同一个 schema_id 时抛出。"""


SchemaRegistry: Final[_SchemaRegistry] = _SchemaRegistry()


class Contract(msgspec.Struct, kw_only=True):
    """所有 MutsukiCore 契约对象的基类。

    子类必须以字符串常量形式定义 ``schema_id`` 与 ``schema_version``
    两个 ``ClassVar``。``__init_subclass__`` 会自动把它们注册到
    :data:`SchemaRegistry`。
    """

    schema_id: ClassVar[str] = ""
    schema_version: ClassVar[str] = ""

    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)
        # 跳过未自带 schema_id 的中间抽象基类。
        sid = cls.__dict__.get("schema_id")
        sver = cls.__dict__.get("schema_version")
        if not sid or not sver:
            return
        SchemaRegistry.register(sid, sver, cls)


__all__ = ["Contract", "SchemaConflictError", "SchemaRegistry"]
