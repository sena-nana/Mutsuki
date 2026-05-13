"""进程级全局注册表。

每个注册表都是 ``dict`` 的薄包装，提供显式的 register / get / iter 接口。
:class:`Plugin` / :class:`Adapter` / :class:`Service` / :class:`Handle` 的子类
会自动登记进对应注册表（通过 ``__init_subclass__``，或对 :class:`Plugin`
而言通过 :class:`PluginMeta`）。
"""

from __future__ import annotations

from collections.abc import Iterator
from typing import TYPE_CHECKING, Generic, TypeVar

if TYPE_CHECKING:
    from nanobot.adapters.base import Adapter
    from nanobot.core.handle import HandleImpl
    from nanobot.core.plugin import Plugin


_T = TypeVar("_T")


class _NamedRegistry(Generic[_T]):
    def __init__(self, kind: str) -> None:
        self._kind = kind
        self._items: dict[str, _T] = {}

    def register(self, key: str, value: _T) -> None:
        existing = self._items.get(key)
        if existing is not None and existing is not value:
            raise RegistryConflictError(
                f"{self._kind} {key!r} 已被注册成不同的值"
            )
        self._items[key] = value

    def unregister(self, key: str) -> None:
        self._items.pop(key, None)

    def get(self, key: str) -> _T | None:
        return self._items.get(key)

    def require(self, key: str) -> _T:
        item = self._items.get(key)
        if item is None:
            raise KeyError(f"{self._kind} {key!r} 未注册")
        return item

    def __iter__(self) -> Iterator[tuple[str, _T]]:
        return iter(self._items.items())

    def __contains__(self, key: object) -> bool:
        return key in self._items

    def __len__(self) -> int:
        return len(self._items)

    def clear(self) -> None:
        self._items.clear()


class RegistryConflictError(Exception):
    """同一 key 被注册成相互冲突的值时抛出。"""


PluginRegistry: _NamedRegistry["type[Plugin]"] = _NamedRegistry("Plugin")
AdapterRegistry: _NamedRegistry["type[Adapter]"] = _NamedRegistry("Adapter")
HandleRegistry: _NamedRegistry["type[HandleImpl]"] = _NamedRegistry("Handle")


__all__ = [
    "AdapterRegistry",
    "HandleRegistry",
    "PluginRegistry",
    "RegistryConflictError",
]
