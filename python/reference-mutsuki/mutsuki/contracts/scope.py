"""ScopeRule 协议 —— envelope 路由谓词。

详见 :doc:`contracts §17 <plans/contracts>`。完全镜像
:class:`mutsuki.contracts.permission.PermissionRule` 的 AST 设计：

* 抽象基类 + ``_Leaf / _And / _Or`` 三个 AST 节点
* ``__and__`` / ``__or__`` 在组合时平展同类节点
* 与 PermissionRule 唯一区别：``check`` 是同步方法（envelope 匹配是纯
  数据计算，无 I/O）；PermissionRule.check 是 async 因为可能查 RBAC

消费点：

* ``Agent.accepts: tuple[ScopeRule, ...]`` —— dispatcher 路由 envelope 时
  筛选目标 Agent
* ``Plugin.consumes: ClassVar[tuple[ScopeRule, ...]]`` —— scheduler 二次分发
* ``@command(consumes=...)`` —— 命令级粒度细化
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, ClassVar

from mutsuki.contracts._registered import RegisteredString

if TYPE_CHECKING:
    from mutsuki.contracts.capability import CapabilityName
    from mutsuki.contracts.envelope import Envelope
    from mutsuki.contracts.source import SourceKindName


CheckerFn = Callable[["Envelope"], bool]


class UnknownScopeError(Exception):
    """构造未注册的 scope 名时抛出。"""


class ScopeConflictError(Exception):
    """两个不同插件试图注册同一 scope 名时抛出。"""


class ScopeRule:
    """可组合的 envelope 路由谓词（抽象基类 + 三个 AST 节点）。

    通过 :meth:`from_checker` 构造叶子节点，用 ``&`` / ``|`` 组合，调用
    :meth:`check` 求值。子类（``_Leaf`` / ``_And`` / ``_Or``）覆盖
    :meth:`check`，``__and__`` / ``__or__`` 在组合时把同类节点平展以保持
    AST 浅且可读。
    """

    __slots__ = ()

    def check(self, envelope: "Envelope") -> bool:
        raise NotImplementedError

    @classmethod
    def from_checker(cls, fn: CheckerFn) -> "ScopeRule":
        return _Leaf(fn)

    @classmethod
    def always(cls) -> "ScopeRule":
        return _Leaf(lambda _e: True)

    @classmethod
    def never(cls) -> "ScopeRule":
        return _Leaf(lambda _e: False)

    def __and__(self, other: object) -> "ScopeRule":
        if not isinstance(other, ScopeRule):
            return NotImplemented  # type: ignore[return-value]
        left = self.parts if isinstance(self, _And) else (self,)
        right = other.parts if isinstance(other, _And) else (other,)
        return _And(left + right)

    def __or__(self, other: object) -> "ScopeRule":
        if not isinstance(other, ScopeRule):
            return NotImplemented  # type: ignore[return-value]
        left = self.parts if isinstance(self, _Or) else (self,)
        right = other.parts if isinstance(other, _Or) else (other,)
        return _Or(left + right)


@dataclass(frozen=True, slots=True)
class _Leaf(ScopeRule):
    checker: CheckerFn

    def check(self, envelope: "Envelope") -> bool:
        return self.checker(envelope)


@dataclass(frozen=True, slots=True)
class _And(ScopeRule):
    parts: tuple[ScopeRule, ...]

    def check(self, envelope: "Envelope") -> bool:
        return all(p.check(envelope) for p in self.parts)


@dataclass(frozen=True, slots=True)
class _Or(ScopeRule):
    parts: tuple[ScopeRule, ...]

    def check(self, envelope: "Envelope") -> bool:
        return any(p.check(envelope) for p in self.parts)


# ---------------------------------------------------------------------------
# 内置叶子构造器（6 个 By* 谓词）
# ---------------------------------------------------------------------------


def BySchema(schema_id: str) -> ScopeRule:
    """匹配 ``envelope.payload_schema_id == schema_id``。"""

    def _check(envelope: "Envelope") -> bool:
        return envelope.payload_schema_id == schema_id

    return ScopeRule.from_checker(_check)


def BySchemaPrefix(prefix: str) -> ScopeRule:
    """匹配 ``envelope.payload_schema_id.startswith(prefix)``。"""

    def _check(envelope: "Envelope") -> bool:
        return envelope.payload_schema_id.startswith(prefix)

    return ScopeRule.from_checker(_check)


def BySourceId(source_id: str) -> ScopeRule:
    """匹配 ``envelope.source.source_id == source_id``。"""

    def _check(envelope: "Envelope") -> bool:
        return envelope.source.source_id == source_id

    return ScopeRule.from_checker(_check)


def BySourceKind(kind: "SourceKindName") -> ScopeRule:
    """匹配 ``envelope.source.kind == kind``。"""

    def _check(envelope: "Envelope") -> bool:
        return envelope.source.kind == kind

    return ScopeRule.from_checker(_check)


def ByCapability(cap: "CapabilityName") -> ScopeRule:
    """匹配 ``cap in envelope.capabilities_required``。"""

    def _check(envelope: "Envelope") -> bool:
        return cap in envelope.capabilities_required

    return ScopeRule.from_checker(_check)


def BySourceField(field: str, value: Any) -> ScopeRule:
    """匹配 ``envelope.source`` 任意字段精确等于 ``value``。"""

    def _check(envelope: "Envelope") -> bool:
        return getattr(envelope.source, field, None) == value

    return ScopeRule.from_checker(_check)


# ---------------------------------------------------------------------------
# ScopeName —— 注册式命名 scope
# ---------------------------------------------------------------------------


class ScopeName(RegisteredString):
    """已注册的命名 scope（``str`` 子类）。

    与 :class:`PermissionName` 同模式：``str`` 子类 + 注册表 + 内置门面
    :class:`Scopes`（在 :mod:`mutsuki.contracts.scope_builtin`）。注册
    时绑定一个 ScopeRule 工厂；通过 :meth:`to_rule` 取得 rule 实例。
    """

    _noun: ClassVar[str] = "scope"
    _unknown_error: ClassVar[type[Exception]] = UnknownScopeError
    _conflict_error: ClassVar[type[Exception]] = ScopeConflictError
    _rule: ClassVar[dict[str, ScopeRule]] = {}

    @classmethod
    def register(  # type: ignore[override]
        cls,
        name: str,
        *,
        declared_by: str,
        rule: ScopeRule,
    ) -> "ScopeName":
        instance = cls._intern(name, declared_by=declared_by)
        cls._rule.setdefault(name, rule)
        return instance

    def to_rule(self) -> ScopeRule:
        return self._rule[self]


__all__ = [
    "ByCapability",
    "BySchema",
    "BySchemaPrefix",
    "BySourceField",
    "BySourceId",
    "BySourceKind",
    "CheckerFn",
    "ScopeConflictError",
    "ScopeName",
    "ScopeRule",
    "UnknownScopeError",
]
