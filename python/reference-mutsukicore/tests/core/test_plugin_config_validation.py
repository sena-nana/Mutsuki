"""PluginLoader 配置 schema 自动校验。"""

from __future__ import annotations

from collections.abc import Mapping
from typing import ClassVar, cast

import msgspec
import pytest

from mutsukicore import Capability, Caps, Perms, Plugin, command
from mutsukicore.contracts import Scopes
from mutsukicore.contracts.error import Errs
from mutsukicore.contracts.ids import AgentId
from mutsukicore.core.agent import Agent
from mutsukicore.core.loader import PluginLoader, PluginLoadFailedError
from mutsukicore.runtime import DeterministicIdGen, SeededRng, SystemClock


class _PrefConfig(msgspec.Struct, kw_only=True):
    prefix: str = "echo: "


class _PrefPlugin(Plugin[_PrefConfig]):
    id: ClassVar[str] = "test-pref-config"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [
        Capability(name=Caps.READ_MESSAGE),
        Capability(name=Caps.SEND_MESSAGE),
    ]
    Config = _PrefConfig

    @command(perms=Perms.PUBLIC)
    async def show(self) -> str:
        """返回 prefix 以验证配置已被正确装载。"""
        return f"{self.config.prefix}ok"


def _new_agent() -> Agent:
    return Agent(
        agent_id=AgentId("config-test"),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(seed=0),
        accepts=(Scopes.IM_TEXT.to_rule(),),
    )


@pytest.mark.asyncio
async def test_loader_converts_raw_mapping_into_struct_config() -> None:
    agent = _new_agent()
    loader = PluginLoader(allow={_PrefPlugin.id})

    await loader.load_into(
        agent,
        [_PrefPlugin],
        configs=cast(
            Mapping[str, object],
            {_PrefPlugin.id: {"prefix": "v2: "}},
        ),
    )

    ctx = agent.make_context()
    result = await agent.dispatch.invoke("test-pref-config.show", {}, ctx=ctx)
    assert result == "v2: ok"

    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_loader_rejects_invalid_config_with_config_invalid_error() -> None:
    agent = _new_agent()
    loader = PluginLoader(allow={_PrefPlugin.id})

    with pytest.raises(PluginLoadFailedError) as ei:
        await loader.load_into(
            agent,
            [_PrefPlugin],
            configs=cast(
                Mapping[str, object],
                {_PrefPlugin.id: {"prefix": 123}},
            ),
        )

    assert ei.value.error.code == Errs.PLUGIN_CONFIG_INVALID
    assert agent.plugins == []
