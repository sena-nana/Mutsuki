"""Agent tick 调度器。

v0.1 的最小循环：

1. 从 ``agent.inbox`` 等待入站 :class:`Message`。
2. 把首词当命令名解析。
3. 通过 :meth:`Agent.find_command` 找到所属插件。
4. 构造 :class:`AgentContext` 与子 :class:`PluginScope`。
5. 检查命令的 :class:`PermissionRule`。
6. 通过解析好的 :class:`Dependent` ``await`` 插件方法。
7. 把结果包成出站 :class:`Message` 投到 ``agent.outbox``。
8. 每次调用产出一个 :class:`TraceSpan`。
"""

from __future__ import annotations

import asyncio
import shlex
from typing import TYPE_CHECKING

from nanobot.contracts.error import Error, Errs
from nanobot.contracts.event import SpanStatus, TraceSpan
from nanobot.contracts.ids import MessageId, SpanId, TraceId
from nanobot.contracts.lifecycle import LifecyclePhase
from nanobot.contracts.message import ChannelRef, ContentKind, ContentPart, Message
from nanobot.core.capability_guard import (
    CapabilityNotDeclaredError,
    check_capabilities,
)
from nanobot.core.container import ServiceNotFoundError
from nanobot.core.context import AgentContext, TraceContext
from nanobot.core.dependency import Dependent
from nanobot.core.scope import HandleLeakError

if TYPE_CHECKING:
    from nanobot.core.agent import Agent


class AgentScheduler:
    def __init__(self, agent: "Agent") -> None:
        self.agent = agent
        self._task: asyncio.Task[None] | None = None
        self._stop_evt = asyncio.Event()

    async def start(self) -> None:
        ctx = self.agent.make_context()
        self.agent.phase = LifecyclePhase.AWAKE
        await self.agent.lifespan.fire("awake", ctx)
        self._task = asyncio.create_task(self._loop())

    async def stop(self) -> None:
        self._stop_evt.set()
        if self._task is not None:
            self._task.cancel()
            try:
                await self._task
            except asyncio.CancelledError:
                pass
            # 真实 loop 异常不静默：让上层看到 bug。
        ctx = self.agent.make_context()
        self.agent.phase = LifecyclePhase.SLEEP
        await self.agent.lifespan.fire("sleep", ctx)
        self.agent.phase = LifecyclePhase.STOP
        await self.agent.lifespan.fire("stop", ctx)
        await self.agent.close_agent_scope()

    async def _loop(self) -> None:
        while not self._stop_evt.is_set():
            try:
                msg = await asyncio.wait_for(self.agent.inbox.get(), timeout=0.1)
            except TimeoutError:
                continue
            await self._handle_message(msg)

    async def _handle_message(self, msg: Message) -> None:
        text = msg.text.strip()
        if not text:
            return
        try:
            tokens = shlex.split(text)
        except ValueError:
            tokens = text.split()
        if not tokens:
            return
        cmd_name = tokens[0]
        positional = tokens[1:]

        target = self.agent.find_command(cmd_name)
        if target is None:
            # 找不到命令视为"普通消息"，不写 outbox（否则真实 IM adapter
            # 会对群里每条非命令消息都吐错误回执）。仅打一条 ok 状态的
            # trace span，attribute 里标记 unmatched，便于审计与未来引入
            # 命令前缀过滤时回看。
            now = self.agent.clock.now()
            unmatched_span = TraceSpan(
                trace_id=TraceId(self.agent.id_gen.next("trace")),
                span_id=SpanId(self.agent.id_gen.next("span")),
                name="agent.scheduler.unmatched",
                start=now,
                end=now,
                status=SpanStatus.OK,
                attributes={
                    "agent_id": self.agent.agent_id,
                    "unmatched": True,
                    "first_token": cmd_name,
                },
            )
            await self.agent.bus.publish("trace.span", unmatched_span)
            return

        plugin = target.plugin
        marker = target.marker
        spec = marker.spec
        if spec is None:
            return
        scope = target.scope

        trace_ctx = TraceContext(
            trace_id=TraceId(self.agent.id_gen.next("trace")),
            span_id=SpanId(self.agent.id_gen.next("span")),
        )
        ctx = AgentContext(
            agent_id=self.agent.agent_id,
            agent_owner=self.agent.owner,
            clock=self.agent.clock,
            id_gen=self.agent.id_gen,
            rng=self.agent.rng,
            services=self.agent.services,
            scope=scope,
            bus=self.agent.bus,
            trace_ctx=trace_ctx,
            message=msg,
        )

        # capability 检查。
        try:
            declared = tuple(c.name for c in plugin.__class__.capabilities)
            check_capabilities(
                plugin_id=plugin.id,
                declared=declared,
                required=spec.requires_capabilities,
                route=f"command.{spec.name}",
            )
        except CapabilityNotDeclaredError as e:
            await self._emit_error(msg, e.error)
            return

        # permission 检查。
        if not await marker.perms.check(ctx):
            await self._emit_error(
                msg,
                Error(
                    code=Errs.PERMISSION_DENIED,
                    source=plugin.id,
                    route=f"command.{spec.name}",
                    evidence={"perms_rule": spec.perms_rule_id or ""},
                ),
            )
            return

        # 从位置参数构造 extras。
        param_names = list(spec.parameters_schema.get("properties", {}))
        extras: dict[str, object] = {}
        for name, value in zip(param_names, positional, strict=False):
            extras[name] = _coerce(value, spec.parameters_schema["properties"][name])

        span_start = self.agent.clock.now()
        status = SpanStatus.OK
        try:
            # PluginMeta 已在 class 定义阶段把 Dependent 解析好缓存到 marker，
            # 这里直接复用，避免 per-tick inspect 开销；旧入口保持回退能力。
            dependent: Dependent[object] = (
                marker.dependent
                if marker.dependent is not None
                else Dependent.parse(marker.func)
            )
            result = await dependent.solve(ctx, bound_self=plugin, **extras)
            await self._emit_result(msg, str(result))
        except Exception as exc:
            status = SpanStatus.ERROR
            await self._emit_error(
                msg, _classify_command_exception(exc, plugin.id, spec.name)
            )
        finally:
            span = TraceSpan(
                trace_id=trace_ctx.trace_id,
                span_id=trace_ctx.span_id,
                parent_span_id=trace_ctx.parent_span_id,
                name=f"plugin.{plugin.id}.{spec.name}",
                start=span_start,
                end=self.agent.clock.now(),
                status=status,
                attributes={"agent_id": self.agent.agent_id},
            )
            await self.agent.bus.publish("trace.span", span)

    async def _emit_result(self, msg: Message, text: str) -> None:
        out = Message(
            id=MessageId(self.agent.id_gen.next("msg")),
            timestamp=self.agent.clock.now(),
            source=ChannelRef(adapter_id="agent", channel_id=self.agent.agent_id),
            parts=(ContentPart(kind=ContentKind.TEXT, text=text),),
        )
        await self.agent.outbox.put(out)

    async def _emit_error(self, msg: Message, err: Error) -> None:
        out = Message(
            id=MessageId(self.agent.id_gen.next("msg")),
            timestamp=self.agent.clock.now(),
            source=ChannelRef(adapter_id="agent", channel_id=self.agent.agent_id),
            parts=(
                ContentPart(
                    kind=ContentKind.TEXT,
                    text=f"[error {err.code}] {err.evidence}",
                ),
            ),
        )
        await self.agent.outbox.put(out)


