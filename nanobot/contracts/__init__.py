"""NanoBot 内部协议定义。

本包只包含类型定义与注册表门面。除内置 capability、permission、错误码常量
的 bootstrap 注册外，禁止任何运行时副作用。
"""

from nanobot.contracts.base import Contract, SchemaRegistry
from nanobot.contracts.capability import (
    Capability,
    CapabilityConflictError,
    CapabilityName,
    UnknownCapabilityError,
)
from nanobot.contracts.capability_builtin import Caps
from nanobot.contracts.decision import Decision
from nanobot.contracts.error import Error, ErrorCode, Errs, RecoveryAction
from nanobot.contracts.event import Event, SpanStatus, TraceSpan
from nanobot.contracts.ids import AgentId, MessageId, RefId, SpanId, TraceId
from nanobot.contracts.lifecycle import LifecyclePhase
from nanobot.contracts.message import ChannelRef, ContentKind, ContentPart, Message
from nanobot.contracts.permission import (
    PermissionConflictError,
    PermissionName,
    PermissionRule,
    UnknownPermissionError,
)
from nanobot.contracts.permission_builtin import Perms
from nanobot.contracts.plugin import (
    Arg,
    CommandSpec,
    ContractDep,
    Inject,
    PluginDep,
    PluginManifest,
    RefArg,
    ServiceDep,
)
from nanobot.contracts.refpayload import (
    BackpressureChannel,
    Handle,
    RefDescriptor,
    RefPayload,
    Replayability,
)
from nanobot.contracts.schema import register_schema_compatibility
from nanobot.contracts.service import Service, ServiceMode

__all__ = [
    "AgentId",
    "Arg",
    "BackpressureChannel",
    "Capability",
    "CapabilityConflictError",
    "CapabilityName",
    "Caps",
    "ChannelRef",
    "CommandSpec",
    "ContentKind",
    "ContentPart",
    "Contract",
    "ContractDep",
    "Decision",
    "Error",
    "ErrorCode",
    "Errs",
    "Event",
    "Handle",
    "Inject",
    "LifecyclePhase",
    "Message",
    "MessageId",
    "PermissionConflictError",
    "PermissionName",
    "PermissionRule",
    "Perms",
    "PluginDep",
    "PluginManifest",
    "RecoveryAction",
    "RefArg",
    "RefDescriptor",
    "RefId",
    "RefPayload",
    "Replayability",
    "SchemaRegistry",
    "Service",
    "ServiceDep",
    "ServiceMode",
    "SpanId",
    "SpanStatus",
    "TraceId",
    "TraceSpan",
    "UnknownCapabilityError",
    "UnknownPermissionError",
    "register_schema_compatibility",
]
