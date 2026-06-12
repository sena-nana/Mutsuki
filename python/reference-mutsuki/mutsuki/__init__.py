"""Mutsuki —— Agent runtime kernel.

公开门面 re-export 最常用的符号。插件作者应该从 ``mutsuki`` 与
``mutsuki.contracts`` 导入，而不是直接深入到子模块。
"""

from mutsuki.contracts.agent_profile import (
    AgentParticipation,
    AgentProfile,
    ExecutionStrategy,
    SideEffectPolicy,
    StrategyResult,
    StrategyResultStatus,
)
from mutsuki.contracts.capability import Capability
from mutsuki.contracts.capability_builtin import Caps
from mutsuki.contracts.error import Errs
from mutsuki.contracts.lifecycle import LifecyclePhase
from mutsuki.contracts.permission_builtin import Perms
from mutsuki.contracts.plugin import Arg, Inject, RefArg, RefArgSource
from mutsuki.core.agent import Agent
from mutsuki.core.context import AgentContext
from mutsuki.core.plugin import Plugin, command, operation

__all__ = [
    "Agent",
    "AgentContext",
    "AgentParticipation",
    "AgentProfile",
    "Arg",
    "Capability",
    "Caps",
    "Errs",
    "ExecutionStrategy",
    "Inject",
    "LifecyclePhase",
    "Perms",
    "Plugin",
    "RefArg",
    "RefArgSource",
    "SideEffectPolicy",
    "StrategyResult",
    "StrategyResultStatus",
    "command",
    "operation",
]
