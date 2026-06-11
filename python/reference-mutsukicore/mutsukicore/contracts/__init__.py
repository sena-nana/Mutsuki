"""MutsukiCore 内部协议定义。

本包只包含类型定义与注册表门面。除内置 capability、permission、错误码常量
的 bootstrap 注册外，禁止任何运行时副作用。
"""

from mutsukicore.contracts.agent_profile import (
    AgentParticipation,
    AgentProfile,
    ExecutionStrategy,
    SideEffectPolicy,
    StrategyResult,
    StrategyResultStatus,
)
from mutsukicore.contracts.base import Contract, SchemaRegistry
from mutsukicore.contracts.capability import (
    Capability,
    CapabilityConflictError,
    CapabilityName,
    UnknownCapabilityError,
)
from mutsukicore.contracts.capability_builtin import Caps
from mutsukicore.contracts.decision import Decision
from mutsukicore.contracts.envelope import Envelope, SourceRef
from mutsukicore.contracts.error import Error, ErrorCode, Errs, RecoveryAction
from mutsukicore.contracts.event import Event, SpanStatus, TraceSpan
from mutsukicore.contracts.ids import (
    AgentId,
    EnvelopeId,
    MessageId,
    RefId,
    SpanId,
    TraceId,
)
from mutsukicore.contracts.lifecycle import LifecyclePhase
from mutsukicore.contracts.operation import OperationDep, OperationDescriptor
from mutsukicore.contracts.permission import (
    PermissionConflictError,
    PermissionName,
    PermissionRule,
    UnknownPermissionError,
)
from mutsukicore.contracts.permission_builtin import Perms
from mutsukicore.contracts.plugin import (
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
from mutsukicore.contracts.refpayload import (
    BackpressureChannel,
    Handle,
    RefDescriptor,
    RefPayload,
    Replayability,
)
from mutsukicore.contracts.resource_host import (
    ResourceHostPolicyConfig,
    ResourceRecordSelector,
)
from mutsukicore.contracts.schema import register_schema_compatibility
from mutsukicore.contracts.scope import (
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
from mutsukicore.contracts.scope_builtin import Scopes
from mutsukicore.contracts.service import Service, ServiceMode
from mutsukicore.contracts.source import (
    SourceDep,
    SourceDescriptor,
    SourceKindConflictError,
    SourceKindName,
    UnknownSourceKindError,
)
from mutsukicore.contracts.source_builtin import SourceKinds

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
        from mutsukicore_ext import im

        return getattr(im, name)
    raise AttributeError(name)
