"""插件 manifest 契约与装饰器侧的 sentinel 标记。

面向用户的高层装饰器（``@command`` 等）位于 :mod:`mutsukicore.core.plugin`。
本模块只暴露 *契约形态* 与命令签名里使用的 ``Annotated`` 友好标记。
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import StrEnum
from typing import ClassVar

from mutsukicore.contracts.base import Contract
from mutsukicore.contracts.capability import Capability
from mutsukicore.contracts.operation import OperationDep, OperationDescriptor
from mutsukicore.contracts.scope import ScopeRule
from mutsukicore.contracts.service import ServiceMode
from mutsukicore.contracts.source import SourceDep, SourceDescriptor


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


class RefArgSource(StrEnum):
    """RefArg 的解析来源。"""

    PAYLOAD = "payload"
    RESOURCE_HOST = "resource_host"


@dataclass(frozen=True, slots=True)
class RefArg:
    """按引用 handle 参数的 Annotated 标记。

    用法::

        latent: Annotated[Handle[Any], RefArg(kind="yume.latent")]
    """

    kind: str
    source: RefArgSource = RefArgSource.PAYLOAD
    ref_id: str | None = None
    host_name: str | None = None


class PluginDep(Contract):
    """插件 → 插件依赖声明（DAG 边）。"""

    schema_id: ClassVar[str] = "mutsukicore.plugin_dep"
    schema_version: ClassVar[str] = "1.0.0"

    plugin_id: str
    version_range: str = "*"


class ContractDep(Contract):
    """插件 → 契约包依赖。"""

    schema_id: ClassVar[str] = "mutsukicore.contract_dep"
    schema_version: ClassVar[str] = "1.0.0"

    package: str
    version_range: str = "*"


class ServiceDep(Contract):
    """服务依赖 / 提供声明。"""

    schema_id: ClassVar[str] = "mutsukicore.service_dep"
    schema_version: ClassVar[str] = "1.0.0"

    name: str
    contract_id: str
    mode: ServiceMode = ServiceMode.BY_REF


# CommandSpec 在 v0.2 起是 OperationDescriptor 的 type alias —— 命令是
# Operation 的特化（来自 @command 装饰器自动汇入），二者共用同一字段集。
# 详见 contracts.md §14（Operation 协议）与 D12（命令与 Operation 统一）。
CommandSpec = OperationDescriptor


class PluginManifest(Contract):
    """插件的静态元数据（由 ``Plugin`` 子类自动构造）。

    v0.2 新增 ``consumes`` / ``provides_operations`` / ``provides_sources``
    / ``requires_operations`` / ``requires_sources`` 五个字段（参见
    contracts.md §14-§17 与 D9b 静态声明 + DAG 依赖）。
    """

    schema_id: ClassVar[str] = "mutsukicore.plugin_manifest"
    schema_version: ClassVar[str] = "1.0.0"

    id: str
    version: str
    contracts: tuple[ContractDep, ...] = ()
    capabilities: tuple[Capability, ...] = ()
    provides_services: tuple[ServiceDep, ...] = ()
    requires_services: tuple[ServiceDep, ...] = ()
    requires_plugins: tuple[PluginDep, ...] = ()
    config_schema_id: str = ""
    commands: tuple[OperationDescriptor, ...] = ()
    # v0.2 新增字段
    consumes: tuple[ScopeRule, ...] = ()
    provides_operations: tuple[OperationDescriptor, ...] = ()
    provides_sources: tuple[SourceDescriptor, ...] = ()
    requires_operations: tuple[OperationDep, ...] = ()
    requires_sources: tuple[SourceDep, ...] = ()


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
    "RefArgSource",
    "ServiceDep",
]
