"""D9：Operation/Source 生命周期 4 场景全覆盖。

* Plugin 卸载 → PluginScope.close → dispatcher 反注册自动触发
* Plugin 重载 = 卸载 + 装载，op_id 在中途窗口不可用，重新装载后 active
* 配置变更 = 重载（v0.2 简化语义）
* Plugin 崩溃（handler 抛错）→ op 标记 unhealthy；plugin 实例本身不连坐
"""

from __future__ import annotations

from typing import ClassVar

import msgspec
import pytest

from mutsukicore import Capability, Caps, Perms, Plugin, command
from mutsukicore.contracts import Scopes
from mutsukicore.contracts.error import Errs
from mutsukicore.contracts.ids import AgentId
from mutsukicore.core.agent import Agent
from mutsukicore.core.dispatcher import OperationInvokeError, OperationStatus
from mutsukicore.core.loader import PluginLoader
from mutsukicore.runtime import DeterministicIdGen, SeededRng, SystemClock


class _Conf(msgspec.Struct, kw_only=True):
    pass


class _GreetPlugin(Plugin[_Conf]):
    id: ClassVar[str] = "test-greet-life"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [Capability(name=Caps.READ_MESSAGE)]
    Config = _Conf

    @command(perms=Perms.PUBLIC)
    async def greet(self) -> str:
        """问候。"""
        return "hi"


class _CrashConfig(msgspec.Struct, kw_only=True):
    pass


class _CrashPlugin(Plugin[_CrashConfig]):
    id: ClassVar[str] = "test-crash-life"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [Capability(name=Caps.READ_MESSAGE)]
    Config = _CrashConfig

    @command(perms=Perms.PUBLIC)
    async def boom(self) -> str:
        """always crashes."""
        raise RuntimeError("crash")


def _new_agent() -> Agent:
    return Agent(
        agent_id=AgentId("life-test"),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(seed=0),
        accepts=(Scopes.IM_TEXT.to_rule(),),
    )


@pytest.mark.asyncio
async def test_plugin_unload_removes_operations_from_dispatcher() -> None:
    """场景 1：Plugin 卸载 → dispatcher 反注册回调由 PluginScope.close 触发。"""
    agent = _new_agent()
    loader = PluginLoader(allow={_GreetPlugin.id})
    await loader.load_into(agent, [_GreetPlugin])
    # 装载后 op 存在
    assert agent.dispatch.has_operation("test-greet-life.greet")

    # 卸载
    await loader.unload_from(agent)
    # 反注册自动触发，op 已不在
    assert not agent.dispatch.has_operation("test-greet-life.greet")
    assert agent.dispatch.list_operations() == ()


@pytest.mark.asyncio
async def test_plugin_reload_restores_operation_active() -> None:
    """场景 2：Plugin reload = 卸载 + 装载，op 重新出现并可调用。"""
    agent = _new_agent()
    loader = PluginLoader(allow={_GreetPlugin.id})

    await loader.load_into(agent, [_GreetPlugin])
    ctx = agent.make_context()
    r1 = await agent.dispatch.invoke("test-greet-life.greet", {}, ctx=ctx)
    assert r1 == "hi"

    # 卸载（中途窗口）
    await loader.unload_from(agent)
    # 中途调用应是 not_found
    with pytest.raises(OperationInvokeError) as ei:
        await agent.dispatch.invoke("test-greet-life.greet", {}, ctx=ctx)
    assert ei.value.error.code == Errs.OPERATION_NOT_FOUND

    # 重新装载（模拟 reload）
    await loader.load_into(agent, [_GreetPlugin])
    r2 = await agent.dispatch.invoke("test-greet-life.greet", {}, ctx=ctx)
    assert r2 == "hi"

    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_config_change_via_reload_with_new_config() -> None:
    """场景 3：配置变更 = unload + load_into(configs=...)。

    v0.2 简化：配置变更等价于一次 reload，连接（Handle / Source）会被
    PluginScope.close 关闭并以新配置重建。
    """

    class _PrefConfig(msgspec.Struct, kw_only=True):
        prefix: str = "v1: "

    class _PrefPlugin(Plugin[_PrefConfig]):
        id: ClassVar[str] = "test-pref-life"
        version: ClassVar[str] = "0.0.1"
        capabilities: ClassVar[list[Capability]] = [Capability(name=Caps.READ_MESSAGE)]
        Config = _PrefConfig

        @command(perms=Perms.PUBLIC)
        async def show(self) -> str:
            """show prefix."""
            return self.config.prefix + "ok"

    agent = _new_agent()
    loader = PluginLoader(allow={_PrefPlugin.id})

    await loader.load_into(agent, [_PrefPlugin])
    ctx = agent.make_context()
    r1 = await agent.dispatch.invoke("test-pref-life.show", {}, ctx=ctx)
    assert r1 == "v1: ok"

    # 配置变更 = reload + 新 config
    await loader.unload_from(agent)
    await loader.load_into(
        agent,
        [_PrefPlugin],
        configs={_PrefPlugin.id: _PrefConfig(prefix="v2: ")},
    )
    r2 = await agent.dispatch.invoke("test-pref-life.show", {}, ctx=ctx)
    assert r2 == "v2: ok"

    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_plugin_handler_crash_marks_operation_unhealthy_not_plugin() -> None:
    """场景 4：handler 抛错 → op 标记 unhealthy；plugin 实例本身不连坐。"""
    agent = _new_agent()
    loader = PluginLoader(allow={_CrashPlugin.id})
    await loader.load_into(agent, [_CrashPlugin])

    ctx = agent.make_context()
    # 第一次调用：handler raised → unhealthy
    with pytest.raises(OperationInvokeError) as ei1:
        await agent.dispatch.invoke("test-crash-life.boom", {}, ctx=ctx)
    assert ei1.value.error.code == Errs.OPERATION_HANDLER_RAISED

    # plugin 实例仍在 agent.plugins 中（不连坐卸载）
    assert any(
        entry.plugin.id == _CrashPlugin.id for entry in agent.plugins
    )
    # op 状态：unhealthy
    assert (
        agent.dispatch.operation_status("test-crash-life.boom")
        == OperationStatus.UNHEALTHY
    )

    # 第二次调用直接以 unhealthy 拒绝
    with pytest.raises(OperationInvokeError) as ei2:
        await agent.dispatch.invoke("test-crash-life.boom", {}, ctx=ctx)
    assert ei2.value.error.code == Errs.OPERATION_UNHEALTHY

    # reload 后 op 恢复 active
    await loader.unload_from(agent)
    await loader.load_into(agent, [_CrashPlugin])
    assert (
        agent.dispatch.operation_status("test-crash-life.boom")
        == OperationStatus.ACTIVE
    )

    await loader.unload_from(agent)


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
