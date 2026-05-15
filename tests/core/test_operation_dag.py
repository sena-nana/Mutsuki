"""D9b：Operation/Source 静态声明 + DAG 依赖。

* requires_operations / requires_sources 翻译为 plugin-level 依赖入拓扑
* 缺失依赖（请求未声明的 op_id / source_id）→ PluginDependencyMissingError
* 多 plugin 声明同一 op_id / source_id → 装载期 fail
* dispatcher.register_operation / register_source 校验 undeclared
"""

from __future__ import annotations

from typing import Any, ClassVar

import msgspec
import pytest

from mutsukibot import Capability, Caps, Plugin
from mutsukibot.contracts import (
    OperationDep,
    OperationDescriptor,
    Perms,
    SourceDescriptor,
    SourceKinds,
)
from mutsukibot.contracts.error import Errs
from mutsukibot.contracts.ids import AgentId
from mutsukibot.core.agent import Agent
from mutsukibot.core.dispatcher import (
    OperationUndeclaredError,
    SourceUndeclaredError,
)
from mutsukibot.core.loader import (
    OperationProvisionConflictError,
    PluginDependencyMissingError,
    PluginLoader,
    SourceProvisionConflictError,
)
from mutsukibot.runtime import DeterministicIdGen, SeededRng, SystemClock


class _Conf(msgspec.Struct, kw_only=True):
    pass


# ---- Provider plugin ----
_OP_A = OperationDescriptor(
    op_id="provA:default.op", name="op", plugin_id="test-prov-a"
)
_OP_B = OperationDescriptor(
    op_id="provB:default.op", name="op", plugin_id="test-prov-b"
)
_SRC_A = SourceDescriptor(source_id="srcA:default", kind=SourceKinds.IM)


class _ProviderAPlugin(Plugin[_Conf]):
    id: ClassVar[str] = "test-prov-a"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [Capability(name=Caps.READ_MESSAGE)]
    provides_operations: ClassVar[tuple[OperationDescriptor, ...]] = (_OP_A,)
    provides_sources: ClassVar[tuple[SourceDescriptor, ...]] = (_SRC_A,)
    Config = _Conf

    async def on_load(self) -> None:
        async def _h(_ctx, _payload):
            return "from-A"

        self.agent.dispatch.register_operation(
            _OP_A,
            handler=_h,
            perms=Perms.PUBLIC.to_rule(),
            plugin_scope=self.scope,
        )
        self.agent.dispatch.register_source(
            _SRC_A, plugin_scope=self.scope, plugin_id=self.id
        )


class _ConsumerPlugin(Plugin[_Conf]):
    id: ClassVar[str] = "test-consumer"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [Capability(name=Caps.READ_MESSAGE)]
    requires_operations: ClassVar[tuple[OperationDep, ...]] = (
        OperationDep(op_id="provA:default.op"),
    )
    Config = _Conf
    load_order: ClassVar[list[str]] = []

    async def on_load(self) -> None:
        type(self).load_order.append(self.id)


def _new_agent() -> Agent:
    return Agent(
        agent_id=AgentId("dag-test"),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(seed=0),
    )


@pytest.mark.asyncio
async def test_requires_operations_enforces_topological_order() -> None:
    """Consumer 依赖 ProviderA 提供的 op；不论传入顺序如何，A 必须先装载。"""
    _ConsumerPlugin.load_order.clear()
    _ProviderAPlugin.__dict__  # ensure imported
    agent = _new_agent()
    loader = PluginLoader(allow={_ProviderAPlugin.id, _ConsumerPlugin.id})
    # 故意把 consumer 放前面 —— DAG 必须仍把 provider 排前
    await loader.load_into(agent, [_ConsumerPlugin, _ProviderAPlugin])
    plugin_order = [entry.plugin.id for entry in agent.plugins]
    assert plugin_order.index(_ProviderAPlugin.id) < plugin_order.index(
        _ConsumerPlugin.id
    )
    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_requires_unprovided_operation_raises_dependency_missing() -> None:
    """consumer 声明 requires_operations 但提供方未在装载列表中 → fail-loud。"""
    agent = _new_agent()
    loader = PluginLoader(allow={_ConsumerPlugin.id})
    with pytest.raises(PluginDependencyMissingError) as ei:
        await loader.load_into(agent, [_ConsumerPlugin])
    assert ei.value.error.code == Errs.PLUGIN_DEPENDENCY_MISSING
    assert any(
        d.startswith("op:provA:default.op") for _src, d in ei.value.missing
    )


# ---- Provision conflict ----


