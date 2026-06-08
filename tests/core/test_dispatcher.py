"""Dispatcher —— Operation/Source 注册、invoke 路径、unhealthy 状态机
（contracts §18 / D9）。
"""

from __future__ import annotations

from typing import ClassVar

import msgspec
import pytest

from mutsukibot import Capability, Caps, Perms, Plugin, command
from mutsukibot.contracts import (
    Scopes,
    SourceDescriptor,
    SourceKinds,
)
from mutsukibot.contracts.error import Errs
from mutsukibot.contracts.ids import AgentId
from mutsukibot.contracts.operation import OperationDescriptor
from mutsukibot.core.agent import Agent
from mutsukibot.core.dispatcher import (
    OperationConflictError,
    OperationInvokeError,
    OperationStatus,
    SourceConflictError,
)
from mutsukibot.core.loader import PluginLoader
from mutsukibot.core.scope import PluginScope
from mutsukibot.runtime import DeterministicIdGen, SeededRng, SystemClock
from mutsukibot_ext.im import ChannelRef, ContentKind, ContentPart, Message


def _new_agent() -> Agent:
    return Agent(
        agent_id=AgentId("disp-test"),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(seed=0),
        accepts=(Scopes.IM_TEXT.to_rule(),),
    )


# ---------------------------------------------------------------------------
# 通过 @command 路径间接验证 dispatcher 注册
# ---------------------------------------------------------------------------


class _Conf(msgspec.Struct, kw_only=True):
    pass


class _GreetPlugin(Plugin[_Conf]):
    id: ClassVar[str] = "test-greet"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [
        Capability(name=Caps.READ_MESSAGE),
        Capability(name=Caps.SEND_MESSAGE),
    ]
    Config = _Conf

    @command(perms=Perms.PUBLIC)
    async def greet(self, who: str = "world") -> str:
        """问候。"""
        return f"hi {who}"


class _CrashPlugin(Plugin[_Conf]):
    id: ClassVar[str] = "test-crash"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [
        Capability(name=Caps.READ_MESSAGE),
    ]
    Config = _Conf

    @command(perms=Perms.PUBLIC)
    async def crash(self) -> str:
        """总是抛错，用于验证 unhealthy 状态机。"""
        raise RuntimeError("boom")


@pytest.mark.asyncio
async def test_command_registers_operation_in_dispatcher() -> None:
    agent = _new_agent()
    loader = PluginLoader(allow={_GreetPlugin.id})
    await loader.load_into(agent, [_GreetPlugin])

    ops = agent.dispatch.list_operations()
    op_ids = {op.op_id for op in ops}
    assert "test-greet.greet" in op_ids

    # 短名查找：echo 风格的 lookup_operation
    assert agent.dispatch.lookup_operation("greet") == "test-greet.greet"
    assert agent.dispatch.lookup_operation("nonexistent") is None
    # 精确 op_id 查找
    assert agent.dispatch.lookup_operation("test-greet.greet") == "test-greet.greet"

    await loader.unload_from(agent)
    # 卸载后 dispatcher 中无残留
    assert agent.dispatch.list_operations() == ()


@pytest.mark.asyncio
async def test_invoke_returns_handler_result() -> None:
    agent = _new_agent()
    loader = PluginLoader(allow={_GreetPlugin.id})
    await loader.load_into(agent, [_GreetPlugin])

    ctx = agent.make_context()
    result = await agent.dispatch.invoke("test-greet.greet", {"who": "alice"}, ctx=ctx)
    assert result == "hi alice"

    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_invoke_unknown_op_raises_not_found() -> None:
    agent = _new_agent()
    ctx = agent.make_context()
    with pytest.raises(OperationInvokeError) as ei:
        await agent.dispatch.invoke("ghost.op", {}, ctx=ctx)
    assert ei.value.error.code == Errs.OPERATION_NOT_FOUND


