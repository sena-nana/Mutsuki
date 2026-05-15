"""框架内置 ScopeName 常量门面。

:class:`Scopes` 暴露 MutsukiBot 核心常用 ScopeRule 命名集合。插件采用相同
模式自建门面（如
``YumeScopes.THOUGHT = ScopeName.register("yume.thought", ..., rule=...)``）。
"""

from __future__ import annotations

from typing import ClassVar

from mutsukibot.contracts.capability_builtin import Caps
from mutsukibot.contracts.scope import (
    ByCapability,
    BySchema,
    BySchemaPrefix,
    BySourceKind,
    ScopeName,
)
from mutsukibot.contracts.source_builtin import SourceKinds

_OWNER = "mutsukibot.core"


class Scopes:
    """所有 MutsukiBot 框架内置 ScopeName 常量。"""

    IM_TEXT: ClassVar[ScopeName]
    IM_ANY: ClassVar[ScopeName]
    TOOL_INVOKE: ClassVar[ScopeName]
    TOOL_EVENT: ClassVar[ScopeName]


Scopes.IM_TEXT = ScopeName.register(
    "im.text",
    declared_by=_OWNER,
    rule=BySchema("mutsukibot.message")
    & BySourceKind(SourceKinds.IM)
    & ByCapability(Caps.IM_TEXT),
)
Scopes.IM_ANY = ScopeName.register(
    "im.any",
    declared_by=_OWNER,
    rule=BySchema("mutsukibot.message") & BySourceKind(SourceKinds.IM),
)
Scopes.TOOL_INVOKE = ScopeName.register(
    "tool.invoke",
    declared_by=_OWNER,
    rule=BySchemaPrefix("mutsukibot.tool_event") & BySourceKind(SourceKinds.TOOL),
)
Scopes.TOOL_EVENT = ScopeName.register(
    "tool.event",
    declared_by=_OWNER,
    rule=BySchema("mutsukibot.tool_event") & BySourceKind(SourceKinds.TOOL),
)


__all__ = ["Scopes"]
