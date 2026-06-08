"""Instant-messaging reference extension.

This module owns the IM-specialized contract shapes and named routing helpers
that used to be exposed as core defaults.
"""

from __future__ import annotations

from enum import StrEnum
from typing import ClassVar

from mutsukibot.contracts.base import Contract
from mutsukibot.contracts.capability import CapabilityName
from mutsukibot.contracts.envelope import Envelope, SourceRef
from mutsukibot.contracts.refpayload import RefDescriptor
from mutsukibot.contracts.scope import ByCapability, BySchema, BySourceKind, ScopeName
from mutsukibot.contracts.source import SourceKindName

_OWNER = "mutsukibot_ext.im"


class IMCaps:
    """IM capability names provided by the IM reference extension."""

    TEXT: ClassVar[CapabilityName]
    IMAGE: ClassVar[CapabilityName]
    AUDIO: ClassVar[CapabilityName]
    FILE: ClassVar[CapabilityName]
    MARKDOWN: ClassVar[CapabilityName]
    CARD: ClassVar[CapabilityName]
    REACTION: ClassVar[CapabilityName]
    TYPING: ClassVar[CapabilityName]


CapabilityName.bootstrap_facade(
    IMCaps,
    {
        "TEXT": "im.text",
        "IMAGE": "im.image",
        "AUDIO": "im.audio",
        "FILE": "im.file",
        "MARKDOWN": "im.markdown",
        "CARD": "im.card",
        "REACTION": "im.reaction",
        "TYPING": "im.typing",
    },
    declared_by=_OWNER,
)


class IMSourceKinds:
    """IM source kind names provided by the IM reference extension."""

    IM: ClassVar[SourceKindName]


SourceKindName.bootstrap_facade(
    IMSourceKinds,
    {"IM": "im"},
    declared_by=_OWNER,
)


class ContentKind(StrEnum):
    TEXT = "text"
    IMAGE_REF = "image_ref"
    AUDIO_REF = "audio_ref"
    FILE_REF = "file_ref"
    LATENT_REF = "latent_ref"
    TOOL_SCHEMA_REF = "tool_schema_ref"


class ChannelRef(SourceRef):
    """IM channel pointer."""

    schema_id: ClassVar[str] = "mutsukibot.channel_ref"
    schema_version: ClassVar[str] = "1.0.0"

    channel_id: str
    user_id: str | None = None


class ContentPart(Contract):
    """One IM message content segment."""

    schema_id: ClassVar[str] = "mutsukibot.content_part"
    schema_version: ClassVar[str] = "1.0.0"

    kind: ContentKind
    text: str | None = None
    ref: RefDescriptor | None = None
    metadata: dict[str, str] = {}


class Message(Envelope):
    """IM-specialized envelope."""

    schema_id: ClassVar[str] = "mutsukibot.message"
    schema_version: ClassVar[str] = "1.0.0"

    parts: tuple[ContentPart, ...] = ()

    @property
    def text(self) -> str:
        """All text segments joined for convenience."""
        return "".join(p.text or "" for p in self.parts if p.kind == ContentKind.TEXT)


class IMScopes:
    """Named IM scopes provided by the IM reference extension."""

    TEXT: ClassVar[ScopeName]
    ANY: ClassVar[ScopeName]


IMScopes.TEXT = ScopeName.register(
    "im.text",
    declared_by=_OWNER,
    rule=BySchema("mutsukibot.message")
    & BySourceKind(IMSourceKinds.IM)
    & ByCapability(IMCaps.TEXT),
)
IMScopes.ANY = ScopeName.register(
    "im.any",
    declared_by=_OWNER,
    rule=BySchema("mutsukibot.message") & BySourceKind(IMSourceKinds.IM),
)


__all__ = [
    "ChannelRef",
    "ContentKind",
    "ContentPart",
    "IMCaps",
    "IMScopes",
    "IMSourceKinds",
    "Message",
]