@pytest.mark.asyncio
async def test_handler_exception_marks_operation_unhealthy() -> None:
    agent = _new_agent()
    loader = PluginLoader(allow={_CrashPlugin.id})
    await loader.load_into(agent, [_CrashPlugin])

    ctx = agent.make_context()

    # 第一次：handler 抛错 → OperationInvokeError(code=OPERATION_HANDLER_RAISED)
    with pytest.raises(OperationInvokeError) as ei1:
        await agent.dispatch.invoke("test-crash.crash", {}, ctx=ctx)
    assert ei1.value.error.code == Errs.OPERATION_HANDLER_RAISED

    # op 已被标记 unhealthy
    assert agent.dispatch.operation_status("test-crash.crash") == OperationStatus.UNHEALTHY

    # 第二次调用直接以 unhealthy 拒绝（不再触发 handler）
    with pytest.raises(OperationInvokeError) as ei2:
        await agent.dispatch.invoke("test-crash.crash", {}, ctx=ctx)
    assert ei2.value.error.code == Errs.OPERATION_UNHEALTHY

    await loader.unload_from(agent)


# ---------------------------------------------------------------------------
# Source 注册 / publish / source.unregistered
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_register_source_visible_in_list_and_status() -> None:
    agent = _new_agent()
    scope = PluginScope("test-src-plugin")
    desc = SourceDescriptor(
        source_id="test:src1",
        kind=SourceKinds.IM,
        capabilities=(Caps.IM_TEXT,),
    )
    agent.dispatch.register_source(desc, plugin_scope=scope, plugin_id="test-src-plugin")

    assert agent.dispatch.has_source("test:src1")
    assert agent.dispatch.source_status("test:src1") is not None
    sources = {s.source_id for s in agent.dispatch.list_sources()}
    assert "test:src1" in sources

    # scope 关闭触发反注册
    await scope.close()
    assert not agent.dispatch.has_source("test:src1")


@pytest.mark.asyncio
async def test_publish_unknown_source_raises_unregistered() -> None:
    """publish 时 envelope.source.source_id 不在已注册集 → source.unregistered。"""
    from mutsukibot.contracts import MessageId

    agent = _new_agent()
    msg = Message(
        id=MessageId("m"),
        timestamp=0.0,
        source=ChannelRef(
            source_id="ghost:never_registered",
            kind=SourceKinds.IM,
            channel_id="c",
        ),
        payload_schema_id="mutsukibot.message",
        capabilities_required=(Caps.IM_TEXT,),
        parts=(ContentPart(kind=ContentKind.TEXT, text="hi"),),
    )
    with pytest.raises(OperationInvokeError) as ei:
        await agent.dispatch.publish(msg)
    assert ei.value.error.code == Errs.SOURCE_UNREGISTERED


# ---------------------------------------------------------------------------
# 冲突检测
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_duplicate_operation_id_raises_conflict() -> None:
    agent = _new_agent()
    scope1 = PluginScope("p1")
    scope2 = PluginScope("p2")

    async def _h(_ctx, _payload):
        return "ok"

    desc = OperationDescriptor(op_id="test:dup.op", name="op", plugin_id="p1")
    agent.dispatch.register_operation(
        desc, handler=_h, perms=Perms.PUBLIC.to_rule(), plugin_scope=scope1
    )
    with pytest.raises(OperationConflictError) as ei:
        agent.dispatch.register_operation(
            OperationDescriptor(op_id="test:dup.op", name="op", plugin_id="p2"),
            handler=_h,
            perms=Perms.PUBLIC.to_rule(),
            plugin_scope=scope2,
        )
    assert ei.value.op_id == "test:dup.op"
    assert ei.value.error.code == Errs.OPERATION_CONFLICT

    await scope1.close()
    await scope2.close()


@pytest.mark.asyncio
async def test_duplicate_source_id_raises_conflict() -> None:
    agent = _new_agent()
    scope1 = PluginScope("p1")
    scope2 = PluginScope("p2")
    desc = SourceDescriptor(source_id="test:dup_src", kind=SourceKinds.IM)
    agent.dispatch.register_source(desc, plugin_scope=scope1, plugin_id="p1")
    with pytest.raises(SourceConflictError) as ei:
        agent.dispatch.register_source(desc, plugin_scope=scope2, plugin_id="p2")
    assert ei.value.source_id == "test:dup_src"
    assert ei.value.error.code == Errs.SOURCE_CONFLICT

    await scope1.close()
    await scope2.close()


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
