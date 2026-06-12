"""PluginMeta 校验行为。"""

from __future__ import annotations

from typing import ClassVar

import msgspec
import pytest

from mutsuki import Capability, Caps, Plugin, command
from mutsuki.core.plugin import PluginDefinitionError
from mutsuki.core.registry import PluginRegistry


def test_plugin_subclass_registered_in_registry() -> None:
    class GoodPlugin(Plugin):
        id: ClassVar[str] = "test-plugin-good"
        version: ClassVar[str] = "0.1.0"
        capabilities: ClassVar[list[Capability]] = [Capability(name=Caps.READ_MESSAGE)]

        class Config(msgspec.Struct, kw_only=True):
            pass

    assert PluginRegistry.get("test-plugin-good") is GoodPlugin


def test_missing_id_classvar_rejected() -> None:
    with pytest.raises(PluginDefinitionError):

        class BadPlugin(Plugin):
            version: ClassVar[str] = "0.1.0"
            capabilities: ClassVar[list[Capability]] = []

            class Config(msgspec.Struct, kw_only=True):
                pass


def test_missing_config_struct_rejected() -> None:
    with pytest.raises(PluginDefinitionError):

        class BadPlugin(Plugin):
            id: ClassVar[str] = "test-plugin-noconfig"
            version: ClassVar[str] = "0.1.0"
            capabilities: ClassVar[list[Capability]] = []


def test_command_collected_into_manifest() -> None:
    class WithCmd(Plugin):
        id: ClassVar[str] = "test-plugin-with-cmd"
        version: ClassVar[str] = "0.1.0"
        capabilities: ClassVar[list[Capability]] = []

        class Config(msgspec.Struct, kw_only=True):
            pass

        @command()
        async def hello(self, name: str) -> str:
            """打招呼。

            Args:
                name: 招呼对象。
            """
            return f"hello, {name}"

    assert len(WithCmd.__commands__) == 1
    spec = WithCmd.__commands__[0]
    assert spec.name == "hello"
    assert spec.description == "打招呼。"
    assert spec.parameters_schema["properties"]["name"]["description"] == "招呼对象。"
    assert "name" in spec.parameters_schema["required"]


def test_command_must_be_async() -> None:
    with pytest.raises(TypeError):

        class _BadAsync(Plugin):
            id: ClassVar[str] = "test-plugin-bad-sync"
            version: ClassVar[str] = "0.1.0"
            capabilities: ClassVar[list[Capability]] = []

            class Config(msgspec.Struct, kw_only=True):
                pass

            @command()  # type: ignore[arg-type]
            def sync_cmd(self, x: str) -> str:
                return x


def test_dependent_cached_on_marker_at_class_definition() -> None:
    """v0.1 P2 优化：Dependent 在 PluginMeta 阶段解析后缓存，避免 per-tick inspect。"""
    from mutsuki.core.dependency import Dependent

    class WithCachedDep(Plugin):
        id: ClassVar[str] = "test-plugin-cached-dep"
        version: ClassVar[str] = "0.1.0"
        capabilities: ClassVar[list[Capability]] = []

        class Config(msgspec.Struct, kw_only=True):
            pass

        @command()
        async def ping(self, who: str) -> str:
            """ping who."""
            return who

    marker = WithCachedDep.__command_markers__["ping"]
    assert marker.dependent is not None
    assert isinstance(marker.dependent, Dependent)
    # 复用同一个 marker —— 一致引用证明缓存命中。
    assert WithCachedDep.__command_markers__["ping"].dependent is marker.dependent
