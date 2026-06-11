from __future__ import annotations

import asyncio
import contextlib
import json

import msgspec
import pytest

from mutsukicore import Capability, Caps, Plugin, command
from mutsukicore.contracts import AgentId, Scopes
from mutsukicore.contracts.error import Errs
from mutsukicore.core.agent import Agent
from mutsukicore.core.agent_registry import AgentRegistry
from mutsukicore.core.dispatcher import OperationInvokeError
from mutsukicore.core.loader import PluginLoader
from mutsukicore.runtime import DeterministicIdGen, SeededRng, SystemClock
from mutsukicore.runtime.scheduler import AgentScheduler
from mutsukicore_ext.command import TextCommandRouterPlugin
from mutsukicore_ext.im import ChannelRef, Message
from tests.support.dispatcher_contract import assert_dispatcher_clean_after_unload


def _agent(name: str = "onebot-test") -> Agent:
    AgentRegistry.clear()
    return Agent(
        agent_id=AgentId(name),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(seed=0),
        accepts=(Scopes.IM_TEXT.to_rule(),),
    )


async def _loaded_onebot(agent: Agent, *, port: int = 0):
    from mutsukicore.plugins.onebot_v11 import OneBotV11Plugin

    loader = PluginLoader(allow={OneBotV11Plugin.id})
    await loader.load_into(
        agent,
        [OneBotV11Plugin],
        configs={OneBotV11Plugin.id: {"host": "127.0.0.1", "port": port}},
    )
    plugin = next(
        p.plugin for p in agent.plugins if isinstance(p.plugin, OneBotV11Plugin)
    )
    return loader, plugin


def test_private_message_event_builds_message_with_onebot_source() -> None:
    from mutsukicore.plugins.onebot_v11 import OneBotV11Plugin

    agent = _agent()
    msg = OneBotV11Plugin.message_from_event(
        agent,
        source_id="onebot:v11.default",
        event={
            "post_type": "message",
            "message_type": "private",
            "message_id": 1001,
            "time": 123.0,
            "user_id": 42,
            "raw_message": "echo hello",
            "message": "echo hello",
        },
    )

    assert isinstance(msg, Message)
    assert msg.id == "1001"
    assert msg.timestamp == 123.0
    assert msg.source.source_id == "onebot:v11.default"
    assert isinstance(msg.source, ChannelRef)
    assert msg.source.channel_id == "private:42"
    assert msg.source.user_id == "42"
    assert msg.text == "echo hello"
    assert msg.parts[0].metadata["onebot.message_type"] == "private"


def test_group_segment_message_extracts_text_and_records_non_text_segments() -> None:
    from mutsukicore.plugins.onebot_v11 import OneBotV11Plugin

    agent = _agent()
    msg = OneBotV11Plugin.message_from_event(
        agent,
        source_id="onebot:v11.default",
        event={
            "post_type": "message",
            "message_type": "group",
            "message_id": 1002,
            "time": 456.0,
            "group_id": 987,
            "user_id": 42,
            "message": [
                {"type": "text", "data": {"text": "todo "}},
                {"type": "image", "data": {"file": "x.jpg"}},
                {"type": "text", "data": {"text": "买菜"}},
            ],
        },
    )

    assert isinstance(msg.source, ChannelRef)
    assert msg.source.channel_id == "group:987"
    assert msg.source.user_id == "42"
    assert msg.text == "todo 买菜"
    assert msg.parts[0].metadata["onebot.non_text_segments"] == "1"


