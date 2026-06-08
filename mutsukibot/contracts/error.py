"""结构化 Error 契约与已注册的 ErrorCode 门面。

按 :doc:`AGENTS.md hard rule §8 <AGENTS>` 的约定，错误是一等数据，不是字符串。
``ErrorCode`` 与 :class:`CapabilityName` 采用相同的注册式字符串模式；
:class:`Errs` 门面暴露内置错误码。
"""

from __future__ import annotations

from enum import StrEnum
from typing import ClassVar, Self

from mutsukibot.contracts._registered import RegisteredString
from mutsukibot.contracts.base import Contract
from mutsukibot.contracts.capability import CapabilityName


class UnknownErrorCodeError(Exception):
    """构造未注册的错误码时抛出。"""


class ErrorCodeConflictError(Exception):
    """两个不同插件试图注册同一错误码时抛出。"""


class ErrorCode(RegisteredString):
    """已注册的错误码（``str`` 子类）。"""

    _noun: ClassVar[str] = "ErrorCode"
    _unknown_error: ClassVar[type[Exception]] = UnknownErrorCodeError
    _conflict_error: ClassVar[type[Exception]] = ErrorCodeConflictError


class RecoveryAction(StrEnum):
    RETRY = "retry"
    FALLBACK = "fallback"
    ESCALATE = "escalate"
    ABORT = "abort"


class Error(Contract):
    """带因果链与恢复提示的结构化错误。"""

    schema_id: ClassVar[str] = "mutsukibot.error"
    schema_version: ClassVar[str] = "1.0.0"

    code: ErrorCode
    source: str
    route: str
    lost_capability: CapabilityName | None = None
    recovery: RecoveryAction | None = None
    cause: "Error | None" = None
    evidence: dict[str, str | int | float | bool] = {}

    def chain(self) -> list[Self]:
        result: list[Self] = []
        cur: Self | None = self
        while cur is not None:
            result.append(cur)
            cur = cur.cause  # type: ignore[assignment]
        return result


_OWNER = "mutsukibot.core"


class Errs:
    """所有 MutsukiBot 框架内置错误码常量。"""

    CAPABILITY_NOT_DECLARED: ClassVar[ErrorCode]
    CAPABILITY_EXHAUSTED: ClassVar[ErrorCode]
    SCHEMA_MISMATCH: ClassVar[ErrorCode]
    HANDLE_LEAK: ClassVar[ErrorCode]
    HANDLE_USE_AFTER_RELEASE: ClassVar[ErrorCode]
    REF_NOT_FOUND: ClassVar[ErrorCode]
    REF_KIND_MISMATCH: ClassVar[ErrorCode]
    REF_CROSS_DOMAIN: ClassVar[ErrorCode]
    REF_SERIALIZE_ATTEMPT: ClassVar[ErrorCode]
    PLUGIN_CYCLE: ClassVar[ErrorCode]
    PLUGIN_DEPENDENCY_MISSING: ClassVar[ErrorCode]
    PLUGIN_SCOPE_VIOLATION: ClassVar[ErrorCode]
    PLUGIN_DEFINITION_ERROR: ClassVar[ErrorCode]
    PLUGIN_LOAD_FAILED: ClassVar[ErrorCode]
    PLUGIN_CONFIG_INVALID: ClassVar[ErrorCode]
    COMMAND_EXECUTION_FAILED: ClassVar[ErrorCode]
    COMMAND_INVALID_ARGS: ClassVar[ErrorCode]
    SERVICE_NOT_FOUND: ClassVar[ErrorCode]
    TRANSACTION_COMPENSATION_FAILED: ClassVar[ErrorCode]
    PERMISSION_DENIED: ClassVar[ErrorCode]
    UNKNOWN_CAPABILITY: ClassVar[ErrorCode]
    SYNC_VIOLATION: ClassVar[ErrorCode]
    # v0.2 新增 —— Operation / Source / Scope 路由错误码
    OPERATION_NOT_FOUND: ClassVar[ErrorCode]
    OPERATION_UNDECLARED: ClassVar[ErrorCode]
    OPERATION_CONFLICT: ClassVar[ErrorCode]
    OPERATION_UNHEALTHY: ClassVar[ErrorCode]
    OPERATION_INVOKE_FAILED: ClassVar[ErrorCode]
    OPERATION_HANDLER_RAISED: ClassVar[ErrorCode]
    SOURCE_UNREGISTERED: ClassVar[ErrorCode]
    SOURCE_CONFLICT: ClassVar[ErrorCode]
    SOURCE_UNDECLARED: ClassVar[ErrorCode]
    SCOPE_NO_MATCH: ClassVar[ErrorCode]
    # v0.3 后续 —— ResourceHost 策略治理错误码
    RESOURCE_POLICY_INVALID: ClassVar[ErrorCode]
    RESOURCE_POLICY_CONFLICT: ClassVar[ErrorCode]
    # v0.3 新增 —— 多 Agent 协作错误码
    AGENT_NOT_FOUND: ClassVar[ErrorCode]
    # v0.3 后续 —— trace 回放 / 记录错误码
    TRACE_RECORD_INVALID: ClassVar[ErrorCode]
    TRACE_REPLAY_FAILED: ClassVar[ErrorCode]
    # Rust / Python runtime backend boundary
    RUNTIME_BACKEND_FAILED: ClassVar[ErrorCode]
    RUNTIME_BACKEND_GENERATION_MISMATCH: ClassVar[ErrorCode]


