"""Agent —— 一等运行时实体。

Agent 拥有自己的：

* 身份（``agent_id``）
* 职责范围（``accepts: tuple[ScopeRule, ...]``，v0.2 引入；空 tuple = 拒绝
  所有 envelope，遵守 [AGENTS.md hard rule #13](../../AGENTS.md)）
* 生命周期阶段（:class:`LifecyclePhase`）
* tick 调度器（一个 ``asyncio.Task``）
* inbox / outbox 队列
* 已加载的插件实例（每个有自己的 :class:`PluginScope`）
* :class:`ServiceContainer` / :class:`Bus` / :class:`Dispatcher`
* trace 上下文

v0.2 起 ``_command_index`` / ``find_command`` / ``CommandTarget`` 已删除：
命令路由统一走 :class:`mutsukibot.core.dispatcher.Dispatcher`（详 D12 命令与
Operation 统一）。``attach_plugin`` 负责把 plugin 实例绑到 ``agent.plugins``，
并把 PluginMeta 派生出的 @command Operation 注册到 dispatcher。
"""

from __future__ import annotations

import asyncio
from dataclasses import dataclass, field
from typing import TYPE_CHECKING

from mutsukibot.contracts.agent_profile import AgentParticipation, AgentProfile
from mutsukibot.contracts.ids import AgentId, SpanId, TraceId
from mutsukibot.contracts.lifecycle import LifecyclePhase
from mutsukibot.contracts.message import Message
from mutsukibot.contracts.scope import ScopeRule
from mutsukibot.core.agent_registry import AgentRegistry
from mutsukibot.core.bus import Bus
from mutsukibot.core.container import ServiceContainer
from mutsukibot.core.context import AgentContext, TraceContext
from mutsukibot.core.dispatcher import Dispatcher
from mutsukibot.core.lifespan import Lifespan
from mutsukibot.core.scope import PluginScope

if TYPE_CHECKING:
    from mutsukibot.core.dependency import Dependent
    from mutsukibot.core.plugin import Plugin
    from mutsukibot.runtime.clock import Clock
    from mutsukibot.runtime.idgen import IdGen
    from mutsukibot.runtime.rng import RNG


@dataclass(slots=True)
class _LoadedPlugin:
    plugin: "Plugin"
    scope: PluginScope


def _make_command_handler(
    plugin: "Plugin",
    dependent: "Dependent[object]",
):
    """把一个 @command 装饰的方法包成 Operation handler。

    Operation handler 统一签名 ``(ctx, payload: dict) -> Awaitable[Any]``。
    这里把 ``payload`` 作为 extras 透传给 :meth:`Dependent.solve`，由 Dependent
    完成 typed-arg 绑定（与 v0.1 scheduler 路径一致）。
    """

    async def _handler(ctx: AgentContext, payload: dict[str, object]) -> object:
        return await dependent.solve(ctx, bound_self=plugin, **payload)

    return _handler


