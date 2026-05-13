"""Adapter 虚拟基类。Adapter 子类通过 ``__init_subclass__`` 自动注册。"""

from __future__ import annotations

from abc import ABC, abstractmethod
from enum import StrEnum
from typing import TYPE_CHECKING, ClassVar

from nanobot.core.registry import AdapterRegistry

if TYPE_CHECKING:
    from nanobot.contracts.message import Message
    from nanobot.core.agent import Agent


class AdapterCapability(StrEnum):
    TEXT = "text"
    IMAGE = "image"
    AUDIO = "audio"
    FILE = "file"
    MARKDOWN = "markdown"
    CARD = "card"
    REACTION = "reaction"
    TYPING = "typing"


class Adapter(ABC):
    adapter_id: ClassVar[str] = ""
    supports: ClassVar[tuple[AdapterCapability, ...]] = ()

    def __init_subclass__(cls, **kwargs: object) -> None:
        super().__init_subclass__(**kwargs)
        if cls.adapter_id:
            AdapterRegistry.register(cls.adapter_id, cls)

    @abstractmethod
    async def deliver(self, agent: "Agent", message: "Message") -> None: ...

    @abstractmethod
    async def receive(self, agent: "Agent") -> "Message | None": ...


__all__ = ["Adapter", "AdapterCapability"]
