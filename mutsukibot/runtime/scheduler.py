"""Agent tick 调度器（v0.2 重构）。

v0.2 循环：

1. 从 ``agent.inbox`` 等待入站 :class:`Envelope`（含 :class:`Message` 子类）。
2. 把首词当命令名解析。
3. 通过 :meth:`Dispatcher.lookup_operation` 找到 op_id。
4. 通过 :meth:`Dispatcher.invoke` 执行 Operation —— 复用 dispatcher 内部的
   capability / permission / trace 拦截链（见 contracts §18.3）。
5. 把结果包成出站 :class:`Message`，**用 inbound message 的
   ``source.source_id`` 复写 ChannelRef**，而非硬编码 ``"agent"``，
   保留回写到正确 transport 的能力（修复 v0.1 缺陷）。
6. 命令执行的 Operation span 只由 dispatcher 产出；scheduler 只记录 unmatched
   与 envelope consumer 这类调度层事实。

Graceful shutdown：``stop()`` 把一个 sentinel 放入 inbox，让 ``_loop``
处理完手头消息后自然退出，而不是直接 ``cancel()`` 打断正在执行的命令。
仅在 ``shutdown_timeout`` 超时后才回退到强制取消，作为最后兜底。
"""

from __future__ import annotations

import asyncio
import shlex
from typing import TYPE_CHECKING, Final

from mutsukibot.contracts.envelope import Envelope
from mutsukibot.contracts.error import Error, Errs
from mutsukibot.contracts.event import SpanStatus, TraceSpan
from mutsukibot.contracts.ids import MessageId, SpanId, TraceId
from mutsukibot.contracts.lifecycle import LifecyclePhase
from mutsukibot.contracts.message import ChannelRef, ContentKind, ContentPart, Message
from mutsukibot.contracts.source_builtin import SourceKinds
from mutsukibot.core.container import ServiceNotFoundError
from mutsukibot.core.dispatcher import OperationInvokeError
from mutsukibot.core.scope import HandleLeakError
from mutsukibot.core.trace import trace_span

if TYPE_CHECKING:
    from mutsukibot.core.agent import Agent


class _StopSentinel:
    """放入 inbox 用来通知 ``_loop`` 优雅退出的哨兵。"""


_STOP: Final[_StopSentinel] = _StopSentinel()


