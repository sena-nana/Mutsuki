"""消息与内容片段契约。

v0.2 起 ``Message`` 是 :class:`mutsukibot.contracts.envelope.Envelope` 的 IM
特化；``ChannelRef`` 是 :class:`mutsukibot.contracts.envelope.SourceRef` 的
IM 特化。原 ``ChannelRef.adapter_id`` 字段已重命名为 ``source_id``（继承自
SourceRef）—— 见 contracts.md §16 与 D1。
"""

from __future__ import annotations

from enum import StrEnum
from typing import ClassVar

from mutsukibot.contracts.base import Contract
from mutsukibot.contracts.envelope import Envelope, SourceRef
from mutsukibot.contracts.refpayload import RefDescriptor


class ContentKind(StrEnum):
    TEXT = "text"
    IMAGE_REF = "image_ref"
    AUDIO_REF = "audio_ref"
    FILE_REF = "file_ref"
    LATENT_REF = "latent_ref"
    TOOL_SCHEMA_REF = "tool_schema_ref"


class ChannelRef(SourceRef):
    """IM 消息来源所在频道的指针 —— SourceRef 的 IM 特化。

    ``source_id`` 继承自 SourceRef（v0.1 名为 ``adapter_id``，v0.2 重命名以
    与统一的 endpoint 命名空间对齐；``kind`` 通常为 ``SourceKinds.IM``）。
    """

    schema_id: ClassVar[str] = "mutsukibot.channel_ref"
    schema_version: ClassVar[str] = "1.0.0"

    channel_id: str
    user_id: str | None = None


class ContentPart(Contract):
    """消息内容的一个切片（文本或按引用 payload）。"""

    schema_id: ClassVar[str] = "mutsukibot.content_part"
    schema_version: ClassVar[str] = "1.0.0"

    kind: ContentKind
    text: str | None = None
    ref: RefDescriptor | None = None
    metadata: dict[str, str] = {}


class Message(Envelope):
    """入站或出站消息 —— Envelope 的 IM 特化。

    ``id / timestamp / source / capabilities_required`` 继承自 Envelope。
    ``payload_schema_id`` 在 v0.2 默认为 ``"mutsukibot.message"``。
    ``source`` 字段类型在运行时为 :class:`ChannelRef`（SourceRef 子类）；
    msgspec 不在结构化继承中收窄字段类型，由调用方保证。
    """

    schema_id: ClassVar[str] = "mutsukibot.message"
    schema_version: ClassVar[str] = "1.0.0"

    parts: tuple[ContentPart, ...] = ()

    @property
    def text(self) -> str:
        """所有 TEXT 片段的拼接（便利属性）。"""
        return "".join(p.text or "" for p in self.parts if p.kind == ContentKind.TEXT)


__all__ = ["ChannelRef", "ContentKind", "ContentPart", "Message"]
