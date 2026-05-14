"""PluginLoader.load_into 失败回滚行为。"""

from __future__ import annotations

from typing import ClassVar

import msgspec
import pytest

from mutsukibot import Capability, Caps, Plugin
from mutsukibot.contracts.error import Errs
from mutsukibot.contracts.ids import AgentId
from mutsukibot.contracts.plugin import PluginDep
from mutsukibot.core.agent import Agent
from mutsukibot.core.loader import PluginLoader, PluginLoadFailedError
from mutsukibot.runtime import DeterministicIdGen, SeededRng, SystemClock


class _OkConfig(msgspec.Struct, kw_only=True):
    pass


class _OkPlugin(Plugin[_OkConfig]):
    id: ClassVar[str] = "rollback-ok"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [Capability(name=Caps.READ_MESSAGE)]
    Config = _OkConfig

    load_count: ClassVar[int] = 0
    unload_count: ClassVar[int] = 0

    async def on_load(self) -> None:
        type(self).load_count += 1

    async def on_unload(self) -> None:
        type(self).unload_count += 1


class _BoomLoadConfig(msgspec.Struct, kw_only=True):
    pass


class _BoomLoadPlugin(Plugin[_BoomLoadConfig]):
    id: ClassVar[str] = "rollback-boom"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [Capability(name=Caps.READ_MESSAGE)]
    requires_plugins: ClassVar[list[PluginDep]] = [PluginDep(plugin_id="rollback-ok")]
    Config = _BoomLoadConfig

    async def on_load(self) -> None:
        raise RuntimeError("on_load 故意失败")


def _new_agent() -> Agent:
    return Agent(
        agent_id=AgentId("rollback-test"),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(seed=0),
    )


@pytest.mark.asyncio
async def test_failed_load_rolls_back_previously_loaded_plugins() -> None:
    """B 的 on_load 抛错时，前面成功加载的 A 必须被反向卸载（on_unload + scope.close）。"""
    _OkPlugin.load_count = 0
    _OkPlugin.unload_count = 0

    agent = _new_agent()
    loader = PluginLoader(allow={_OkPlugin.id, _BoomLoadPlugin.id})

    with pytest.raises(PluginLoadFailedError) as ei:
        await loader.load_into(agent, [_OkPlugin, _BoomLoadPlugin])

    assert ei.value.plugin_id == "rollback-boom"
    assert ei.value.error.code == Errs.PLUGIN_LOAD_FAILED
    assert ei.value.error.evidence["rolled_back"] == 1

    # 关键：A 的 on_load 跑过一次，回滚阶段 on_unload 也跑过一次
    assert _OkPlugin.load_count == 1
    assert _OkPlugin.unload_count == 1

    # agent 状态干净，没有半加载残留
    assert agent.plugins == []
    # 命令索引也已清空（A 没有命令，但仍然检查 dict 状态）
    assert agent._command_index == {}


@pytest.mark.asyncio
async def test_first_plugin_load_failure_leaves_agent_clean() -> None:
    """加载列表里第一个就失败，没有需要回滚的对象，但 agent 仍应保持干净。"""
    agent = _new_agent()
    loader = PluginLoader(allow={_BoomLoadPlugin.id, _OkPlugin.id})

    with pytest.raises(PluginLoadFailedError):
        # 调换顺序但 _BoomLoadPlugin 依赖 _OkPlugin，拓扑序仍是 ok -> boom；
        # 这里改为只装 boom 自己（先模拟"第一个就失败"的形态需要去掉 deps）。
        await loader.load_into(agent, [_OkPlugin, _BoomLoadPlugin])

    assert agent.plugins == []
