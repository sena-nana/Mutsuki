"""Agent —— 一等运行时实体。

Agent 拥有自己的：

* 身份（``agent_id``）
* 生命周期阶段（:class:`LifecyclePhase`）
* tick 调度器（一个 ``asyncio.Task``）
* inbox / outbox 队列
* 已加载的插件实例（每个有自己的 :class:`PluginScope`）
* :class:`ServiceContainer` 与 :class:`Bus`
* trace 上下文

Agent 由 :class:`AgentRegistry` 创建，由 :class:`AgentScheduler` 驱动
（见 :mod:`nanobot.runtime.scheduler`）。
"""

from __future__ import annotations

import asyncio
from dataclasses import dataclass, field
from typing import TYPE_CHECKING

from nanobot.contracts.ids import AgentId, SpanId, TraceId
from nanobot.contracts.lifecycle import LifecyclePhase
from nanobot.contracts.message import Message
from nanobot.core.bus import Bus
from nanobot.core.container import ServiceContainer
from nanobot.core.context import AgentContext, TraceContext
from nanobot.core.lifespan import Lifespan
from nanobot.core.scope import PluginScope

if TYPE_CHECKING:
    from nanobot.core.plugin import Plugin, _CommandMarker
    from nanobot.runtime.clock import Clock
    from nanobot.runtime.idgen import IdGen
    from nanobot.runtime.rng import RNG


@dataclass(slots=True)
class _LoadedPlugin:
    plugin: "Plugin"
    scope: PluginScope


@dataclass(slots=True, frozen=True)
class CommandTarget:
    """``find_command`` 的命中结果 —— 调度器路由命令所需的全部句柄。"""

    plugin: "Plugin"
    attr_name: str
    scope: PluginScope
    marker: "_CommandMarker"


@dataclass
class Agent:
    """一等 Agent 实体。"""

    agent_id: AgentId
    clock: "Clock"
    id_gen: "IdGen"
    rng: "RNG"
    owner: str | None = None
    services: ServiceContainer = field(default_factory=ServiceContainer)
    bus: Bus = field(default_factory=Bus)
    lifespan: Lifespan = field(default_factory=Lifespan)
    # inbox 类型放宽到 object，让 scheduler 既能投 Message 也能投控制
    # sentinel（如 graceful shutdown 用的 _STOP）。outbox 保持 Message
    # 严格类型 —— adapter 是消费者，需要类型保护。
    inbox: asyncio.Queue[object] = field(default_factory=asyncio.Queue)
    outbox: asyncio.Queue[Message] = field(default_factory=asyncio.Queue)
    phase: LifecyclePhase = LifecyclePhase.AWAKE
    plugins: list[_LoadedPlugin] = field(default_factory=list)
    _agent_scope: PluginScope | None = field(default=None, repr=False)
    _command_index: dict[str, CommandTarget] = field(default_factory=dict, repr=False)

    def make_context(self, message: Message | None = None) -> AgentContext:
        trace_ctx = TraceContext(
            trace_id=TraceId(self.id_gen.next("trace")),
            span_id=SpanId(self.id_gen.next("span")),
        )
        # 默认 scope 始终是 agent 自有 scope，避免「碰巧排第一」的插件被卸载
        # 时把 agent 上下文一起带走。命令路由路径在 scheduler 里会显式替换为
        # 插件本体的 scope。
        if self._agent_scope is None:
            self._agent_scope = PluginScope(self.agent_id)
        scope = self._agent_scope
        return AgentContext(
            agent_id=self.agent_id,
            agent_owner=self.owner,
            clock=self.clock,
            id_gen=self.id_gen,
            rng=self.rng,
            services=self.services,
            scope=scope,
            bus=self.bus,
            trace_ctx=trace_ctx,
            message=message,
        )

    def attach_plugin(self, plugin: "Plugin", scope: PluginScope) -> None:
        self.plugins.append(_LoadedPlugin(plugin, scope))
        markers: dict[str, "_CommandMarker"] = plugin.__class__.__command_markers__
        # 用 spec.name（marker.explicit_name or func.__name__）登记，与外部
        # 触发时输入的命令名一致。原先同时按 attr_name 与 func.__name__ 两次
        # setdefault：99% 情况下两者相同，剩下 1% 也没人会用 attr_name 触发。
        for attr_name, marker in markers.items():
            spec = marker.spec
            cmd_name = spec.name if spec is not None else attr_name
            target = CommandTarget(
                plugin=plugin, attr_name=attr_name, scope=scope, marker=marker
            )
            self._command_index.setdefault(cmd_name, target)

    def detach_plugin(self, plugin: "Plugin") -> None:
        """从命令索引里剔除该插件 —— 由 loader 卸载时调用。"""
        keys_to_drop = [k for k, t in self._command_index.items() if t.plugin is plugin]
        for k in keys_to_drop:
            del self._command_index[k]

    async def close_agent_scope(self) -> None:
        """关闭 agent 自有 fallback scope；由调度器在 stop 阶段调用。"""
        if self._agent_scope is not None and not self._agent_scope.closed:
            await self._agent_scope.close()
        self._agent_scope = None

    def find_command(self, name: str) -> CommandTarget | None:
        return self._command_index.get(name)


__all__ = ["Agent", "CommandTarget"]
