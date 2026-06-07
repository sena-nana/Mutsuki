"""AgentScheduler 关键回归：异常透传 + 错误码分级。"""

from __future__ import annotations

import asyncio
from typing import ClassVar

import msgspec
import pytest

from mutsukibot import Capability, Caps, Perms, Plugin, command
from mutsukibot.contracts import Scopes
from mutsukibot.contracts.error import Errs
from mutsukibot.contracts.event import SpanStatus, TraceSpan
from mutsukibot.contracts.ids import AgentId
from mutsukibot.core.agent import Agent
from mutsukibot.core.container import ServiceNotFoundError
from mutsukibot.core.loader import PluginLoader
from mutsukibot.plugins.inmemory_endpoint import InMemoryEndpointPlugin
from mutsukibot.runtime import DeterministicIdGen, SeededRng, SystemClock
from mutsukibot.runtime.scheduler import AgentScheduler, _classify_command_exception


class _BoomConfig(msgspec.Struct, kw_only=True):
    pass


class _BoomPlugin(Plugin[_BoomConfig]):
    """命令体直接抛错，用于验证错误码不被一刀切。"""

    id: ClassVar[str] = "test-boom"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [
        Capability(name=Caps.READ_MESSAGE),
        Capability(name=Caps.SEND_MESSAGE),
    ]
    Config = _BoomConfig

    @command(perms=Perms.PUBLIC)
    async def boom(self, mode: str = "value") -> str:
        """根据 mode 抛不同异常以测试 _classify_command_exception。"""
        if mode == "service":
            raise ServiceNotFoundError("svc-X 未注册")
        if mode == "key":
            raise KeyError("missing-arg")
        raise ValueError("命令体崩了")


def _new_agent() -> Agent:
    return Agent(
        agent_id=AgentId("sched-test"),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(seed=0),
        accepts=(Scopes.IM_TEXT.to_rule(),),
    )


def _get_inmem(agent: Agent) -> InMemoryEndpointPlugin:
    """从已装载 plugin 中拿出 InMemoryEndpointPlugin 实例（v0.2 测试驱动）。"""
    for entry in agent.plugins:
        if isinstance(entry.plugin, InMemoryEndpointPlugin):
            return entry.plugin
    raise RuntimeError("InMemoryEndpointPlugin 未装载")


def test_classify_service_not_found_maps_to_service_not_found() -> None:
    err = _classify_command_exception(ServiceNotFoundError("nope"), "p", "c")
    assert err.code == Errs.SERVICE_NOT_FOUND
    assert err.evidence["reason"] == "service_not_found"


def test_classify_key_error_maps_to_invalid_args() -> None:
    err = _classify_command_exception(KeyError("x"), "p", "c")
    assert err.code == Errs.COMMAND_INVALID_ARGS
    assert err.evidence["reason"] == "missing_arg"


def test_classify_generic_exception_maps_to_execution_failed() -> None:
    err = _classify_command_exception(ValueError("oops"), "p", "c")
    assert err.code == Errs.COMMAND_EXECUTION_FAILED
    assert err.evidence["reason"] == "command_raised"
    assert err.evidence["exception_type"] == "ValueError"
    assert "oops" in str(err.evidence["exception_repr"])


def test_classify_never_returns_plugin_definition_error() -> None:
    """PLUGIN_DEFINITION_ERROR 仅由 PluginMeta 在类定义阶段使用，
    scheduler 路径不应再产生它（避免运维误以为是定义层 bug）。"""
    for exc in (ServiceNotFoundError("x"), KeyError("y"), ValueError("z"), RuntimeError("w")):
        err = _classify_command_exception(exc, "p", "c")
        assert err.code != Errs.PLUGIN_DEFINITION_ERROR


@pytest.mark.asyncio
async def test_handle_message_emits_classified_error_for_command_body_exception() -> None:
    agent = _new_agent()
    loader = PluginLoader(allow={_BoomPlugin.id, InMemoryEndpointPlugin.id})
    await loader.load_into(agent, [InMemoryEndpointPlugin, _BoomPlugin])
    scheduler = AgentScheduler(agent)
    spans: list[TraceSpan] = []

    async def _on_span(payload: object) -> None:
        if isinstance(payload, TraceSpan):
            spans.append(payload)

    agent.bus.subscribe("trace.span", _on_span)
    await scheduler.start()

    inmem = _get_inmem(agent)
    await inmem.send_text("boom value")
    await asyncio.sleep(0.2)
    msgs = await inmem.drain_outbox(timeout=0.5)
    assert msgs, "至少有一条出错消息"
    text = "".join(m.text for m in msgs)
    # 命令体 ValueError 应当映射到 OPERATION_HANDLER_RAISED（v0.2 dispatcher
    # 路径）。dispatcher 把 handler 异常包成结构化 Error，scheduler 直接转写。
    assert Errs.OPERATION_HANDLER_RAISED in text
    assert "ValueError" in text
    invoke_spans = [s for s in spans if s.name == "dispatch.invoke"]
    scheduler_command_spans = [s for s in spans if s.name == "plugin.test-boom.boom"]
    assert len(invoke_spans) == 1
    assert invoke_spans[0].status == SpanStatus.ERROR
    assert scheduler_command_spans == []

    await scheduler.stop()
    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_unmatched_command_is_silent_and_emits_trace_span() -> None:
    """普通消息（首词不是任何已注册命令）不应进 outbox，仅写一条 unmatched span。"""
    agent = _new_agent()
    loader = PluginLoader(allow={_BoomPlugin.id, InMemoryEndpointPlugin.id})
    await loader.load_into(agent, [InMemoryEndpointPlugin, _BoomPlugin])
    scheduler = AgentScheduler(agent)

    spans: list[TraceSpan] = []

    async def _on_span(payload: object) -> None:
        if isinstance(payload, TraceSpan):
            spans.append(payload)

    agent.bus.subscribe("trace.span", _on_span)

    await scheduler.start()

    inmem = _get_inmem(agent)
    await inmem.send_text("你好世界")
    await asyncio.sleep(0.2)

    # 关键：outbox 不应有任何"命令不存在"错误回执
    msgs = await inmem.drain_outbox(timeout=0.3)
    assert msgs == []

    # 但应有一条 unmatched trace span
    await asyncio.sleep(0.05)
    unmatched = [s for s in spans if s.name == "agent.scheduler.unmatched"]
    assert len(unmatched) == 1
    span = unmatched[0]
    assert span.attributes["unmatched"] is True
    assert span.attributes["first_token"] == "你好世界"

    await scheduler.stop()
    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_stop_propagates_loop_exceptions() -> None:
    """v0.1 P0 修复后，_loop 中抛出的非 CancelledError 必须可见。"""
    agent = _new_agent()
    scheduler = AgentScheduler(agent)
    await scheduler.start()

    # 在 loop 中注入故障：替换 inbox.get 让其抛出。
    boom = RuntimeError("loop crashed")

    async def failing_get() -> object:
        raise boom

    agent.inbox.get = failing_get  # type: ignore[method-assign]

    # 给一点时间让 _loop 撞到错误并把 task 转入 done(exception=...)
    for _ in range(50):
        if scheduler._task is not None and scheduler._task.done():
            break
        await asyncio.sleep(0.01)

    # stop 不应再吞掉真实的 loop 异常。
    with pytest.raises(RuntimeError, match="loop crashed"):
        await scheduler.stop()


