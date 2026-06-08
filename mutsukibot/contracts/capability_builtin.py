"""框架内置的 capability 常量门面。

:class:`Caps` 门面让 pyright/IDE 能够准确推断 MutsukiBot 核心 capability 名。
插件应当采用相同模式，自建门面类（例如
``YumeCaps.VRAM = CapabilityName.register("yume.vram", ...)``）。
"""

from __future__ import annotations

from typing import ClassVar

from mutsukibot.contracts.capability import CapabilityName

_OWNER = "mutsukibot.core"


class Caps:
    """所有 MutsukiBot 框架内置 capability 常量。

    协议或领域能力由 extension / plugin 用 ``CapabilityName.register(...)``
    自行声明。
    """

    READ_MESSAGE: ClassVar[CapabilityName]
    SEND_MESSAGE: ClassVar[CapabilityName]
    CALL_LLM: ClassVar[CapabilityName]
    PERSIST: ClassVar[CapabilityName]
    NETWORK_EGRESS: ClassVar[CapabilityName]
    SPAWN_AGENT: ClassVar[CapabilityName]
    HOLD_REF: ClassVar[CapabilityName]
    BORROW_REF: ClassVar[CapabilityName]
    PRODUCE_REF_STREAM: ClassVar[CapabilityName]
    # Deprecated compatibility aliases; canonical names live in
    # ``mutsukibot_ext.im.IMCaps``.
    IM_TEXT: ClassVar[CapabilityName]
    IM_IMAGE: ClassVar[CapabilityName]
    IM_AUDIO: ClassVar[CapabilityName]
    IM_FILE: ClassVar[CapabilityName]
    IM_MARKDOWN: ClassVar[CapabilityName]
    IM_CARD: ClassVar[CapabilityName]
    IM_REACTION: ClassVar[CapabilityName]
    IM_TYPING: ClassVar[CapabilityName]


CapabilityName.bootstrap_facade(
    Caps,
    {
        "READ_MESSAGE": "read_message",
        "SEND_MESSAGE": "send_message",
        "CALL_LLM": "call_llm",
        "PERSIST": "persist",
        "NETWORK_EGRESS": "network_egress",
        "SPAWN_AGENT": "spawn_agent",
        "HOLD_REF": "hold_ref",
        "BORROW_REF": "borrow_ref",
        "PRODUCE_REF_STREAM": "produce_ref_stream",
    },
    declared_by=_OWNER,
)

from mutsukibot_ext.im import IMCaps

Caps.IM_TEXT = IMCaps.TEXT
Caps.IM_IMAGE = IMCaps.IMAGE
Caps.IM_AUDIO = IMCaps.AUDIO
Caps.IM_FILE = IMCaps.FILE
Caps.IM_MARKDOWN = IMCaps.MARKDOWN
Caps.IM_CARD = IMCaps.CARD
Caps.IM_REACTION = IMCaps.REACTION
Caps.IM_TYPING = IMCaps.TYPING


__all__ = ["Caps"]
