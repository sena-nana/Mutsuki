"""Schema 兼容回调注册表。

按 :doc:`contracts §10.2 <plans/contracts>` 的约定，核心不内置任何具体的
兼容规则。契约包自行注册回调 ``(producer_version, consumer_version) -> bool``。
未注册时的默认策略：版本字节相等才视为兼容。
"""

from __future__ import annotations

from collections.abc import Callable
from typing import Final

CompatibilityFn = Callable[[str, str], bool]


def _default_compat(producer: str, consumer: str) -> bool:
    return producer == consumer


class _CompatRegistry:
    def __init__(self) -> None:
        self._fns: dict[str, CompatibilityFn] = {}

    def register(self, schema_id: str, fn: CompatibilityFn) -> None:
        self._fns[schema_id] = fn

    def is_compatible(self, schema_id: str, producer: str, consumer: str) -> bool:
        fn = self._fns.get(schema_id, _default_compat)
        return fn(producer, consumer)


_REGISTRY: Final[_CompatRegistry] = _CompatRegistry()


def register_schema_compatibility(schema_id: str, fn: CompatibilityFn) -> None:
    """为指定 ``schema_id`` 注册兼容性谓词。"""
    _REGISTRY.register(schema_id, fn)


def is_compatible(schema_id: str, producer_version: str, consumer_version: str) -> bool:
    """检查同一 schema 的两个版本是否兼容。"""
    return _REGISTRY.is_compatible(schema_id, producer_version, consumer_version)


__all__ = ["CompatibilityFn", "is_compatible", "register_schema_compatibility"]
