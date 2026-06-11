"""AgentProfile 与 ExecutionStrategy 契约。

本模块只定义 Agent 参与方式与策略接口，不实现策略运行循环。
Agent 仍是身份、生命周期、权限与调度边界的主体；ExecutionStrategy 只是在
Agent 授权边界内决定下一步如何推进。
"""

from __future__ import annotations

from collections.abc import Awaitable
from enum import StrEnum
from typing import TYPE_CHECKING, ClassVar, Protocol, runtime_checkable

from mutsukicore.contracts.base import Contract
from mutsukicore.contracts.decision import Decision
from mutsukicore.contracts.envelope import Envelope
from mutsukicore.contracts.error import Error
from mutsukicore.contracts.scope import ScopeRule

if TYPE_CHECKING:
    from mutsukicore.core.context import AgentContext


class AgentParticipation(StrEnum):
    """Agent 在外部输入流中的参与方式。"""

    PRIMARY_CANDIDATE = "primary_candidate"
    OBSERVER = "observer"
    EXPLICIT_HELPER = "explicit_helper"


class SideEffectPolicy(StrEnum):
    """AgentProfile 的最小副作用策略声明。"""

    READ_ONLY = "read_only"
    ALLOW_EXTERNAL = "allow_external"


class StrategyResultStatus(StrEnum):
    """ExecutionStrategy 推进一次后的状态。"""

    CONTINUE = "continue"
    WAIT_INPUT = "wait_input"
    COMPLETED = "completed"
    FAILED = "failed"


class AgentProfile(Contract):
    """Agent 角色、外部输入参与方式与策略选择的配置层。"""

    schema_id: ClassVar[str] = "mutsukicore.agent_profile"
    schema_version: ClassVar[str] = "1.0.0"

    profile_id: str
    participation: AgentParticipation
    accepts: tuple[ScopeRule, ...] = ()
    strategy_id: str = ""
    side_effect_policy: SideEffectPolicy = SideEffectPolicy.READ_ONLY


class StrategyResult(Contract):
    """ExecutionStrategy 单次推进结果。"""

    schema_id: ClassVar[str] = "mutsukicore.strategy_result"
    schema_version: ClassVar[str] = "1.0.0"

    status: StrategyResultStatus
    decision: Decision | None = None
    emitted: tuple[Envelope, ...] = ()
    error: Error | None = None


@runtime_checkable
class ExecutionStrategy(Protocol):
    """Agent 内部的下一步推进策略接口。"""

    strategy_id: str
    supported_profiles: tuple[str, ...]

    def on_awake(self, ctx: "AgentContext") -> Awaitable[None]: ...

    def on_input(
        self,
        ctx: "AgentContext",
        envelope: Envelope,
    ) -> Awaitable[StrategyResult]: ...

    def on_stop(self, ctx: "AgentContext") -> Awaitable[None]: ...

    def next_step(self, ctx: "AgentContext") -> Awaitable[StrategyResult]: ...


__all__ = [
    "AgentParticipation",
    "AgentProfile",
    "ExecutionStrategy",
    "SideEffectPolicy",
    "StrategyResult",
    "StrategyResultStatus",
]
