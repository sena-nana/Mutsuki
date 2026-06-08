"""Service 注入协议与模式枚举。"""

from __future__ import annotations

from enum import StrEnum
from typing import Protocol, runtime_checkable


class ServiceMode(StrEnum):
    BY_VALUE = "by_value"
    BY_REF = "by_ref"


@runtime_checkable
class Service(Protocol):
    """可注册到容器的服务标记 Protocol。

    具体服务应继承一个提供 ``__init_subclass__`` 自动注册逻辑的基类，
    详见 :mod:`mutsukibot.core.registry`。
    """

    service_id: str


__all__ = ["Service", "ServiceMode"]
