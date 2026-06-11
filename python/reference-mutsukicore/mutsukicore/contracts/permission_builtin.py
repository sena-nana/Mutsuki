"""框架内置的 permission 常量门面。

:class:`Perms` 门面让 pyright/IDE 能够准确推断 MutsukiCore 核心 permission 名。
插件应当采用相同模式自建门面类。
"""

from __future__ import annotations

from typing import TYPE_CHECKING, ClassVar

from mutsukicore.contracts.permission import PermissionName

if TYPE_CHECKING:
    from mutsukicore.core.context import AgentContext


_OWNER = "mutsukicore.core"


async def _public(_ctx: "AgentContext") -> bool:
    return True


async def _agent_owner(ctx: "AgentContext") -> bool:
    if ctx.message is None:
        return True
    src = ctx.message.source
    source_user = getattr(src, "user_id", None)
    if not isinstance(source_user, str):
        return False
    return source_user == ctx.agent_owner


class Perms:
    """所有 MutsukiCore 框架内置 permission 常量。"""

    PUBLIC: ClassVar[PermissionName]
    AGENT_OWNER: ClassVar[PermissionName]


def _bootstrap() -> None:
    Perms.PUBLIC = PermissionName.register(
        "public", declared_by=_OWNER, checker=_public
    )
    Perms.AGENT_OWNER = PermissionName.register(
        "agent_owner", declared_by=_OWNER, checker=_agent_owner
    )


_bootstrap()


__all__ = ["Perms"]