ErrorCode.bootstrap_facade(
    Errs,
    {
        "CAPABILITY_NOT_DECLARED": "capability.not_declared",
        "CAPABILITY_EXHAUSTED": "capability.exhausted",
        "SCHEMA_MISMATCH": "schema.mismatch",
        "HANDLE_LEAK": "handle.leak",
        "HANDLE_USE_AFTER_RELEASE": "handle.use_after_release",
        "REF_NOT_FOUND": "ref.not_found",
        "REF_KIND_MISMATCH": "ref.kind_mismatch",
        "REF_CROSS_DOMAIN": "ref.cross_domain",
        "REF_SERIALIZE_ATTEMPT": "ref.serialize_attempt",
        "PLUGIN_CYCLE": "plugin.cycle",
        "PLUGIN_DEPENDENCY_MISSING": "plugin.dependency_missing",
        "PLUGIN_SCOPE_VIOLATION": "plugin.scope_violation",
        "PLUGIN_DEFINITION_ERROR": "plugin.definition_error",
        "PLUGIN_LOAD_FAILED": "plugin.load_failed",
        "PLUGIN_CONFIG_INVALID": "plugin.config_invalid",
        "COMMAND_EXECUTION_FAILED": "command.execution_failed",
        "COMMAND_INVALID_ARGS": "command.invalid_args",
        "SERVICE_NOT_FOUND": "service.not_found",
        "TRANSACTION_COMPENSATION_FAILED": "transaction.compensation_failed",
        "PERMISSION_DENIED": "permission.denied",
        "UNKNOWN_CAPABILITY": "capability.unknown",
        "SYNC_VIOLATION": "plugin.sync_violation",
        "OPERATION_NOT_FOUND": "operation.not_found",
        "OPERATION_UNDECLARED": "operation.undeclared",
        "OPERATION_CONFLICT": "operation.conflict",
        "OPERATION_UNHEALTHY": "operation.unhealthy",
        "OPERATION_INVOKE_FAILED": "operation.invoke_failed",
        "OPERATION_HANDLER_RAISED": "operation.handler_raised",
        "SOURCE_UNREGISTERED": "source.unregistered",
        "SOURCE_CONFLICT": "source.conflict",
        "SOURCE_UNDECLARED": "source.undeclared",
        "SCOPE_NO_MATCH": "scope.no_match",
        "RESOURCE_POLICY_INVALID": "resource.policy_invalid",
        "RESOURCE_POLICY_CONFLICT": "resource.policy_conflict",
        "AGENT_NOT_FOUND": "agent.not_found",
        "TRACE_RECORD_INVALID": "trace.record_invalid",
        "TRACE_REPLAY_FAILED": "trace.replay_failed",
        "RUNTIME_BACKEND_FAILED": "runtime.backend_failed",
        "RUNTIME_BACKEND_GENERATION_MISMATCH": "runtime.backend_generation_mismatch",
    },
    declared_by=_OWNER,
)


__all__ = [
    "Error",
    "ErrorCode",
    "ErrorCodeConflictError",
    "Errs",
    "RecoveryAction",
    "UnknownErrorCodeError",
]
