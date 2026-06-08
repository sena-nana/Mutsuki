"""Envelope 协议 —— 通用入站/出站载体。

详见 :doc:`contracts §16 <plans/contracts>`。Envelope 是 dispatcher 路由的
基本单位；IM ``Message`` 是其特化（参见 :mod:`mutsukibot.contracts.message`）。
其他外部后端或领域输入应由 bridge / 领域插件声明自己的 Envelope 子类与
``SourceKindName``。

路由按 ``payload_schema_id`` 与 ``source.source_id`` / ``source.kind`` /
``capabilities_required`` 决定（参见 :mod:`mutsukibot.contracts.scope`）。
"""

from __future__ import annotations

from typing import ClassVar

from mutsukibot.contracts.base import Contract
from mutsukibot.contracts.capability import CapabilityName
from mutsukibot.contracts.ids import EnvelopeId
from mutsukibot.contracts.source import SourceKindName


class SourceRef(Contract):
    """通用事件来源描述。IM ``ChannelRef`` 与领域 SourceRef 可收窄它。"""

    schema_id: ClassVar[str] = "mutsukibot.source_ref"
    schema_version: ClassVar[str] = "1.0.0"

    source_id: str
    kind: SourceKindName


class Envelope(Contract):
    """通用入站/出站载体基类。

    路由主键是 ``payload_schema_id`` —— 子类（``Message`` / 领域 envelope）
    通过自身 ``schema_id`` ClassVar 标识具体形态，但
    ``payload_schema_id`` 字段允许 envelope 携带的 *payload* 与 *envelope
    壳体* 各自独立声明 schema，以便复杂场景（如 IM 消息携带 latent ref
    payload）的路由表达。
    """

    schema_id: ClassVar[str] = "mutsukibot.envelope"
    schema_version: ClassVar[str] = "1.0.0"

    id: EnvelopeId
    timestamp: float
    source: SourceRef
    payload_schema_id: str = ""
    capabilities_required: tuple[CapabilityName, ...] = ()


__all__ = ["Envelope", "SourceRef"]
