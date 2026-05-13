"""结构化 Error 契约与已注册的 ErrorCode 门面。

按 :doc:`AGENTS.md hard rule §8 <AGENTS>` 的约定，错误是一等数据，不是字符串。
``ErrorCode`` 与 :class:`CapabilityName` 采用相同的注册式字符串模式；
:class:`Errs` 门面暴露内置错误码。
"""

from __future__ import annotations

from enum import StrEnum
from typing import ClassVar, Self

from nanobot.contracts._registered import RegisteredString
from nanobot.contracts.base import Contract
from nanobot.contracts.capability import CapabilityName


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

    schema_id: ClassVar[str] = "nanobot.error"
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


_OWNER = "nanobot.core"


class Errs:
    """所有 NanoBot 框架内置错误码常量。"""

    CAPABILITY_NOT_DECLARED: ClassVar[ErrorCode]
    CAPABILITY_EXHAUSTED: ClassVar[ErrorCode]
    SCHEMA_MISMATCH: ClassVar[ErrorCode]
    HANDLE_LEAK: ClassVar[ErrorCode]
    HANDLE_USE_AFTER_RELEASE: ClassVar[ErrorCode]
    REF_CROSS_DOMAIN: ClassVar[ErrorCode]
    REF_SERIALIZE_ATTEMPT: ClassVar[ErrorCode]
    PLUGIN_CYCLE: ClassVar[ErrorCode]
    PLUGIN_SCOPE_VIOLATION: ClassVar[ErrorCode]
    PLUGIN_DEFINITION_ERROR: ClassVar[ErrorCode]
    TRANSACTION_COMPENSATION_FAILED: ClassVar[ErrorCode]
    PERMISSION_DENIED: ClassVar[ErrorCode]
    UNKNOWN_CAPABILITY: ClassVar[ErrorCode]
    SYNC_VIOLATION: ClassVar[ErrorCode]


ErrorCode.bootstrap_facade(
    Errs,
    {
        "CAPABILITY_NOT_DECLARED": "capability.not_declared",
        "CAPABILITY_EXHAUSTED": "capability.exhausted",
        "SCHEMA_MISMATCH": "schema.mismatch",
        "HANDLE_LEAK": "handle.leak",
        "HANDLE_USE_AFTER_RELEASE": "handle.use_after_release",
        "REF_CROSS_DOMAIN": "ref.cross_domain",
        "REF_SERIALIZE_ATTEMPT": "ref.serialize_attempt",
        "PLUGIN_CYCLE": "plugin.cycle",
        "PLUGIN_SCOPE_VIOLATION": "plugin.scope_violation",
        "PLUGIN_DEFINITION_ERROR": "plugin.definition_error",
        "TRANSACTION_COMPENSATION_FAILED": "transaction.compensation_failed",
        "PERMISSION_DENIED": "permission.denied",
        "UNKNOWN_CAPABILITY": "capability.unknown",
        "SYNC_VIOLATION": "plugin.sync_violation",
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
