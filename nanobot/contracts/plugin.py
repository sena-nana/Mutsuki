"""插件 manifest 契约与装饰器侧的 sentinel 标记。

面向用户的高层装饰器（``@command`` 等）位于 :mod:`nanobot.core.plugin`。
本模块只暴露 *契约形态* 与命令签名里使用的 ``Annotated`` 友好标记。
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, ClassVar

from nanobot.contracts.base import Contract
from nanobot.contracts.capability import Capability, CapabilityName
from nanobot.contracts.service import ServiceMode


@dataclass(frozen=True, slots=True)
class Arg:
    """命令参数的 Annotated 标记 —— 约束 + fallback 描述。

    按设计约定，描述通常应来自函数 docstring（Google 风格的 ``Args:`` 段）。
    本标记主要用于 *约束*（``ge`` / ``le`` / ``min_length`` / ``max_length``
    / ``regex`` / ``choices``），仅在 docstring 缺失时作为描述的 fallback。
    """

    desc: str | None = None
    ge: int | float | None = None
    le: int | float | None = None
    gt: int | float | None = None
    lt: int | float | None = None
    min_length: int | None = None
    max_length: int | None = None
    regex: str | None = None
    choices: tuple[str, ...] | None = None


@dataclass(frozen=True, slots=True)
class Inject:
    """命令签名里用于服务 / 配置注入的 sentinel 标记。

    用法：``svc: SomeService = Inject()`` 或 ``cfg: Config = Inject()``。
    """

    name: str | None = None


@dataclass(frozen=True, slots=True)
class RefArg:
    """按引用 handle 参数的 Annotated 标记。

    用法::

        latent: Annotated[Handle[Any], RefArg(kind="yume.latent")]
    """

    kind: str


class PluginDep(Contract):
    """插件 → 插件依赖声明（DAG 边）。"""

    schema_id: ClassVar[str] = "nanobot.plugin_dep"
    schema_version: ClassVar[str] = "1.0.0"

    plugin_id: str
    version_range: str = "*"


class ContractDep(Contract):
    """插件 → 契约包依赖。"""

    schema_id: ClassVar[str] = "nanobot.contract_dep"
    schema_version: ClassVar[str] = "1.0.0"

    package: str
    version_range: str = "*"


class ServiceDep(Contract):
    """服务依赖 / 提供声明。"""

    schema_id: ClassVar[str] = "nanobot.service_dep"
    schema_version: ClassVar[str] = "1.0.0"

    name: str
    contract_id: str
    mode: ServiceMode = ServiceMode.BY_REF


class CommandSpec(Contract):
    """已编译的命令 / 工具规约（由 ``@command`` 装饰器自动生成）。"""

    schema_id: ClassVar[str] = "nanobot.command_spec"
    schema_version: ClassVar[str] = "1.0.0"

    name: str
    description: str
    plugin_id: str
    func_qualname: str
    parameters_schema: dict[str, Any] = {}
    return_schema: dict[str, Any] = {}
    perms_rule_id: str | None = None
    requires_capabilities: tuple[CapabilityName, ...] = ()
    is_tool: bool = True


class PluginManifest(Contract):
    """插件的静态元数据（由 ``Plugin`` 子类自动构造）。"""

    schema_id: ClassVar[str] = "nanobot.plugin_manifest"
    schema_version: ClassVar[str] = "1.0.0"

    id: str
    version: str
    contracts: tuple[ContractDep, ...] = ()
    capabilities: tuple[Capability, ...] = ()
    provides_services: tuple[ServiceDep, ...] = ()
    requires_services: tuple[ServiceDep, ...] = ()
    requires_plugins: tuple[PluginDep, ...] = ()
    config_schema_id: str = ""
    commands: tuple[CommandSpec, ...] = ()


# 给下游调用者用的、对 lint 友好的空列表工厂兼容钩子。
def _empty_caps() -> list[Capability]:
    return field(default_factory=list)  # pragma: no cover


__all__ = [
    "Arg",
    "CommandSpec",
    "ContractDep",
    "Inject",
    "PluginDep",
    "PluginManifest",
    "RefArg",
    "ServiceDep",
]
