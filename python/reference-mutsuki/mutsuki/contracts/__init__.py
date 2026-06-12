"""Mutsuki 内部协议定义。

本包只包含类型定义与注册表门面。除内置 capability、permission、错误码常量
的 bootstrap 注册外，禁止任何运行时副作用。
"""

from mutsuki.contracts.agent_profile import (
    AgentParticipation,
    AgentProfile,
    ExecutionStrategy,
    SideEffectPolicy,
    StrategyResult,
    StrategyResultStatus,
)
from mutsuki.contracts.base import Contract, SchemaRegistry
from mutsuki.contracts.capability import (
    Capability,
    CapabilityConflictError,
    CapabilityName,
    UnknownCapabilityError,
)
from mutsuki.contracts.capability_builtin import Caps
from mutsuki.contracts.decision import Decision
from mutsuki.contracts.envelope import Envelope, SourceRef
from mutsuki.contracts.error import Error, ErrorCode, Errs, RecoveryAction
from mutsuki.contracts.event import Event, SpanStatus, TraceSpan
from mutsuki.contracts.ids import (
    AgentId,
    EnvelopeId,
    MessageId,
    RefId,
    SpanId,
    TraceId,
)
from mutsuki.contracts.lifecycle import LifecyclePhase
from mutsuki.contracts.operation import OperationDep, OperationDescriptor
from mutsuki.contracts.permission import (
    PermissionConflictError,
    PermissionName,
    PermissionRule,
    UnknownPermissionError,
)
from mutsuki.contracts.permission_builtin import Perms
from mutsuki.contracts.plugin import (
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
from mutsuki.contracts.refpayload import (
    BackpressureChannel,
    Handle,
    RefDescriptor,
    RefPayload,
    Replayability,
)
from mutsuki.contracts.resource_host import (
    ResourceHostPolicyConfig,
    ResourceRecordSelector,
)
from mutsuki.contracts.schema import register_schema_compatibility
from mutsuki.contracts.scope import (
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
from mutsuki.contracts.scope_builtin import Scopes
from mutsuki.contracts.service import Service, ServiceMode
from mutsuki.contracts.source import (
    SourceDep,
    SourceDescriptor,
    SourceKindConflictError,
    SourceKindName,
    UnknownSourceKindError,
)
from mutsuki.contracts.source_builtin import SourceKinds

__all__ = [
    "AgentId",
    "AgentParticipation",
    "AgentProfile",
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
    "CommandSpec",
    "Contract",
    "ContractDep",
    "Decision",
    "Envelope",
    "EnvelopeId",
    "Error",
    "ErrorCode",
    "Errs",
    "Event",
    "ExecutionStrategy",
    "Handle",
    "Inject",
    "LifecyclePhase",
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
    "ResourceHostPolicyConfig",
    "ResourceRecordSelector",
    "SchemaRegistry",
    "ScopeConflictError",
    "ScopeName",
    "ScopeRule",
    "Scopes",
    "Service",
    "ServiceDep",
    "ServiceMode",
    "SideEffectPolicy",
    "SourceDep",
    "SourceDescriptor",
    "SourceKindConflictError",
    "SourceKindName",
    "SourceKinds",
    "SourceRef",
    "SpanId",
    "SpanStatus",
    "StrategyResult",
    "StrategyResultStatus",
    "TraceId",
    "TraceSpan",
    "UnknownCapabilityError",
    "UnknownPermissionError",
    "UnknownScopeError",
    "UnknownSourceKindError",
    "register_schema_compatibility",
]


def __getattr__(name: str):
    if name in {"ChannelRef", "ContentKind", "ContentPart", "Message"}:
        from mutsuki_ext import im

        return getattr(im, name)
    raise AttributeError(name)
