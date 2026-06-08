"""Operation 协议 —— 命令与跨 plugin RPC 统一表达。

详见 :doc:`contracts §14 <plans/contracts>`。Operation 是「插件暴露的可调用
物」的一等概念 —— ``@command`` 装饰的方法、显式 ``dispatch.register_operation``
注册的 op 都是其实例。op_id agent-local 全限定，约定
``<plugin_id>.<name>`` 或 ``<source_namespace>.<name>``。

CommandSpec（v0.1）是 OperationDescriptor 的 type alias —— 命令是 Operation
的特化（来自 @command 装饰器自动汇入），二者共用同一字段集。
"""

from __future__ import annotations

from typing import Any, ClassVar

from mutsukibot.contracts.base import Contract
from mutsukibot.contracts.capability import CapabilityName


class OperationDescriptor(Contract):
    """已编译的 Operation 规约（@command 自动生成 / 显式 register_operation 用户提供）。

    ``op_id`` 是 agent-local 全限定标识。``name`` 字段保留作为短名（首词
    匹配用，与 v0.1 ``CommandSpec.name`` 语义一致）。``plugin_id`` 标识
    declaring plugin。``func_qualname`` 仅用于诊断与 trace。
    """

    schema_id: ClassVar[str] = "mutsukibot.operation_descriptor"
    schema_version: ClassVar[str] = "1.0.0"

    op_id: str
    name: str
    description: str = ""
    plugin_id: str = ""
    func_qualname: str = ""
    parameters_schema: dict[str, Any] = {}
    return_schema: dict[str, Any] = {}
    perms_rule_id: str | None = None
    requires_capabilities: tuple[CapabilityName, ...] = ()
    is_tool: bool = True


class OperationDep(Contract):
    """插件依赖外部 Operation 的声明（用于 DAG 反向解析，详见 §14.4）。"""

    schema_id: ClassVar[str] = "mutsukibot.operation_dep"
    schema_version: ClassVar[str] = "1.0.0"

    op_id: str
    required_caps: tuple[CapabilityName, ...] = ()


__all__ = ["OperationDep", "OperationDescriptor"]
