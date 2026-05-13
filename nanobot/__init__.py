"""NanoBot —— Agent 中心 Bot 框架运行核心。

公开门面 re-export 最常用的符号。插件作者应该从 ``nanobot`` 与
``nanobot.contracts`` 导入，而不是直接深入到子模块。
"""

from nanobot.contracts.capability import Capability
from nanobot.contracts.capability_builtin import Caps
from nanobot.contracts.error import Errs
from nanobot.contracts.lifecycle import LifecyclePhase
from nanobot.contracts.permission_builtin import Perms
from nanobot.contracts.plugin import Arg, Inject, RefArg
from nanobot.core.agent import Agent
from nanobot.core.context import AgentContext
from nanobot.core.plugin import Plugin, command

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
