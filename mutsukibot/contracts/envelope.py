"""Envelope 协议 —— 通用入站/出站载体。

详见 :doc:`contracts §16 <plans/contracts>`。Envelope 是 dispatcher 路由的
基本单位；IM ``Message`` 是其特化（参见 :mod:`mutsukibot.contracts.message`），
MCP 风格 ``ToolEvent`` 是另一种特化。

路由按 ``payload_schema_id`` 与 ``source.source_id`` / ``source.kind`` /
``capabilities_required`` 决定（参见 :mod:`mutsukibot.contracts.scope`）。
"""

from __future__ import annotations

from typing import Any, ClassVar

from mutsukibot.contracts.base import Contract
from mutsukibot.contracts.capability import CapabilityName
from mutsukibot.contracts.ids import EnvelopeId
from mutsukibot.contracts.source import SourceKindName


class SourceRef(Contract):
    """通用事件来源描述。IM ``ChannelRef`` 与 Tool ``ToolSourceRef`` 是其特化。"""

    schema_id: ClassVar[str] = "mutsukibot.source_ref"
    schema_version: ClassVar[str] = "1.0.0"

    source_id: str
    kind: SourceKindName


class Envelope(Contract):
    """通用入站/出站载体基类。

    路由主键是 ``payload_schema_id`` —— 子类（``Message`` / ``ToolEvent`` /
    领域 envelope）通过自身 ``schema_id`` ClassVar 标识具体形态，但
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


class ToolSourceRef(SourceRef):
    """工具型 Source 特化（MCP 风格）。"""

    schema_id: ClassVar[str] = "mutsukibot.tool_source_ref"
    schema_version: ClassVar[str] = "1.0.0"

    endpoint_path: str | None = None


class ToolEvent(Envelope):
    """MCP 风格事件推送 envelope。

    用于工具型 Source 主动推送外部状态变更（如 ``todo.created`` /
    ``fs.changed``）。``payload`` 是领域 dict；复杂场景用 ``RefPayload[T]``
    字段（参见 :doc:`contracts §11 <plans/contracts>`）。
    """

    schema_id: ClassVar[str] = "mutsukibot.tool_event"
    schema_version: ClassVar[str] = "1.0.0"

    event_type: str = ""
    payload: dict[str, Any] = {}


__all__ = ["Envelope", "SourceRef", "ToolEvent", "ToolSourceRef"]