class AgentScheduler:
    def __init__(
        self,
        agent: "Agent",
        *,
        shutdown_timeout: float = 5.0,
    ) -> None:
        self.agent = agent
        self.shutdown_timeout = shutdown_timeout
        self._task: asyncio.Task[None] | None = None

    async def start(self) -> None:
        ctx = self.agent.make_context()
        self.agent.phase = LifecyclePhase.AWAKE
        await self.agent.lifespan.fire("awake", ctx)
        self._task = asyncio.create_task(self._loop())

    async def stop(self) -> None:
        if self._task is not None:
            # 优雅停机：让 _loop 处理完手头消息后自然退出。
            await self.agent.inbox.put(_STOP)
            try:
                await asyncio.wait_for(self._task, timeout=self.shutdown_timeout)
            except TimeoutError:
                # 超时兜底：强制取消（接受被打断命令的副作用半完成风险）
                self._task.cancel()
                try:
                    await self._task
                except asyncio.CancelledError:
                    pass
            # 真实 loop 异常不静默：让上层看到 bug。
        # sleep / stop 各自新建 ctx，避免 trace 上下文混淆两个阶段
        sleep_ctx = self.agent.make_context()
        self.agent.phase = LifecyclePhase.SLEEP
        await self.agent.lifespan.fire("sleep", sleep_ctx)
        stop_ctx = self.agent.make_context()
        self.agent.phase = LifecyclePhase.STOP
        await self.agent.lifespan.fire("stop", stop_ctx)
        await self.agent.close_agent_scope()

    async def _loop(self) -> None:
        # 直接阻塞 await，不再每秒 10 次轮询。stop 通过 _STOP sentinel 唤醒。
        while True:
            item = await self.agent.inbox.get()
            if item is _STOP:
                return
            if isinstance(item, Envelope):
                # v0.2 双路径并行：
                # 1. envelope 二次分发 —— 按 plugin.consumes 派发到 plugin.on_envelope
                # 2. Message 子类还走命令路由（首词解析 → dispatch.invoke）
                await self._dispatch_to_plugins(item)
                if isinstance(item, Message):
                    await self._handle_message(item)

    async def _dispatch_to_plugins(self, envelope: Envelope) -> None:
        """按 plugin.consumes 把 envelope 派发到所有匹配的 plugin。

        每个 plugin 独立 trace span。on_envelope 抛错不连带其他 plugin —— 仅
        记录结构化 Error 到 trace span attributes，让 observability 可见。
        """
        for entry in self.agent.plugins:
            plugin = entry.plugin
            consumes: tuple = plugin.__class__.consumes
            if not consumes:
                continue
            if not any(rule.check(envelope) for rule in consumes):
                continue
            attributes: dict[str, str | int | float | bool] = {
                "agent_id": str(self.agent.agent_id),
                "envelope_id": str(envelope.id),
                "envelope_schema": envelope.payload_schema_id,
                "source_id": envelope.source.source_id,
            }
            ctx = self.agent.make_context()
            async with trace_span(
                ctx,
                f"plugin.{plugin.id}.on_envelope",
                attributes=attributes,
            ) as span:
                try:
                    await plugin.on_envelope(envelope)
                except Exception as exc:
                    span.status = SpanStatus.ERROR
                    span.attributes["exception_type"] = type(exc).__qualname__
                    span.attributes["exception_repr"] = repr(exc)

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

        op_id = self.agent.dispatch.lookup_operation(cmd_name)
        if op_id is None:
            # 找不到命令视为"普通消息"，不写 outbox（否则真实 IM transport
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

        # 拿 OperationDescriptor 用于 params_schema 解参 + trace span name
        spec = next(
            (op for op in self.agent.dispatch.list_operations() if op.op_id == op_id),
            None,
        )
        if spec is None:
            # 极小的 race window：lookup 后到 list 之间被 unregister。
            return

        # 从位置参数构造 payload（typed-arg → dict）
        param_names = list(spec.parameters_schema.get("properties", {}))
        payload: dict[str, object] = {}
        for name, value in zip(param_names, positional, strict=False):
            payload[name] = _coerce(value, spec.parameters_schema["properties"][name])

        # 构造 ctx —— scope 用 agent 自有 fallback scope，dispatcher.invoke
        # 内部会按 op 注册时绑定的 plugin scope 行使权限/容量检查，并产出唯一
        # Operation 执行 span。
        ctx = self.agent.make_context(message=msg)

        try:
            result = await self.agent.dispatch.invoke(op_id, payload, ctx=ctx)
            await self._emit_result(msg, str(result))
        except OperationInvokeError as exc:
            await self._emit_error(msg, exc.error)
        except Exception as exc:
            await self._emit_error(
                msg, _classify_command_exception(exc, spec.plugin_id, spec.name)
            )

    def _outbound_source(self, inbound: Message) -> ChannelRef:
        """复用入站 source 信息构造出站 ChannelRef，避免硬编码 ``"agent"``。

        复用 inbound message 的 source 信息，让回执能路由回正确 transport。
        """
        src = inbound.source
        if isinstance(src, ChannelRef):
            return ChannelRef(
                source_id=src.source_id,
                kind=src.kind,
                channel_id=src.channel_id,
                user_id=src.user_id,
            )
        # inbound 不是 ChannelRef（理论上不会发生，但兜底）：构造一个最小
        # IM ChannelRef 指向 agent 自身，至少 trace 不丢上下文。
        return ChannelRef(
            source_id=src.source_id,
            kind=SourceKinds.IM,
            channel_id=self.agent.agent_id,
        )

    async def _emit_result(self, msg: Message, text: str) -> None:
        out = Message(
            id=MessageId(self.agent.id_gen.next("msg")),
            timestamp=self.agent.clock.now(),
            source=self._outbound_source(msg),
            payload_schema_id="mutsukibot.message",
            parts=(ContentPart(kind=ContentKind.TEXT, text=text),),
        )
        await self.agent.outbox.put(out)

    async def _emit_error(self, msg: Message, err: Error) -> None:
        out = Message(
            id=MessageId(self.agent.id_gen.next("msg")),
            timestamp=self.agent.clock.now(),
            source=self._outbound_source(msg),
            payload_schema_id="mutsukibot.message",
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