@pytest.mark.asyncio
async def test_send_msg_without_active_connection_raises_structured_error() -> None:
    agent = _agent()
    loader, _plugin = await _loaded_onebot(agent)

    with pytest.raises(OperationInvokeError) as ei:
        await agent.dispatch.invoke(
            "onebot:v11.default.send_msg",
            {
                "message_type": "private",
                "user_id": 42,
                "message": "hello",
            },
            ctx=agent.make_context(),
        )

    assert ei.value.error.code == Errs.OPERATION_INVOKE_FAILED
    assert ei.value.error.evidence["reason"] == "no_active_connection"

    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_send_msg_writes_onebot_action_and_matches_echo_response() -> None:
    import websockets

    agent = _agent()
    loader, plugin = await _loaded_onebot(agent)

    async with websockets.connect(plugin.url) as ws:
        task = asyncio.create_task(
            agent.dispatch.invoke(
                "onebot:v11.default.send_msg",
                {
                    "message_type": "group",
                    "group_id": 987,
                    "message": "echo: hello",
                    "auto_escape": False,
                },
                ctx=agent.make_context(),
            )
        )
        frame = json.loads(await asyncio.wait_for(ws.recv(), timeout=1.0))
        assert frame["action"] == "send_msg"
        assert frame["params"] == {
            "message_type": "group",
            "group_id": 987,
            "message": "echo: hello",
            "auto_escape": False,
        }

        await ws.send(
            json.dumps(
                {
                    "status": "ok",
                    "retcode": 0,
                    "data": {"message_id": 1234},
                    "echo": frame["echo"],
                }
            )
        )

        assert await asyncio.wait_for(task, timeout=1.0) == {"message_id": 1234}

    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_onebot_plugin_unload_cleans_dispatcher_registrations() -> None:
    from mutsukicore.plugins.onebot_v11 import OneBotV11Plugin

    agent = _agent()
    loader, _plugin = await _loaded_onebot(agent)

    await assert_dispatcher_clean_after_unload(
        loader,
        agent,
        operations=("onebot:v11.default.send_msg",),
        sources=("onebot:v11.default",),
    )

    assert OneBotV11Plugin.id == "mutsukicore-onebot-v11"


class _EchoConfig(msgspec.Struct, kw_only=True):
    pass


class _EchoPlugin(Plugin[_EchoConfig]):
    id = "test-onebot-echo"
    version = "0.0.1"
    capabilities = [Capability(name=Caps.READ_MESSAGE), Capability(name=Caps.SEND_MESSAGE)]
    Config = _EchoConfig

    @command()
    async def echo(self, text: str) -> str:
        return f"echo: {text}"


@pytest.mark.asyncio
async def test_reverse_ws_event_reaches_agent_and_outbox_pump_sends_reply() -> None:
    import websockets

    from mutsukicore.plugins.onebot_v11 import OneBotV11Plugin

    agent = _agent("onebot-smoke")
    loader = PluginLoader(
        allow={OneBotV11Plugin.id, _EchoPlugin.id, TextCommandRouterPlugin.id}
    )
    await loader.load_into(
        agent,
        [OneBotV11Plugin, TextCommandRouterPlugin, _EchoPlugin],
        configs={OneBotV11Plugin.id: {"host": "127.0.0.1", "port": 0}},
    )
    plugin = next(
        p.plugin for p in agent.plugins if isinstance(p.plugin, OneBotV11Plugin)
    )
    scheduler = AgentScheduler(agent)
    await scheduler.start()

    try:
        async with websockets.connect(plugin.url) as ws:
            await ws.send(
                json.dumps(
                    {
                        "post_type": "message",
                        "message_type": "private",
                        "message_id": 2001,
                        "time": 1.0,
                        "user_id": 42,
                        "message": "echo hello",
                    }
                )
            )
            frame = json.loads(await asyncio.wait_for(ws.recv(), timeout=1.0))
            assert frame["action"] == "send_msg"
            assert frame["params"]["message_type"] == "private"
            assert frame["params"]["user_id"] == 42
            assert frame["params"]["message"] == "echo: hello"
            await ws.send(
                json.dumps(
                    {
                        "status": "ok",
                        "retcode": 0,
                        "data": {"message_id": 2002},
                        "echo": frame["echo"],
                    }
                )
            )
    finally:
        with contextlib.suppress(Exception):
            await scheduler.stop()
        await loader.unload_from(agent)