class _ProviderA1Plugin(Plugin[_Conf]):
    """声明同一 op_id 与 _ProviderAPlugin 冲突。"""

    id: ClassVar[str] = "test-prov-a-dup"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = []
    provides_operations: ClassVar[tuple[OperationDescriptor, ...]] = (
        OperationDescriptor(
            op_id="provA:default.op",  # 故意撞名
            name="op",
            plugin_id="test-prov-a-dup",
        ),
    )
    Config = _Conf


class _ProviderASrcDupPlugin(Plugin[_Conf]):
    """声明同一 source_id 与 _ProviderAPlugin 冲突。"""

    id: ClassVar[str] = "test-prov-a-srcdup"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = []
    provides_sources: ClassVar[tuple[SourceDescriptor, ...]] = (
        SourceDescriptor(source_id="srcA:default", kind=SourceKinds.IM),
    )
    Config = _Conf


@pytest.mark.asyncio
async def test_duplicate_op_provision_raises_at_load() -> None:
    agent = _new_agent()
    loader = PluginLoader(allow={_ProviderAPlugin.id, _ProviderA1Plugin.id})
    with pytest.raises(OperationProvisionConflictError) as ei:
        await loader.load_into(agent, [_ProviderAPlugin, _ProviderA1Plugin])
    assert ei.value.op_id == "provA:default.op"
    assert ei.value.error.code == Errs.OPERATION_CONFLICT


@pytest.mark.asyncio
async def test_duplicate_source_provision_raises_at_load() -> None:
    agent = _new_agent()
    loader = PluginLoader(allow={_ProviderAPlugin.id, _ProviderASrcDupPlugin.id})
    with pytest.raises(SourceProvisionConflictError) as ei:
        await loader.load_into(agent, [_ProviderAPlugin, _ProviderASrcDupPlugin])
    assert ei.value.source_id == "srcA:default"
    assert ei.value.error.code == Errs.SOURCE_CONFLICT


# ---- Dispatcher runtime undeclared check ----


class _UndeclaredOpPlugin(Plugin[_Conf]):
    """on_load 偷偷注册一个未在 provides_operations 静态声明的 op_id。"""

    id: ClassVar[str] = "test-undeclared-op"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = []
    # 故意空 provides_operations，但 on_load 中尝试注册
    Config = _Conf

    async def on_load(self) -> None:
        async def _h(_ctx, _payload):
            return "x"

        bad_desc = OperationDescriptor(
            op_id="ghost.op",
            name="op",
            plugin_id=self.id,
        )
        self.agent.dispatch.register_operation(
            bad_desc,
            handler=_h,
            perms=Perms.PUBLIC.to_rule(),
            plugin_scope=self.scope,
        )


class _UndeclaredSrcPlugin(Plugin[_Conf]):
    """on_load 偷偷注册一个未在 provides_sources 静态声明的 source_id。"""

    id: ClassVar[str] = "test-undeclared-src"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = []
    Config = _Conf

    async def on_load(self) -> None:
        bad_desc = SourceDescriptor(
            source_id="ghost:src", kind=SourceKinds.IM
        )
        self.agent.dispatch.register_source(
            bad_desc, plugin_scope=self.scope, plugin_id=self.id
        )


@pytest.mark.asyncio
async def test_dispatcher_rejects_undeclared_operation_at_runtime() -> None:
    agent = _new_agent()
    loader = PluginLoader(allow={_UndeclaredOpPlugin.id})
    # PluginLoader.load_into 在 on_load 失败时会包成 PluginLoadFailedError
    from mutsukibot.core.loader import PluginLoadFailedError

    with pytest.raises(PluginLoadFailedError) as ei:
        await loader.load_into(agent, [_UndeclaredOpPlugin])
    cause = ei.value.__cause__
    assert isinstance(cause, OperationUndeclaredError)
    assert cause.error.code == Errs.OPERATION_UNDECLARED


@pytest.mark.asyncio
async def test_dispatcher_rejects_undeclared_source_at_runtime() -> None:
    agent = _new_agent()
    loader = PluginLoader(allow={_UndeclaredSrcPlugin.id})
    from mutsukibot.core.loader import PluginLoadFailedError

    with pytest.raises(PluginLoadFailedError) as ei:
        await loader.load_into(agent, [_UndeclaredSrcPlugin])
    cause = ei.value.__cause__
    assert isinstance(cause, SourceUndeclaredError)
    assert cause.error.code == Errs.SOURCE_UNDECLARED


# 兼容 mypy 用：消除 _ProviderA1Plugin 等仅声明类的"未使用"报警
_ALL_PLUGINS: tuple[type[Plugin[Any]], ...] = (
    _ProviderAPlugin,
    _ProviderA1Plugin,
    _ProviderASrcDupPlugin,
    _UndeclaredOpPlugin,
    _UndeclaredSrcPlugin,
)


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
