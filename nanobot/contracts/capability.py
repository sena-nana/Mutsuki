"""Capability —— 「插件能做什么」的静态声明。

详见 :doc:`contracts §4 <plans/contracts>` 关于 capability 的定位。
``CapabilityName`` 是可扩展的注册式字符串类型；框架通过
:class:`Caps`（在 :mod:`nanobot.contracts.capability_builtin`）暴露内置常量，
插件可以注册自有命名空间（如 ``yume.vram``）。
"""

from __future__ import annotations

from typing import ClassVar

from nanobot.contracts._registered import RegisteredString
from nanobot.contracts.base import Contract


class UnknownCapabilityError(Exception):
    """构造未注册的 capability 名时抛出。"""


class CapabilityConflictError(Exception):
    """两个不同插件试图注册同一 capability 名时抛出。"""


class CapabilityName(RegisteredString):
    """已注册的 capability 名（``str`` 子类）。

    构造时强制要求已注册：``CapabilityName("read_message")`` 仅对此前调用过
    :meth:`register` 的名字有效。插件通过 :meth:`register` 添加新名字。
    """

    _noun: ClassVar[str] = "capability"
    _unknown_error: ClassVar[type[Exception]] = UnknownCapabilityError
    _conflict_error: ClassVar[type[Exception]] = CapabilityConflictError


class Capability(Contract):
    """带资源量纲的 capability 声明。"""

    schema_id: ClassVar[str] = "nanobot.capability"
    schema_version: ClassVar[str] = "1.0.0"

    name: CapabilityName
    quantity: dict[str, int | str] | None = None
    policy: dict[str, str] | None = None


__all__ = [
    "Capability",
    "CapabilityConflictError",
    "CapabilityName",
    "UnknownCapabilityError",
]
