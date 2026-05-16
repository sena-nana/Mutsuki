"""MutsukiBot 内部协议定义。

本包只包含类型定义与注册表门面。除内置 capability、permission、错误码常量
的 bootstrap 注册外，禁止任何运行时副作用。
"""

from mutsukibot.contracts.base import Contract, SchemaRegistry
from mutsukibot.contracts.capability import (
    Capability,
    CapabilityConflictError,
    CapabilityName,
    UnknownCapabilityError,
)
from mutsukibot.contracts.capability_builtin import Caps
from mutsukibot.contracts.decision import Decision
from mutsukibot.contracts.envelope import Envelope, SourceRef, ToolEvent, ToolSourceRef
from mutsukibot.contracts.error import Error, ErrorCode, Errs, RecoveryAction
from mutsukibot.contracts.event import Event, SpanStatus, TraceSpan
from mutsukibot.contracts.ids import (
    AgentId,
    EnvelopeId,
    MessageId,
    RefId,
    SpanId,
    TraceId,
)
from mutsukibot.contracts.lifecycle import LifecyclePhase
from mutsukibot.contracts.message import ChannelRef, ContentKind, ContentPart, Message
from mutsukibot.contracts.operation import OperationDep, OperationDescriptor
from mutsukibot.contracts.permission import (
    PermissionConflictError,
    PermissionName,
    PermissionRule,
    UnknownPermissionError,
)
from mutsukibot.contracts.permission_builtin import Perms
from mutsukibot.contracts.plugin import (
    Arg,
    CommandSpec,
    ContractDep,
    Inject,
    PluginDep,
    PluginManifest,
    RefArg,
    RefArgSource,
    ServiceDep,
)
from mutsukibot.contracts.refpayload import (
    BackpressureChannel,
    Handle,
    RefDescriptor,
    RefPayload,
    Replayability,
)
from mutsukibot.contracts.schema import register_schema_compatibility
from mutsukibot.contracts.scope import (
    ByCapability,
    BySchema,
    BySchemaPrefix,
    BySourceField,
    BySourceId,
    BySourceKind,
    ScopeConflictError,
    ScopeName,
    ScopeRule,
    UnknownScopeError,
)
from mutsukibot.contracts.scope_builtin import Scopes
from mutsukibot.contracts.service import Service, ServiceMode
from mutsukibot.contracts.source import (
    SourceDep,
    SourceDescriptor,
    SourceKindConflictError,
    SourceKindName,
    UnknownSourceKindError,
)
from mutsukibot.contracts.source_builtin import SourceKinds

__all__ = [
    "AgentId",
    "Arg",
    "BackpressureChannel",
    "ByCapability",
    "BySchema",
    "BySchemaPrefix",
    "BySourceField",
    "BySourceId",
    "BySourceKind",
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
    "Envelope",
    "EnvelopeId",
    "Error",
    "ErrorCode",
    "Errs",
    "Event",
    "Handle",
    "Inject",
    "LifecyclePhase",
    "Message",
    "MessageId",
    "OperationDep",
    "OperationDescriptor",
    "PermissionConflictError",
    "PermissionName",
    "PermissionRule",
    "Perms",
    "PluginDep",
    "PluginManifest",
    "RecoveryAction",
    "RefArg",
    "RefArgSource",
    "RefDescriptor",
    "RefId",
    "RefPayload",
    "Replayability",
    "SchemaRegistry",
    "ScopeConflictError",
    "ScopeName",
    "ScopeRule",
    "Scopes",
    "Service",
    "ServiceDep",
    "ServiceMode",
    "SourceDep",
    "SourceDescriptor",
    "SourceKindConflictError",
    "SourceKindName",
    "SourceKinds",
    "SourceRef",
    "SpanId",
    "SpanStatus",
    "ToolEvent",
    "ToolSourceRef",
    "TraceId",
    "TraceSpan",
    "UnknownCapabilityError",
    "UnknownPermissionError",
    "UnknownScopeError",
    "UnknownSourceKindError",
    "register_schema_compatibility",
]