@dataclass
class Agent:
    """一等 Agent 实体。"""

    agent_id: AgentId
    clock: "Clock"
    id_gen: "IdGen"
    rng: "RNG"
    owner: str | None = None
    # v0.3：多 Agent 目标选择的稳定权重。数值越大优先级越高；
    # 平手由 AgentRegistry 按 agent_id 升序打破，保证确定性。
    priority: int = 0
    # v0.2 新增：Agent 显式声明可消费 envelope 的 ScopeRule 集合。
    # 空 tuple = 拒绝所有 envelope（命令路径仍可用 —— 命令是被显式调用，
    # 不走 envelope 路由）。详见 hard rule #13 与 contracts §17。
    accepts: tuple[ScopeRule, ...] = ()
    # v0.5 前置切片：AgentProfile 是 Agent 的角色/策略配置层。为空时保持
    # 既有 Agent 行为，等价于 primary_candidate + accepts 字段。
    profile: AgentProfile | None = None
    services: ServiceContainer = field(default_factory=ServiceContainer)
    bus: Bus = field(default_factory=Bus)
    lifespan: Lifespan = field(default_factory=Lifespan)
    # inbox 类型放宽到 object，让 scheduler 既能投 Message / Envelope 也能
    # 投控制 sentinel（如 graceful shutdown 用的 _STOP）。outbox 保持
    # Message 严格类型 —— transport plugin 是消费者，需要类型保护。
    inbox: asyncio.Queue[object] = field(default_factory=asyncio.Queue)
    outbox: asyncio.Queue[Message] = field(default_factory=asyncio.Queue)
    phase: LifecyclePhase = LifecyclePhase.AWAKE
    plugins: list[_LoadedPlugin] = field(default_factory=list)
    _agent_scope: PluginScope | None = field(default=None, repr=False)
    _dispatch: Dispatcher | None = field(default=None, repr=False)

    def __post_init__(self) -> None:
        AgentRegistry.register(self)

    @property
    def participation(self) -> AgentParticipation:
        """该 Agent 的外部输入参与方式。"""
        if self.profile is None:
            return AgentParticipation.PRIMARY_CANDIDATE
        return self.profile.participation

    @property
    def effective_accepts(self) -> tuple[ScopeRule, ...]:
        """实际用于 envelope 路由匹配的 ScopeRule。"""
        if self.profile is None:
            return self.accepts
        return self.profile.accepts

    @property
    def dispatch(self) -> Dispatcher:
        """该 Agent 的 Dispatcher 实例（懒初始化）。"""
        if self._dispatch is None:
            self._dispatch = Dispatcher(self)
        return self._dispatch

    def make_context(self, message: Message | None = None) -> AgentContext:
        trace_ctx = TraceContext(
            trace_id=TraceId(self.id_gen.next("trace")),
            span_id=SpanId(self.id_gen.next("span")),
        )
        # 默认 scope 始终是 agent 自有 scope，避免「碰巧排第一」的插件被卸载
        # 时把 agent 上下文一起带走。Operation 调用路径在 dispatcher 里会
        # 显式替换为 op 注册时绑定的 plugin scope。
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
            dispatch=self.dispatch,
            trace_ctx=trace_ctx,
            message=message,
        )

    def attach_plugin(self, plugin: "Plugin", scope: PluginScope) -> None:
        """把 plugin 实例登记到 agent.plugins，并把 @command 注册为 Operation。

        v0.2 改动：原来的 ``_command_index`` 直接索引已删除；改为遍历 plugin
        类的 ``__command_markers__``，把每个 @command 标记包装成一个
        Operation handler 并通过 ``self.dispatch.register_operation`` 注册。
        反注册回调挂到 plugin 的 PluginScope 上，plugin 卸载时自动清理。
        """
        self.plugins.append(_LoadedPlugin(plugin, scope))

        # 把 @command 标记的方法注册为 Operation。延迟 import 避免循环。
        from mutsukibot.core.dependency import Dependent
        from mutsukibot.core.plugin import _CommandMarker

        markers: dict[str, _CommandMarker] = plugin.__class__.__command_markers__
        for _attr_name, marker in markers.items():
            spec = marker.spec
            if spec is None:
                continue
            # marker.dependent 已在 PluginMeta 装载阶段缓存
            dependent: Dependent[object] = (
                marker.dependent
                if marker.dependent is not None
                else Dependent.parse(marker.func)
            )
            handler = _make_command_handler(plugin, dependent)
            self.dispatch.register_operation(
                spec,
                handler=handler,
                perms=marker.perms,
                plugin_scope=scope,
            )

    async def close_agent_scope(self) -> None:
        """关闭 agent 自有 fallback scope；由调度器在 stop 阶段调用。"""
        if self._agent_scope is not None and not self._agent_scope.closed:
            await self._agent_scope.close()
        self._agent_scope = None


__all__ = ["Agent"]
