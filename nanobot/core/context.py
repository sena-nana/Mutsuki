"""AgentContext —— 传给插件命令的运行时上下文。

插件应该把 ``AgentContext`` 当作时间、ID、RNG、服务、scope、trace 元数据
的唯一入口。直接访问 ``time.*``、``uuid.*``、``random.*`` 是 hard rule
禁止的，后续的 lint 规则会标记此类违规。
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING

from nanobot.contracts.ids import AgentId, SpanId, TraceId

if TYPE_CHECKING:
    from nanobot.contracts.message import Message
    from nanobot.core.bus import Bus
    from nanobot.core.container import ServiceContainer
    from nanobot.core.scope import PluginScope
    from nanobot.runtime.clock import Clock
    from nanobot.runtime.idgen import IdGen
    from nanobot.runtime.rng import RNG


@dataclass(slots=True)
class TraceContext:
    trace_id: TraceId
    span_id: SpanId
    parent_span_id: SpanId | None = None


@dataclass(slots=True)
class AgentContext:
    """单次调用的上下文。插件命令签名里以 ``ctx`` 形式接收。"""

    agent_id: AgentId
    agent_owner: str | None
    clock: "Clock"
    id_gen: "IdGen"
    rng: "RNG"
    services: "ServiceContainer"
    scope: "PluginScope"
    bus: "Bus"
    trace_ctx: TraceContext
    message: "Message | None" = None
    extras: dict[str, object] = field(default_factory=dict)


__all__ = ["AgentContext", "TraceContext"]
