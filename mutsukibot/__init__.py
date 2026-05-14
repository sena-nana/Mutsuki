"""MutsukiBot —— Agent 中心 Bot 框架运行核心。

公开门面 re-export 最常用的符号。插件作者应该从 ``mutsukibot`` 与
``mutsukibot.contracts`` 导入，而不是直接深入到子模块。
"""

from mutsukibot.contracts.capability import Capability
from mutsukibot.contracts.capability_builtin import Caps
from mutsukibot.contracts.error import Errs
from mutsukibot.contracts.lifecycle import LifecyclePhase
from mutsukibot.contracts.permission_builtin import Perms
from mutsukibot.contracts.plugin import Arg, Inject, RefArg
from mutsukibot.core.agent import Agent
from mutsukibot.core.context import AgentContext
from mutsukibot.core.plugin import Plugin, command

__all__ = [
    "Agent",
    "AgentContext",
    "Arg",
    "Capability",
    "Caps",
    "Errs",
    "Inject",
    "LifecyclePhase",
    "Perms",
    "Plugin",
    "RefArg",
    "command",
]
