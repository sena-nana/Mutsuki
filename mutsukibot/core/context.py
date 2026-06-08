"""AgentContext —— 传给插件命令的运行时上下文。

插件应该把 ``AgentContext`` 当作时间、ID、RNG、服务、scope、trace 元数据
的唯一入口。直接访问 ``time.*``、``uuid.*``、``random.*`` 是 hard rule
禁止的，后续的 lint 规则会标记此类违规。
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING

from mutsukibot.contracts.ids import AgentId, SpanId, TraceId

if TYPE_CHECKING:
    from mutsukibot.contracts.envelope import Envelope
    from mutsukibot.core.bus import Bus
    from mutsukibot.core.container import ServiceContainer
    from mutsukibot.core.dispatcher import Dispatcher
    from mutsukibot.core.scope import PluginScope
    from mutsukibot.runtime.clock import Clock
    from mutsukibot.runtime.idgen import IdGen
    from mutsukibot.runtime.rng import RNG


@dataclass(slots=True)
class TraceContext:
    trace_id: TraceId
    span_id: SpanId
    parent_span_id: SpanId | None = None


@dataclass(slots=True)
class AgentContext:
    """单次调用的上下文。插件命令签名里以 ``ctx`` 形式接收。

    v0.2 新增 ``dispatch`` 字段：插件通过 ``ctx.dispatch.invoke(op_id, ...)``
    调用其他 plugin 的 Operation（详 contracts §18）。
    """

    agent_id: AgentId
    agent_owner: str | None
    clock: "Clock"
    id_gen: "IdGen"
    rng: "RNG"
    services: "ServiceContainer"
    scope: "PluginScope"
    bus: "Bus"
    dispatch: "Dispatcher"
    trace_ctx: TraceContext
    # Compatibility field name: historically this held an IM Message. Core now
    # treats it as the triggering Envelope; IM extensions may pass Message here.
    message: "Envelope | None" = None
    extras: dict[str, object] = field(default_factory=dict)


__all__ = ["AgentContext", "TraceContext"]
