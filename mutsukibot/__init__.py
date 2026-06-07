"""MutsukiBot —— Agent 中心 Bot 框架运行核心。

公开门面 re-export 最常用的符号。插件作者应该从 ``mutsukibot`` 与
``mutsukibot.contracts`` 导入，而不是直接深入到子模块。
"""

from mutsukibot.contracts.agent_profile import (
    AgentParticipation,
    AgentProfile,
    ExecutionStrategy,
    SideEffectPolicy,
    StrategyResult,
    StrategyResultStatus,
)
from mutsukibot.contracts.capability import Capability
from mutsukibot.contracts.capability_builtin import Caps
from mutsukibot.contracts.error import Errs
from mutsukibot.contracts.lifecycle import LifecyclePhase
from mutsukibot.contracts.permission_builtin import Perms
from mutsukibot.contracts.plugin import Arg, Inject, RefArg, RefArgSource
from mutsukibot.core.agent import Agent
from mutsukibot.core.context import AgentContext
from mutsukibot.core.plugin import Plugin, command

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
]
