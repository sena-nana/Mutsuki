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

    ``IM_*`` / ``TOOL_*`` 系列（v0.2 引入）取代旧的 ``AdapterCapability``
    StrEnum；详见 [contracts.md §4](../../plans/contracts.md#4-capability-命名)。
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
    IM_TEXT: ClassVar[CapabilityName]
    IM_IMAGE: ClassVar[CapabilityName]
    IM_AUDIO: ClassVar[CapabilityName]
    IM_FILE: ClassVar[CapabilityName]
    IM_MARKDOWN: ClassVar[CapabilityName]
    IM_CARD: ClassVar[CapabilityName]
    IM_REACTION: ClassVar[CapabilityName]
    IM_TYPING: ClassVar[CapabilityName]
    TOOL_INVOKE: ClassVar[CapabilityName]
    TOOL_EVENT: ClassVar[CapabilityName]


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
        "IM_TEXT": "im.text",
        "IM_IMAGE": "im.image",
        "IM_AUDIO": "im.audio",
        "IM_FILE": "im.file",
        "IM_MARKDOWN": "im.markdown",
        "IM_CARD": "im.card",
        "IM_REACTION": "im.reaction",
        "IM_TYPING": "im.typing",
        "TOOL_INVOKE": "tool.invoke",
        "TOOL_EVENT": "tool.event",
    },
    declared_by=_OWNER,
)


__all__ = ["Caps"]