def _coerce(raw: str, schema: dict[str, object]) -> object:
    t = schema.get("type")
    if t == "integer":
        return int(raw)
    if t == "number":
        return float(raw)
    if t == "boolean":
        return raw.lower() in {"true", "1", "yes"}
    return raw


def _classify_command_exception(
    exc: BaseException, plugin_id: str, command_name: str
) -> Error:
    """把命令执行抛出的异常映射到分级错误码。

    分级原则（按"运维看到这个码会去查什么"对齐）：

    * ``HANDLE_LEAK`` —— 句柄泄漏（直接复用 scope 抛出的结构化 Error）
    * ``SERVICE_NOT_FOUND`` —— 装载配置或服务注册有问题
    * ``COMMAND_INVALID_ARGS`` —— 调用方参数错误（KeyError 通常是缺参）
    * ``COMMAND_EXECUTION_FAILED`` —— 插件命令体抛了业务异常

    ``PLUGIN_DEFINITION_ERROR`` 仅由 :class:`PluginMeta` 在类定义阶段使用，
    不再被 scheduler 路径产生。
    """
    route = f"command.{command_name}"
    if isinstance(exc, HandleLeakError):
        return Error(
            code=Errs.HANDLE_LEAK,
            source=plugin_id,
            route=route,
            evidence=dict(exc.error.evidence),
        )
    if isinstance(exc, ServiceNotFoundError):
        return Error(
            code=Errs.SERVICE_NOT_FOUND,
            source=plugin_id,
            route=route,
            evidence={
                "reason": "service_not_found",
                "detail": str(exc),
            },
        )
    if isinstance(exc, KeyError):
        return Error(
            code=Errs.COMMAND_INVALID_ARGS,
            source=plugin_id,
            route=route,
            evidence={
                "reason": "missing_arg",
                "detail": str(exc),
            },
        )
    return Error(
        code=Errs.COMMAND_EXECUTION_FAILED,
        source=plugin_id,
        route=route,
        evidence={
            "reason": "command_raised",
            "exception_type": type(exc).__qualname__,
            "exception_repr": repr(exc),
        },
    )


__all__ = ["AgentScheduler"]
