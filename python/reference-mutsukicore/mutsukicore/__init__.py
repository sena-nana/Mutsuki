"""MutsukiCore —— Agent runtime kernel.

公开门面 re-export 最常用的符号。插件作者应该从 ``mutsukicore`` 与
``mutsukicore.contracts`` 导入，而不是直接深入到子模块。
"""

from mutsukicore.contracts.agent_profile import (
    AgentParticipation,
    AgentProfile,
    ExecutionStrategy,
    SideEffectPolicy,
    StrategyResult,
    StrategyResultStatus,
)
from mutsukicore.contracts.capability import Capability
from mutsukicore.contracts.capability_builtin import Caps
from mutsukicore.contracts.error import Errs
from mutsukicore.contracts.lifecycle import LifecyclePhase
from mutsukicore.contracts.permission_builtin import Perms
from mutsukicore.contracts.plugin import Arg, Inject, RefArg, RefArgSource
from mutsukicore.core.agent import Agent
from mutsukicore.core.context import AgentContext
from mutsukicore.core.plugin import Plugin, command, operation

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
