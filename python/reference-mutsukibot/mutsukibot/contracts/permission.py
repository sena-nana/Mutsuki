"""Permission 系统 —— 调用准入的运行时谓词。

Permission 与 capability **正交**：capability 表达「插件**能**做某事」；
permission 表达「**当下这个调用者，在这个上下文中**，是否被允许调用」。

两层结构：

* :class:`PermissionRule` —— 显式 AST 谓词组合，支持 ``&``（AND）与 ``|``
  （OR）操作符。语义借鉴 NoneBot 的 ``Rule`` / ``Permission``，但合并成
  单一类型；规则保持完整布尔语义（``(a|b) & (c|d)`` 严格按 (a OR b) AND
  (c OR d) 求值，不会退化为四项 OR）。
* :class:`PermissionName` —— 已注册的命名权限，便于稳定引用（门面：:class:`Perms`）。

插件可通过 :meth:`PermissionName.register` 注册新名字。
"""

from __future__ import annotations

from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from typing import TYPE_CHECKING, ClassVar

from mutsukibot.contracts._registered import RegisteredString

if TYPE_CHECKING:
    from mutsukibot.core.context import AgentContext


CheckerFn = Callable[["AgentContext"], Awaitable[bool]]


class UnknownPermissionError(Exception):
    """构造未注册的 permission 名时抛出。"""


class PermissionConflictError(Exception):
    """两个不同插件试图注册同一 permission 名时抛出。"""


class PermissionRule:
    """可组合的权限谓词（抽象基类 + 三个 AST 节点）。

    通过 :meth:`from_checker` 构造叶子节点，用 ``&`` / ``|`` 组合，调用
    :meth:`check` 求值。子类（``_Leaf`` / ``_And`` / ``_Or``）覆盖
    :meth:`check`，``__and__`` / ``__or__`` 在组合时把同类节点平展以保持
    AST 浅且可读。
    """

    __slots__ = ()

    async def check(self, ctx: "AgentContext") -> bool:
        raise NotImplementedError

    @classmethod
    def from_checker(cls, fn: CheckerFn) -> "PermissionRule":
        return _Leaf(fn)

    @classmethod
    def always(cls) -> "PermissionRule":
        async def _ok(_ctx: "AgentContext") -> bool:
            return True

        return _Leaf(_ok)

    @classmethod
    def never(cls) -> "PermissionRule":
        async def _no(_ctx: "AgentContext") -> bool:
            return False

        return _Leaf(_no)

    def __and__(self, other: object) -> "PermissionRule":
        if not isinstance(other, PermissionRule):
            return NotImplemented  # type: ignore[return-value]
        left = self.parts if isinstance(self, _And) else (self,)
        right = other.parts if isinstance(other, _And) else (other,)
        return _And(left + right)

    def __or__(self, other: object) -> "PermissionRule":
        if not isinstance(other, PermissionRule):
            return NotImplemented  # type: ignore[return-value]
        left = self.parts if isinstance(self, _Or) else (self,)
        right = other.parts if isinstance(other, _Or) else (other,)
        return _Or(left + right)


@dataclass(frozen=True, slots=True)
class _Leaf(PermissionRule):
    checker: CheckerFn

    async def check(self, ctx: "AgentContext") -> bool:
        return await self.checker(ctx)


@dataclass(frozen=True, slots=True)
class _And(PermissionRule):
    parts: tuple[PermissionRule, ...]

    async def check(self, ctx: "AgentContext") -> bool:
        for part in self.parts:
            if not await part.check(ctx):
                return False
        return True


@dataclass(frozen=True, slots=True)
class _Or(PermissionRule):
    parts: tuple[PermissionRule, ...]

    async def check(self, ctx: "AgentContext") -> bool:
        for part in self.parts:
            if await part.check(ctx):
                return True
        return False


class PermissionName(RegisteredString):
    """已注册的命名权限（``str`` 子类）。

    每个名字关联一个 checker 调用对象，通过 :meth:`register` 注册。该名字
    在类型层面就是字符串，通过 :meth:`to_rule` 得到 :class:`PermissionRule`。
    """

    _noun: ClassVar[str] = "permission"
    _unknown_error: ClassVar[type[Exception]] = UnknownPermissionError
    _conflict_error: ClassVar[type[Exception]] = PermissionConflictError
    _checker: ClassVar[dict[str, CheckerFn]] = {}

    @classmethod
    def register(  # type: ignore[override]
        cls,
        name: str,
        *,
        declared_by: str,
        checker: CheckerFn,
    ) -> "PermissionName":
        instance = cls._intern(name, declared_by=declared_by)
        # 第一次注册才记 checker；幂等重注册不覆盖（同 owner 已校验）。
        cls._checker.setdefault(name, checker)
        return instance

    def to_rule(self) -> PermissionRule:
        return PermissionRule.from_checker(self._checker[self])

    def __and__(self, other: object) -> PermissionRule:  # type: ignore[override]
        if isinstance(other, PermissionName):
            return self.to_rule() & other.to_rule()
        if isinstance(other, PermissionRule):
            return self.to_rule() & other
        return NotImplemented  # type: ignore[return-value]

    def __or__(self, other: object) -> PermissionRule:  # type: ignore[override]
        if isinstance(other, PermissionName):
            return self.to_rule() | other.to_rule()
        if isinstance(other, PermissionRule):
            return self.to_rule() | other
        return NotImplemented  # type: ignore[return-value]


__all__ = [
    "CheckerFn",
    "PermissionConflictError",
    "PermissionName",
    "PermissionRule",
    "UnknownPermissionError",
]