class _SlowConfig(msgspec.Struct, kw_only=True):
    pass


class _SlowPlugin(Plugin[_SlowConfig]):
    """命令体故意 sleep，用来验证 graceful shutdown 不会打断它。"""

    id: ClassVar[str] = "test-slow"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [
        Capability(name=Caps.READ_MESSAGE),
        Capability(name=Caps.SEND_MESSAGE),
    ]
    Config = _SlowConfig

    finished: ClassVar[bool]

    @command(perms=Perms.PUBLIC)
    async def slow(self) -> str:
        """模拟一个需要时间完成的命令。"""
        await asyncio.sleep(0.2)
        _SlowPlugin.finished = True
        return "done"


@pytest.mark.asyncio
async def test_graceful_shutdown_lets_in_flight_command_complete() -> None:
    """stop() 在调用时若有正在执行的命令，应等它跑完再退出，而不是 cancel 打断。"""
    _SlowPlugin.finished = False
    agent = _new_agent()
    loader = PluginLoader(allow={_SlowPlugin.id, InMemoryEndpointPlugin.id})
    await loader.load_into(agent, [InMemoryEndpointPlugin, _SlowPlugin])
    scheduler = AgentScheduler(agent, shutdown_timeout=2.0)
    await scheduler.start()

    inmem = _get_inmem(agent)
    await inmem.send_text("slow")
    # 在命令开始执行后立即 stop（命令还在 await asyncio.sleep）
    await asyncio.sleep(0.05)
    await scheduler.stop()

    # 关键：命令体应跑完，而不是被 cancel 打断
    assert _SlowPlugin.finished is True
    msgs = await inmem.drain_outbox(timeout=0.3)
    assert any("done" in m.text for m in msgs)
    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_shutdown_timeout_falls_back_to_cancel() -> None:
    """shutdown_timeout 超时时回退到 cancel（最后兜底，不让 stop 永远挂住）。"""
    agent = _new_agent()
    loader = PluginLoader(allow={InMemoryEndpointPlugin.id})
    await loader.load_into(agent, [InMemoryEndpointPlugin])
    scheduler = AgentScheduler(agent, shutdown_timeout=0.1)
    await scheduler.start()

    # 直接 monkey-patch _handle_message 让它永远不返回
    async def never_returns(_msg: object) -> None:
        await asyncio.sleep(60)

    scheduler._handle_message = never_returns  # type: ignore[method-assign]

    inmem = _get_inmem(agent)
    await inmem.send_text("anything")
    await asyncio.sleep(0.05)

    # stop 不应永远挂住；shutdown_timeout 后应回退到 cancel
    await scheduler.stop()
    assert scheduler._task is not None
    assert scheduler._task.done()
    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_command_success_emits_only_dispatch_invoke_operation_span() -> None:
    """命令执行事实只由 dispatcher span 表达，scheduler 不再重复造 command span。"""
    _SlowPlugin.finished = False
    agent = _new_agent()
    loader = PluginLoader(allow={_SlowPlugin.id, InMemoryEndpointPlugin.id})
    await loader.load_into(agent, [InMemoryEndpointPlugin, _SlowPlugin])
    scheduler = AgentScheduler(agent)
    spans: list[TraceSpan] = []

    async def _on_span(payload: object) -> None:
        if isinstance(payload, TraceSpan):
            spans.append(payload)

    agent.bus.subscribe("trace.span", _on_span)
    await scheduler.start()

    inmem = _get_inmem(agent)
    await inmem.send_text("slow")
    await asyncio.sleep(0.3)
    msgs = await inmem.drain_outbox(timeout=0.3)

    assert any("done" in m.text for m in msgs)
    invoke_spans = [s for s in spans if s.name == "dispatch.invoke"]
    scheduler_command_spans = [s for s in spans if s.name == "plugin.test-slow.slow"]
    assert len(invoke_spans) == 1
    assert invoke_spans[0].status == SpanStatus.OK
    assert scheduler_command_spans == []

    await scheduler.stop()
    await loader.unload_from(agent)
