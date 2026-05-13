"""框架内置的 capability 常量门面。

:class:`Caps` 门面让 pyright/IDE 能够准确推断 NanoBot 核心 capability 名。
插件应当采用相同模式，自建门面类（例如
``YumeCaps.VRAM = CapabilityName.register("yume.vram", ...)``）。
"""

from __future__ import annotations

from typing import ClassVar

from nanobot.contracts.capability import CapabilityName

_OWNER = "nanobot.core"


class Caps:
    """所有 NanoBot 框架内置 capability 常量。"""

    READ_MESSAGE: ClassVar[CapabilityName]
    SEND_MESSAGE: ClassVar[CapabilityName]
    CALL_LLM: ClassVar[CapabilityName]
    PERSIST: ClassVar[CapabilityName]
    NETWORK_EGRESS: ClassVar[CapabilityName]
    SPAWN_AGENT: ClassVar[CapabilityName]
    HOLD_REF: ClassVar[CapabilityName]
    BORROW_REF: ClassVar[CapabilityName]
    PRODUCE_REF_STREAM: ClassVar[CapabilityName]


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


__all__ = ["Caps"]
