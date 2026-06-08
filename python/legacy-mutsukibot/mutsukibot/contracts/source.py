"""Source 协议 —— 事件推送端的标识声明。

详见 :doc:`contracts §15 <plans/contracts>`。Source 是「plugin 主动 publish
envelope 时使用的来源标识」的注册式声明，不需要 handler，仅声明 source_id +
kind + capabilities，用于：

1. ``Envelope.source.source_id`` 字段引用
2. ``ScopeRule`` 中 ``BySourceId`` / ``BySourceKind`` 路由匹配
3. dashboard / 审计区分事件来源

Source 与 Operation（参见 :mod:`mutsukibot.contracts.operation`）共享命名空间
约定：``qq:bot1`` 是 Source ``source_id``，``qq:bot1.send_msg`` 是 Operation
``op_id`` —— 同一 plugin 同时声明二者表达"一个外部集成"。
"""

from __future__ import annotations

from typing import ClassVar

from mutsukibot.contracts._registered import RegisteredString
from mutsukibot.contracts.base import Contract
from mutsukibot.contracts.capability import CapabilityName


class UnknownSourceKindError(Exception):
    """构造未注册的 source kind 名时抛出。"""


class SourceKindConflictError(Exception):
    """两个不同插件试图注册同一 source kind 名时抛出。"""


class SourceKindName(RegisteredString):
    """已注册的 Source kind 名（``str`` 子类）。

    与 :class:`CapabilityName` 同模式：``str`` 子类 + 注册表 + 内置门面
    :class:`SourceKinds`。插件可注册自有 kind（如 ``mcp.fs``）。
    """

    _noun: ClassVar[str] = "source kind"
    _unknown_error: ClassVar[type[Exception]] = UnknownSourceKindError
    _conflict_error: ClassVar[type[Exception]] = SourceKindConflictError


class SourceDescriptor(Contract):
    """静态声明的事件源。"""

    schema_id: ClassVar[str] = "mutsukibot.source_descriptor"
    schema_version: ClassVar[str] = "1.0.0"

    source_id: str
    kind: SourceKindName
    capabilities: tuple[CapabilityName, ...] = ()
    description: str = ""


class SourceDep(Contract):
    """插件依赖外部 Source 的声明（用于 DAG 反向解析）。"""

    schema_id: ClassVar[str] = "mutsukibot.source_dep"
    schema_version: ClassVar[str] = "1.0.0"

    source_id: str
    required_caps: tuple[CapabilityName, ...] = ()


__all__ = [
    "SourceDep",
    "SourceDescriptor",
    "SourceKindConflictError",
    "SourceKindName",
    "UnknownSourceKindError",
]
