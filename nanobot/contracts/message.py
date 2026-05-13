"""消息与内容片段契约。"""

from __future__ import annotations

from enum import StrEnum
from typing import ClassVar

from nanobot.contracts.base import Contract
from nanobot.contracts.capability import CapabilityName
from nanobot.contracts.ids import MessageId
from nanobot.contracts.refpayload import RefDescriptor


class ContentKind(StrEnum):
    TEXT = "text"
    IMAGE_REF = "image_ref"
    AUDIO_REF = "audio_ref"
    FILE_REF = "file_ref"
    LATENT_REF = "latent_ref"
    TOOL_SCHEMA_REF = "tool_schema_ref"


class ChannelRef(Contract):
    """消息来源所在频道的指针。"""

    schema_id: ClassVar[str] = "nanobot.channel_ref"
    schema_version: ClassVar[str] = "1.0.0"

    adapter_id: str
    channel_id: str
    user_id: str | None = None


class ContentPart(Contract):
    """消息内容的一个切片（文本或按引用 payload）。"""

    schema_id: ClassVar[str] = "nanobot.content_part"
    schema_version: ClassVar[str] = "1.0.0"

    kind: ContentKind
    text: str | None = None
    ref: RefDescriptor | None = None
    metadata: dict[str, str] = {}


class Message(Contract):
    """入站或出站消息的封装。"""

    schema_id: ClassVar[str] = "nanobot.message"
    schema_version: ClassVar[str] = "1.0.0"

    id: MessageId
    timestamp: float
    source: ChannelRef
    parts: tuple[ContentPart, ...]
    capabilities_required: tuple[CapabilityName, ...] = ()

    @property
    def text(self) -> str:
        """所有 TEXT 片段的拼接（便利属性）。"""
        return "".join(p.text or "" for p in self.parts if p.kind == ContentKind.TEXT)


__all__ = ["ChannelRef", "ContentKind", "ContentPart", "Message"]
