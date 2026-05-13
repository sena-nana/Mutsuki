"""AgentScheduler 关键回归：异常透传 + 错误码分级。"""

from __future__ import annotations

import asyncio
from typing import ClassVar

import msgspec
import pytest

from nanobot import Capability, Caps, Perms, Plugin, command
from nanobot.adapters import InMemoryAdapter
from nanobot.contracts.error import Errs
from nanobot.contracts.ids import AgentId
from nanobot.core.agent import Agent
from nanobot.core.container import ServiceNotFoundError
from nanobot.core.loader import PluginLoader
from nanobot.runtime import DeterministicIdGen, SeededRng, SystemClock
from nanobot.runtime.scheduler import AgentScheduler, _classify_command_exception


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
    )


def test_classify_service_not_found_maps_to_definition_error() -> None:
    err = _classify_command_exception(ServiceNotFoundError("nope"), "p", "c")
    assert err.code == Errs.PLUGIN_DEFINITION_ERROR
    assert err.evidence["reason"] == "service_not_found"


def test_classify_key_error_maps_to_definition_error_with_arg_reason() -> None:
    err = _classify_command_exception(KeyError("x"), "p", "c")
    assert err.code == Errs.PLUGIN_DEFINITION_ERROR
    assert err.evidence["reason"] == "missing_arg"


def test_classify_generic_exception_preserves_type_info() -> None:
    err = _classify_command_exception(ValueError("oops"), "p", "c")
    assert err.code == Errs.PLUGIN_DEFINITION_ERROR
    assert err.evidence["reason"] == "command_raised"
    assert err.evidence["exception_type"] == "ValueError"
    assert "oops" in str(err.evidence["exception_repr"])


@pytest.mark.asyncio
async def test_handle_message_emits_classified_error_for_command_body_exception() -> None:
    agent = _new_agent()
    loader = PluginLoader(allow={_BoomPlugin.id})
    await loader.load_into(agent, [_BoomPlugin])
    scheduler = AgentScheduler(agent)
    await scheduler.start()

    adapter = InMemoryAdapter()
    await adapter.send_text(agent, "boom value")
    await asyncio.sleep(0.2)
    msgs = await adapter.drain_outbox(agent, timeout=0.5)
    assert msgs, "至少有一条出错消息"
    text = "".join(m.text for m in msgs)
    # 即使没拿到结构化 evidence，错误码也不应该被吞或换成无关码。
    assert Errs.PLUGIN_DEFINITION_ERROR in text
    assert "command_raised" in text
    assert "ValueError" in text

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
