"""D12：@command 装饰器自动转 Operation 注册到 dispatcher。

验证：

* @command 装饰的方法在 plugin 装载时自动产生一个 Operation 注册到 dispatcher
* op_id 命名约定 ``<plugin_id>.<method_name>``
* CommandSpec / OperationDescriptor 是同一类型（v0.2 alias）
* 通过 dispatch.invoke 调用与 scheduler 文本路径行为一致（perm/cap/return）
* @command 派生 op_id 与显式 register_operation 不冲突（不同命名空间）
"""

from __future__ import annotations

from typing import ClassVar

import msgspec
import pytest

from mutsukicore import Capability, Caps, Perms, Plugin, command
from mutsukicore.contracts import (
    OperationDescriptor,
    Scopes,
)
from mutsukicore.contracts.ids import AgentId
from mutsukicore.contracts.plugin import CommandSpec
from mutsukicore.core.agent import Agent
from mutsukicore.core.loader import PluginLoader
from mutsukicore.runtime import DeterministicIdGen, SeededRng, SystemClock


class _Conf(msgspec.Struct, kw_only=True):
    pass


class _GreetPlugin(Plugin[_Conf]):
    id: ClassVar[str] = "test-greet-d12"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [
        Capability(name=Caps.READ_MESSAGE),
        Capability(name=Caps.SEND_MESSAGE),
    ]
    Config = _Conf

    @command(perms=Perms.PUBLIC)
    async def greet(self, who: str = "world") -> str:
        """问候 someone."""
        return f"hi {who}"

    @command(perms=Perms.PUBLIC)
    async def shout(self, text: str) -> str:
        """大声说。"""
        return text.upper()


def _new_agent() -> Agent:
    return Agent(
        agent_id=AgentId("d12-test"),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(seed=0),
        accepts=(Scopes.IM_TEXT.to_rule(),),
    )


def test_commandspec_is_operationdescriptor_alias() -> None:
    """CommandSpec 在 v0.2 是 OperationDescriptor 的 type alias。"""
    assert CommandSpec is OperationDescriptor


def test_command_op_id_follows_plugin_id_dot_method_convention() -> None:
    """@command 派生 op_id = <plugin_id>.<method_name>。"""
    op_ids = {op.op_id for op in _GreetPlugin.provides_operations}
    assert "test-greet-d12.greet" in op_ids
    assert "test-greet-d12.shout" in op_ids


def test_command_methods_auto_merged_into_provides_operations() -> None:
    """PluginMeta 把 @command 派生 op 自动汇入 provides_operations。"""
    assert len(_GreetPlugin.provides_operations) == 2
    # 都是 OperationDescriptor 实例
    for op in _GreetPlugin.provides_operations:
        assert isinstance(op, OperationDescriptor)


@pytest.mark.asyncio
async def test_command_registered_into_dispatcher_at_attach() -> None:
    agent = _new_agent()
    loader = PluginLoader(allow={_GreetPlugin.id})
    await loader.load_into(agent, [_GreetPlugin])

    op_ids = {op.op_id for op in agent.dispatch.list_operations()}
    assert "test-greet-d12.greet" in op_ids
    assert "test-greet-d12.shout" in op_ids
    # 短名查找：scheduler 文本路径用此机制
    assert agent.dispatch.lookup_operation("greet") == "test-greet-d12.greet"
    assert agent.dispatch.lookup_operation("shout") == "test-greet-d12.shout"

    await loader.unload_from(agent)
    # 卸载后 dispatcher 中无残留
    assert agent.dispatch.list_operations() == ()


@pytest.mark.asyncio
async def test_invoke_command_via_dispatcher_returns_handler_result() -> None:
    """dispatch.invoke 调用 @command 派生的 op，行为等价 scheduler 文本路径。"""
    agent = _new_agent()
    loader = PluginLoader(allow={_GreetPlugin.id})
    await loader.load_into(agent, [_GreetPlugin])

    ctx = agent.make_context()
    result = await agent.dispatch.invoke(
        "test-greet-d12.greet", {"who": "alice"}, ctx=ctx
    )
    assert result == "hi alice"

    result2 = await agent.dispatch.invoke(
        "test-greet-d12.shout", {"text": "wow"}, ctx=ctx
    )
    assert result2 == "WOW"

    await loader.unload_from(agent)


# ---- 显式 register_operation 与 @command 共存测试 ----

_EXPLICIT_OP = OperationDescriptor(
    op_id="test-mixed.explicit_op",
    name="explicit_op",
    plugin_id="test-mixed",
)


class _MixedPlugin(Plugin[_Conf]):
    """同时有 @command 派生 op 和 on_load 显式注册的 op。"""

    id: ClassVar[str] = "test-mixed"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [Capability(name=Caps.READ_MESSAGE)]
    provides_operations: ClassVar[tuple[OperationDescriptor, ...]] = (_EXPLICIT_OP,)
    Config = _Conf

    @command(perms=Perms.PUBLIC)
    async def cmd_op(self) -> str:
        """A @command-derived op."""
        return "from-cmd"

    async def on_load(self) -> None:
        async def _h(_ctx, _payload):
            return "from-explicit"

        self.agent.dispatch.register_operation(
            _EXPLICIT_OP,
            handler=_h,
            perms=Perms.PUBLIC.to_rule(),
            plugin_scope=self.scope,
        )


@pytest.mark.asyncio
async def test_command_and_explicit_op_coexist() -> None:
    """同 plugin 内 @command 和显式 register_operation 共存，互不冲突。"""
    agent = _new_agent()
    loader = PluginLoader(allow={_MixedPlugin.id})
    await loader.load_into(agent, [_MixedPlugin])

    op_ids = {op.op_id for op in agent.dispatch.list_operations()}
    # @command 派生
    assert "test-mixed.cmd_op" in op_ids
    # on_load 显式
    assert "test-mixed.explicit_op" in op_ids

    ctx = agent.make_context()
    a = await agent.dispatch.invoke("test-mixed.cmd_op", {}, ctx=ctx)
    b = await agent.dispatch.invoke("test-mixed.explicit_op", {}, ctx=ctx)
    assert a == "from-cmd"
    assert b == "from-explicit"

    await loader.unload_from(agent)


def test_explicit_op_clashing_with_command_op_id_rejected_at_define() -> None:
    """PluginMeta 在类定义时检测：显式声明的 op_id 不得与 @command 派生 op_id 撞名。"""
    from mutsukicore.core.plugin import PluginDefinitionError

    with pytest.raises(PluginDefinitionError):

        class _Bad(Plugin[_Conf]):
            id: ClassVar[str] = "test-bad-d12"
            version: ClassVar[str] = "0.0.1"
            capabilities: ClassVar[list[Capability]] = []
            # 显式声明撞名 @command 派生 op_id
            provides_operations: ClassVar[tuple[OperationDescriptor, ...]] = (
                OperationDescriptor(
                    op_id="test-bad-d12.foo",
                    name="foo",
                    plugin_id="test-bad-d12",
                ),
            )
            Config = _Conf

            @command(perms=Perms.PUBLIC)
            async def foo(self) -> str:
                """f."""
                return "f"


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
